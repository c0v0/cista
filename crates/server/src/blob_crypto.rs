//! 共享 blob 的静态加密（`design.md` §2.6）：分块 AEAD，支持流式读写。
//!
//! 文件格式（v1）：
//! ```text
//! "CB1\0"                        魔数
//! 重复： nonce(24) ‖ ct_len(u32be) ‖ ciphertext(ct_len)
//! ```
//! 每块明文 ≤ 1 MiB（最后一块可短），AAD = `cista.blob.v1 ‖ u32be(块序号)`。
//! 内容寻址与去重按**明文**哈希；块级独立加密让上传可以边收边加密，不必整包进内存。
use cista_core::crypto::{open, seal, Key, Sealed, NONCE_LEN};
use std::io::{Read, Write};

use crate::error::AppError;

pub const MAGIC: &[u8; 4] = b"CB1\0";
const AAD_DOMAIN: &[u8] = b"cista.blob.v1";
const TAG_LEN: usize = 16;
/// 每块明文上限：解码时据此拒绝异常大块，避免恶意文件撑爆内存。
pub const CHUNK: usize = 1024 * 1024;
const MAX_CT: usize = CHUNK + TAG_LEN;

fn chunk_aad(index: u32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(AAD_DOMAIN.len() + 4);
    aad.extend_from_slice(AAD_DOMAIN);
    aad.extend_from_slice(&index.to_be_bytes());
    aad
}

/// 加密单个明文块并返回可直接写入文件的帧（流式上传用）。
pub fn seal_chunk(key: &[u8; 32], index: u32, plaintext: &[u8]) -> Result<Vec<u8>, AppError> {
    let key = Key::from_bytes(*key);
    let sealed = seal(&key, &chunk_aad(index), plaintext)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("shared blob encrypt failed")))?;
    let mut frame = Vec::with_capacity(NONCE_LEN + 4 + sealed.ct.len());
    write_frame(&mut frame, &sealed);
    Ok(frame)
}

/// 把明文整体加密为分块密文（调用方已在内存里时使用）。
pub fn seal_blob(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::with_capacity(plaintext.len() + MAGIC.len() + 8);
    out.extend_from_slice(MAGIC);
    for (index, chunk) in plaintext.chunks(CHUNK).enumerate() {
        let index = u32::try_from(index)
            .map_err(|_| AppError::Internal(anyhow::anyhow!("too many blob chunks")))?;
        out.extend_from_slice(&seal_chunk(key, index, chunk)?);
    }
    Ok(out)
}

fn write_frame(out: &mut Vec<u8>, sealed: &Sealed) {
    out.extend_from_slice(&sealed.nonce);
    out.extend_from_slice(&(u32::try_from(sealed.ct.len()).unwrap_or(u32::MAX)).to_be_bytes());
    out.extend_from_slice(&sealed.ct);
}

/// 解密整段分块密文。
pub fn open_blob(key: &[u8; 32], stored: &[u8]) -> Result<Vec<u8>, AppError> {
    if !stored.starts_with(MAGIC) {
        return Err(AppError::Internal(anyhow::anyhow!(
            "shared blob is not encrypted (wrong CISTA_STATIC_KEY or legacy plaintext)"
        )));
    }
    let key = Key::from_bytes(*key);
    let mut out = Vec::with_capacity(stored.len());
    open_frames(&key, &stored[MAGIC.len()..], &mut out)?;
    Ok(out)
}

fn open_frames(key: &Key, mut rest: &[u8], out: &mut Vec<u8>) -> Result<(), AppError> {
    let mut index: u32 = 0;
    while !rest.is_empty() {
        if rest.len() < NONCE_LEN + 4 {
            return Err(AppError::Internal(anyhow::anyhow!(
                "shared blob frame truncated"
            )));
        }
        let nonce = rest[..NONCE_LEN].to_vec();
        let ct_len = u32::from_be_bytes(
            rest[NONCE_LEN..NONCE_LEN + 4]
                .try_into()
                .unwrap_or_default(),
        );
        let ct_len = usize::try_from(ct_len)
            .map_err(|_| AppError::Internal(anyhow::anyhow!("bad blob chunk length")))?;
        if ct_len > MAX_CT {
            return Err(AppError::Internal(anyhow::anyhow!(
                "shared blob chunk too large"
            )));
        }
        let start = NONCE_LEN + 4;
        let end = start + ct_len;
        let Some(frame) = rest.get(start..end) else {
            return Err(AppError::Internal(anyhow::anyhow!(
                "shared blob frame truncated"
            )));
        };
        let plaintext = open(
            key,
            &chunk_aad(index),
            &Sealed {
                nonce,
                ct: frame.to_vec(),
            },
        )
        .map_err(|_| AppError::Internal(anyhow::anyhow!("shared blob decrypt failed")))?;
        out.extend_from_slice(&plaintext);
        rest = &rest[end..];
        index = index
            .checked_add(1)
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("too many blob chunks")))?;
    }
    Ok(())
}

/// 流式加密：从 `reader` 读、向 `writer` 写分块密文，返回明文字节数。
pub fn seal_stream<R: Read, W: Write>(
    key: &[u8; 32],
    reader: &mut R,
    writer: &mut W,
) -> Result<u64, AppError> {
    let key = Key::from_bytes(*key);
    writer
        .write_all(MAGIC)
        .map_err(|error| AppError::Internal(error.into()))?;
    let mut buffer = vec![0u8; CHUNK];
    let mut total: u64 = 0;
    let mut index: u32 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| AppError::Internal(error.into()))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        let sealed = seal(&key, &chunk_aad(index), &buffer[..read])
            .map_err(|_| AppError::Internal(anyhow::anyhow!("shared blob encrypt failed")))?;
        let mut frame = Vec::with_capacity(NONCE_LEN + 4 + sealed.ct.len());
        write_frame(&mut frame, &sealed);
        writer
            .write_all(&frame)
            .map_err(|error| AppError::Internal(error.into()))?;
        index = index
            .checked_add(1)
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("too many blob chunks")))?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_across_chunk_boundaries() {
        let key = [7u8; 32];
        for len in [1usize, CHUNK, CHUNK + 1, CHUNK * 2 + 123] {
            let plaintext = vec![0x5au8; len];
            let stored = seal_blob(&key, &plaintext).expect("seal");
            assert!(stored.starts_with(MAGIC));
            assert_ne!(stored.len(), plaintext.len(), "必须有 nonce/tag 开销");
            assert_eq!(open_blob(&key, &stored).expect("open"), plaintext);
        }
    }

    #[test]
    fn stream_roundtrip_matches_buffered() {
        let key = [3u8; 32];
        let plaintext = vec![0x11u8; CHUNK + 77];
        let mut sealed = Vec::new();
        let written = seal_stream(&key, &mut plaintext.as_slice(), &mut sealed).expect("stream");
        assert_eq!(written, plaintext.len() as u64);
        assert_eq!(open_blob(&key, &sealed).expect("open"), plaintext);
    }

    #[test]
    fn rejects_plaintext_and_tampering() {
        let key = [9u8; 32];
        assert!(open_blob(&key, b"plain text").is_err());
        let mut stored = seal_blob(&key, b"abc").expect("seal");
        let last = stored.len() - 1;
        stored[last] ^= 0x01;
        assert!(open_blob(&key, &stored).is_err());
        assert!(open_blob(&[2u8; 32], &stored).is_err());
    }
}
