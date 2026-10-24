//! 私有内容的对称加密、密钥包裹与恢复码（v1 冻结）。
//!
//! 规格：`docs/specs/phase1-core-auth.md` §1。
//!
//! - AEAD：`XChaCha20-Poly1305`，24B 随机 nonce；每次加密都新生成，不复用计数器。
//! - AAD 用长度前缀拼接（`domain NUL (len:u32be ‖ bytes)*`），避免分隔符歧义。
//! - MK 只在本模块内使用；服务端永远看不到它。

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const NOTE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("随机数源不可用")]
    Random,
    #[error("密钥长度必须为 32 字节")]
    KeyLength,
    #[error("nonce 长度必须为 24 字节")]
    NonceLength,
    #[error("密文版本 {0} 不支持")]
    UnsupportedVersion(u32),
    #[error("解密失败：密文被篡改或 AAD 不匹配")]
    Decrypt,
    #[error("明文格式非法")]
    Plaintext,
}

/// 32 字节对称密钥（MK，或每对象的 CK/FK）。离开作用域时清零。
#[derive(Clone)]
pub struct Key(Zeroizing<[u8; KEY_LEN]>);

impl Key {
    /// 用 CSPRNG 生成新密钥（MK / CK / FK）。
    pub fn generate() -> Result<Self, CryptoError> {
        let mut bytes = [0u8; KEY_LEN];
        getrandom::fill(&mut bytes).map_err(|_| CryptoError::Random)?;
        Ok(Self(Zeroizing::new(bytes)))
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self, CryptoError> {
        let arr: [u8; KEY_LEN] = bytes.try_into().map_err(|_| CryptoError::KeyLength)?;
        Ok(Self::from_bytes(arr))
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// 恢复码：32B 的 base64url（无填充）。离线保存，别和服务端/备份放一起。
    pub fn recovery_code(&self) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        URL_SAFE_NO_PAD.encode(self.0.as_ref())
    }

    pub fn from_recovery_code(code: &str) -> Result<Self, CryptoError> {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let bytes = URL_SAFE_NO_PAD
            .decode(code.trim().as_bytes())
            .map_err(|_| CryptoError::KeyLength)?;
        Self::from_slice(&bytes)
    }
}

/// 加密后的 blob：`nonce ‖ ciphertext`（JSON 里都是 base64）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    #[serde(with = "b64_vec")]
    pub nonce: Vec<u8>,
    #[serde(with = "b64_vec")]
    pub ct: Vec<u8>,
}

/// 私有笔记明文信封（只存在于客户端）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotePlaintext {
    pub title: String,
    pub markdown: String,
}

/// 私有笔记的密文信封：CK 被 MK 包裹，正文被 CK 加密。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedNote {
    pub v: u32,
    pub rev: u64,
    pub wrap: Sealed,
    pub body: Sealed,
}

/// `domain NUL (len:u32be ‖ part)*`：无歧义的 AAD 拼接。
fn aad(domain: &str, parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(domain.len() + 1 + parts.len() * 8);
    out.extend_from_slice(domain.as_bytes());
    out.push(0);
    for part in parts {
        let len = u32::try_from(part.len()).unwrap_or(u32::MAX);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// 包裹 CK 用的 AAD：绑定笔记 ID（CK 挪到别的笔记就解不开）。
pub fn ck_aad(note_id: &str) -> Vec<u8> {
    aad("cista.ck.v1", &[note_id.as_bytes()])
}

/// 正文用的 AAD：绑定笔记 ID 与 rev（旧 rev 重放、跨对象搬运都解不开）。
pub fn note_aad(note_id: &str, rev: u64) -> Vec<u8> {
    aad("cista.note.v1", &[note_id.as_bytes(), &rev.to_be_bytes()])
}

/// 用给定密钥加密明文。
pub fn seal(key: &Key, aad: &[u8], plaintext: &[u8]) -> Result<Sealed, CryptoError> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|_| CryptoError::Random)?;
    seal_with_nonce(key, aad, plaintext, &nonce)
}

/// 用**指定** nonce 加密：只用于测试向量与跨语言 KAT。
///
/// 生产路径必须走 [`seal`]（每次新随机 nonce）；同 key 复用 nonce 会毁掉 AEAD 安全性。
pub fn seal_with_nonce(
    key: &Key,
    aad: &[u8],
    plaintext: &[u8],
    nonce: &[u8; NONCE_LEN],
) -> Result<Sealed, CryptoError> {
    let cipher =
        XChaCha20Poly1305::new_from_slice(key.as_bytes()).map_err(|_| CryptoError::KeyLength)?;
    let ct = cipher
        .encrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    Ok(Sealed {
        nonce: nonce.to_vec(),
        ct,
    })
}

/// 解密；AAD 不匹配或密文被篡改都返回 `Decrypt`。
pub fn open(key: &Key, aad: &[u8], sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let nonce: [u8; NONCE_LEN] = sealed
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::NonceLength)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(key.as_bytes()).map_err(|_| CryptoError::KeyLength)?;
    let plaintext = cipher
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: sealed.ct.as_slice(),
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)?;
    Ok(Zeroizing::new(plaintext))
}

