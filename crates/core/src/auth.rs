//! B+ 设备认证：Ed25519 请求签名、时间窗与重放缓存。
//!
//! 规格：`docs/specs/phase1-core-auth.md` §2。
//!
//! 签名串（UTF-8，`\n` 分隔，无结尾换行）：
//! `cista.b+1\n<METHOD>\n<PATH>\n<sha256(body).hex>\n<nonce.hex>\n<ts>`

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// 时钟偏移窗口（±秒）。服务端与客户端都要遵守。
pub const MAX_SKEW_SECS: i64 = 120;
/// nonce 记忆时长：必须显著大于偏移窗口，保证窗口内无法重放。
pub const NONCE_TTL_SECS: i64 = 300;
/// nonce 允许的原始字节数范围。
pub const NONCE_MIN_LEN: usize = 16;
pub const NONCE_MAX_LEN: usize = 32;

const DOMAIN: &str = "cista.b+1";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("时钟偏移超出允许窗口")]
    ClockSkew,
    #[error("nonce 长度非法")]
    BadNonce,
    #[error("设备公钥非法")]
    BadPublicKey,
    #[error("签名非法")]
    BadSignature,
}

/// 签名素材。`body_sha256` 覆盖原始请求体，路径含 query。
#[derive(Debug, Clone)]
pub struct RequestMeta<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub body_sha256: [u8; 32],
    pub nonce: &'a [u8],
    pub ts: i64,
}

impl<'a> RequestMeta<'a> {
    pub fn new(method: &'a str, path: &'a str, body: &'a [u8], nonce: &'a [u8], ts: i64) -> Self {
        let digest = Sha256::digest(body);
        let mut body_sha256 = [0u8; 32];
        body_sha256.copy_from_slice(&digest);
        Self {
            method,
            path,
            body_sha256,
            nonce,
            ts,
        }
    }

    /// 规范串；任何字段变化都必须改变签名结果。
    pub fn canonical(&self) -> Vec<u8> {
        format!(
            "{DOMAIN}\n{}\n{}\n{}\n{}\n{}",
            self.method.to_uppercase(),
            self.path,
            hex::encode(self.body_sha256),
            hex::encode(self.nonce),
            self.ts
        )
        .into_bytes()
    }
}

/// 设备私钥。只在设备上存在；Drop 时 dalek 会清零内部字节。
pub struct DeviceSecret(SigningKey);

impl DeviceSecret {
    /// 用 CSPRNG 生成设备密钥对。
    pub fn generate() -> Result<Self, AuthError> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|_| AuthError::BadPublicKey)?;
        Ok(Self(SigningKey::from_bytes(&seed)))
    }

    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    /// `device_id = hex(sha256(pubkey))[..32]`，与私钥一一对应、可被服务端复算。
    pub fn device_id(&self) -> String {
        device_id(&self.public_key())
    }

    /// 返回签名的 hex（128 字符）。
    pub fn sign(&self, meta: &RequestMeta<'_>) -> String {
        hex::encode(self.0.sign(&meta.canonical()).to_bytes())
    }
}

pub fn device_id(pubkey: &[u8; 32]) -> String {
    let digest = Sha256::digest(pubkey);
    hex::encode(&digest[..16])
}

/// 校验 nonce 长度；服务端在查重放缓存前调用。
pub fn validate_nonce(nonce: &[u8]) -> Result<(), AuthError> {
    if (NONCE_MIN_LEN..=NONCE_MAX_LEN).contains(&nonce.len()) {
        Ok(())
    } else {
        Err(AuthError::BadNonce)
    }
}

/// 验签（含时间窗）。调用方负责确认设备未吊销。
pub fn verify(
    pubkey: &[u8; 32],
    meta: &RequestMeta<'_>,
    signature_hex: &str,
    now: i64,
) -> Result<(), AuthError> {
    if (now - meta.ts).abs() > MAX_SKEW_SECS {
        return Err(AuthError::ClockSkew);
    }
    let verifying_key = VerifyingKey::from_bytes(pubkey).map_err(|_| AuthError::BadPublicKey)?;
    let bytes = hex::decode(signature_hex).map_err(|_| AuthError::BadSignature)?;
    let signature = Signature::from_slice(&bytes).map_err(|_| AuthError::BadSignature)?;
    verifying_key
        .verify_strict(&meta.canonical(), &signature)
        .map_err(|_| AuthError::BadSignature)
}

/// 进程内 nonce 重放缓存（纯逻辑，无 IO）。服务端用 `Mutex` 包住。
#[derive(Debug)]
pub struct ReplayCache {
    ttl_secs: i64,
    seen: HashMap<String, i64>,
}

impl Default for ReplayCache {
    fn default() -> Self {
        Self::new(NONCE_TTL_SECS)
    }
}

impl ReplayCache {
    pub fn new(ttl_secs: i64) -> Self {
        Self {
            ttl_secs: ttl_secs.max(1),
            seen: HashMap::new(),
        }
    }

    /// 记录 `(device, nonce)`；返回 `false` 表示重放（窗口内已见过）。
    ///
    /// 键使用 nonce 的规范字节编码，调用方无论从 header 收到大写还是小写 hex，
    /// 都会落到同一条记录（否则大小写变体可绕过重放检测）。
    pub fn check_and_record(&mut self, device_id: &str, nonce: &[u8], now: i64) -> bool {
        self.prune(now);
        let key = format!("{device_id}:{}", hex::encode(nonce));
        if self.seen.contains_key(&key) {
            return false;
        }
        self.seen.insert(key, now.saturating_add(self.ttl_secs));
        true
    }