/// 用 MK 包裹 CK 并加密正文；返回可上行的信封。
pub fn seal_note(
    mk: &Key,
    note_id: &str,
    rev: u64,
    ck: &Key,
    note: &NotePlaintext,
) -> Result<SealedNote, CryptoError> {
    let plaintext = serde_json::to_vec(note).map_err(|_| CryptoError::Plaintext)?;
    let wrap = seal(mk, &ck_aad(note_id), ck.as_bytes())?;
    let body = seal(ck, &note_aad(note_id, rev), plaintext.as_slice())?;
    Ok(SealedNote {
        v: NOTE_VERSION,
        rev,
        wrap,
        body,
    })
}

/// 用 MK 解包并解密；`sealed.rev` 必须与加密时的 rev 一致。
pub fn open_note(
    mk: &Key,
    note_id: &str,
    sealed: &SealedNote,
) -> Result<NotePlaintext, CryptoError> {
    if sealed.v != NOTE_VERSION {
        return Err(CryptoError::UnsupportedVersion(sealed.v));
    }
    let ck_bytes = open(mk, &ck_aad(note_id), &sealed.wrap)?;
    let ck = Key::from_slice(ck_bytes.as_slice())?;
    let plaintext = open(&ck, &note_aad(note_id, sealed.rev), &sealed.body)?;
    serde_json::from_slice(plaintext.as_slice()).map_err(|_| CryptoError::Plaintext)
}

mod b64_vec {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        STANDARD
            .decode(encoded.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str) -> NotePlaintext {
        NotePlaintext {
            title: title.into(),
            markdown: "# 私有\n服务端不可读".into(),
        }
    }

    #[test]
    fn seal_note_roundtrip() {
        let mk = Key::generate().expect("mk");
        let ck = Key::generate().expect("ck");
        let plain = note("周报");
        let sealed = seal_note(&mk, "note-1", 3, &ck, &plain).expect("seal");
        let opened = open_note(&mk, "note-1", &sealed).expect("open");
        assert_eq!(opened, plain);
        assert_eq!(sealed.v, NOTE_VERSION);
        assert_eq!(sealed.rev, 3);
    }

    #[test]
    fn aad_binds_note_id_and_rev() {
        let mk = Key::generate().expect("mk");
        let ck = Key::generate().expect("ck");
        let sealed = seal_note(&mk, "note-1", 1, &ck, &note("a")).expect("seal");

        // 换对象 ID → 包裹层解不开。
        assert_eq!(
            open_note(&mk, "note-2", &sealed).expect_err("wrong id"),
            CryptoError::Decrypt
        );

        // 换 rev → 正文层解不开。
        let mut bumped = sealed.clone();
        bumped.rev = 2;
        assert_eq!(
            open_note(&mk, "note-1", &bumped).expect_err("wrong rev"),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let mk = Key::generate().expect("mk");
        let ck = Key::generate().expect("ck");
        let mut sealed = seal_note(&mk, "n", 1, &ck, &note("x")).expect("seal");
        let last = sealed.body.ct.len() - 1;
        sealed.body.ct[last] ^= 0x01;
        assert_eq!(
            open_note(&mk, "n", &sealed).expect_err("tamper"),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn wrong_key_cannot_open() {
        let mk = Key::generate().expect("mk");
        let other = Key::generate().expect("other");
        let ck = Key::generate().expect("ck");
        let sealed = seal_note(&mk, "n", 1, &ck, &note("x")).expect("seal");
        assert_eq!(
            open_note(&other, "n", &sealed).expect_err("wrong mk"),
            CryptoError::Decrypt
        );
    }

    #[test]
    fn nonce_is_never_reused() {
        let key = Key::generate().expect("key");
        let a = seal(&key, b"aad", b"same plaintext").expect("a");
        let b = seal(&key, b"aad", b"same plaintext").expect("b");
        assert_ne!(a.nonce, b.nonce, "每次加密必须新 nonce");
        assert_ne!(a.ct, b.ct);
        assert_eq!(a.nonce.len(), NONCE_LEN);
    }

    #[test]
    fn unsupported_version_is_reported() {
        let mk = Key::generate().expect("mk");
        let ck = Key::generate().expect("ck");
        let mut sealed = seal_note(&mk, "n", 1, &ck, &note("x")).expect("seal");
        sealed.v = 99;
        assert_eq!(
            open_note(&mk, "n", &sealed).expect_err("version"),
            CryptoError::UnsupportedVersion(99)
        );
    }

    #[test]
    fn recovery_code_roundtrip() {
        let mk = Key::generate().expect("mk");
        let code = mk.recovery_code();
        assert_eq!(code.len(), 43, "32B base64url 无填充");
        let restored = Key::from_recovery_code(&code).expect("restore");
        assert_eq!(restored.as_bytes(), mk.as_bytes());
        assert!(Key::from_recovery_code("not base64!!").is_err());
    }
}