    pub fn prune(&mut self, now: i64) {
        self.seen.retain(|_, expires_at| *expires_at > now);
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta<'a>(
        method: &'a str,
        path: &'a str,
        body: &'a [u8],
        nonce: &'a [u8],
    ) -> RequestMeta<'a> {
        RequestMeta::new(method, path, body, nonce, 1_790_000_000)
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let secret = DeviceSecret::generate().expect("key");
        let nonce = [7u8; 16];
        let m = meta("PUT", "/api/notes/n1", b"{}", &nonce);
        let sig = secret.sign(&m);
        assert_eq!(sig.len(), 128);
        assert_eq!(
            verify(&secret.public_key(), &m, &sig, 1_790_000_000),
            Ok(())
        );
    }

    #[test]
    fn any_field_change_breaks_signature() {
        let secret = DeviceSecret::generate().expect("key");
        let nonce = [9u8; 16];
        let m = meta("PUT", "/api/notes/n1", b"{\"a\":1}", &nonce);
        let sig = secret.sign(&m);
        let now = 1_790_000_000;

        let other_method = meta("POST", "/api/notes/n1", b"{\"a\":1}", &nonce);
        let other_path = meta("PUT", "/api/notes/n2", b"{\"a\":1}", &nonce);
        let other_body = meta("PUT", "/api/notes/n1", b"{\"a\":2}", &nonce);
        let other_nonce = meta("PUT", "/api/notes/n1", b"{\"a\":1}", &[8u8; 16]);
        let mut shifted = m.clone();
        shifted.ts += 1;

        for (case, meta) in [
            ("method", other_method),
            ("path", other_path),
            ("body", other_body),
            ("nonce", other_nonce),
            ("ts", shifted),
        ] {
            assert_eq!(
                verify(&secret.public_key(), &meta, &sig, now),
                Err(AuthError::BadSignature),
                "改动 {case} 后必须验签失败"
            );
        }
    }

    #[test]
    fn clock_skew_is_enforced() {
        let secret = DeviceSecret::generate().expect("key");
        let nonce = [1u8; 16];
        let m = meta("GET", "/api/notes", b"", &nonce);
        let sig = secret.sign(&m);
        assert_eq!(
            verify(
                &secret.public_key(),
                &m,
                &sig,
                1_790_000_000 + MAX_SKEW_SECS
            ),
            Ok(())
        );
        assert_eq!(
            verify(
                &secret.public_key(),
                &m,
                &sig,
                1_790_000_001 + MAX_SKEW_SECS
            ),
            Err(AuthError::ClockSkew)
        );
        assert_eq!(
            verify(
                &secret.public_key(),
                &m,
                &sig,
                1_790_000_000 - MAX_SKEW_SECS - 1
            ),
            Err(AuthError::ClockSkew)
        );
    }

    #[test]
    fn wrong_public_key_fails() {
        let a = DeviceSecret::generate().expect("a");
        let b = DeviceSecret::generate().expect("b");
        let nonce = [3u8; 16];
        let m = meta("GET", "/api/notes", b"", &nonce);
        let sig = a.sign(&m);
        assert_eq!(
            verify(&b.public_key(), &m, &sig, 1_790_000_000),
            Err(AuthError::BadSignature)
        );
    }

    #[test]
    fn device_id_matches_public_key_hash() {
        let secret = DeviceSecret::generate().expect("key");
        let expected = hex::encode(&Sha256::digest(secret.public_key())[..16]);
        assert_eq!(secret.device_id(), expected);
        assert_eq!(secret.device_id().len(), 32);
    }

    #[test]
    fn nonce_replay_is_rejected_and_pruned() {
        let mut cache = ReplayCache::new(300);
        assert!(cache.check_and_record("dev", &[0xaa], 1000));
        assert!(!cache.check_and_record("dev", &[0xaa], 1001));
        // 不同设备同一 nonce 视为不同条目。
        assert!(cache.check_and_record("dev2", &[0xaa], 1001));
        assert_eq!(cache.len(), 2);

        // 超过 TTL 后旧条目被清理；再次记录同一 nonce 成功，且只剩这一条。
        assert!(cache.check_and_record("dev", &[0xaa], 1301));
        assert_eq!(cache.len(), 1);
        assert!(!cache.check_and_record("dev", &[0xaa], 1302));
    }

    #[test]
    fn nonce_case_variants_share_one_replay_entry() {
        // 服务端从 header 解析 hex，大小写不影响字节；缓存必须按字节规范化。
        let mut cache = ReplayCache::new(300);
        let lower = hex::decode("0a0b").unwrap();
        let upper = hex::decode("0A0B").unwrap();
        assert!(cache.check_and_record("dev", &lower, 1000));
        assert!(!cache.check_and_record("dev", &upper, 1001));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn nonce_length_is_validated() {
        assert_eq!(validate_nonce(&[0u8; 15]), Err(AuthError::BadNonce));
        assert_eq!(validate_nonce(&[0u8; 16]), Ok(()));
        assert_eq!(validate_nonce(&[0u8; 32]), Ok(()));
        assert_eq!(validate_nonce(&[0u8; 33]), Err(AuthError::BadNonce));
    }
}
