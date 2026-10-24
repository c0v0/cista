//! 共享面协议：权限位、UI 预设、分享 token、文件名清洗、上传白名单（纯逻辑）。
//!
//! 规格：`docs/specs/phase2-sharing.md` §1–§4。
//! 存储里只用 bitmask；UI 的 `off/ro/rw/...` 只是映射。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 笔记：可读。
pub const NOTE_READ: u32 = 1 << 0;
/// 笔记：可写（Yjs update 不被丢弃）。
pub const NOTE_WRITE: u32 = 1 << 1;
/// 文件：可列目录/下载/预览。
pub const FILE_READ: u32 = 1 << 2;
/// 文件：可上传（依赖 `FILE_READ` 以外的上传位）。
pub const FILE_UPLOAD: u32 = 1 << 3;
/// 文件：可改名/删除（依赖 `FILE_READ`）。
pub const FILE_MANAGE: u32 = 1 << 4;
/// `drop` 链接：只上传、不列目录（与 `FILE_READ` 互斥）。
pub const FILE_DROP_UPLOAD_ONLY: u32 = 1 << 5;

/// 链接类型：单文件 / 工作区。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareKind {
    File,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteMode {
    Off,
    Ro,
    Rw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesMode {
    Off,
    Ro,
    Rw,
    Rwm,
    Drop,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScopeError {
    #[error("note=rw 需要 note 读权限")]
    NoteWriteWithoutRead,
    #[error("files 管理权限需要读权限")]
    ManageWithoutRead,
    #[error("files 预设组合非法")]
    InvalidPreset,
}

pub fn note_scopes(mode: NoteMode) -> u32 {
    match mode {
        NoteMode::Off => 0,
        NoteMode::Ro => NOTE_READ,
        NoteMode::Rw => NOTE_READ | NOTE_WRITE,
    }
}

pub fn file_scopes(mode: FilesMode) -> u32 {
    match mode {
        FilesMode::Off => 0,
        FilesMode::Ro => FILE_READ,
        FilesMode::Rw => FILE_READ | FILE_UPLOAD,
        FilesMode::Rwm => FILE_READ | FILE_UPLOAD | FILE_MANAGE,
        FilesMode::Drop => FILE_DROP_UPLOAD_ONLY,
    }
}

/// 校验 bitmask 组合（服务端落库前与每次请求都要成立）。
pub fn validate_scopes(note_scopes: u32, file_scopes: u32) -> Result<(), ScopeError> {
    let note_bits = NOTE_READ | NOTE_WRITE;
    let file_bits = FILE_READ | FILE_UPLOAD | FILE_MANAGE | FILE_DROP_UPLOAD_ONLY;
    if note_scopes & !note_bits != 0 || file_scopes & !file_bits != 0 {
        return Err(ScopeError::InvalidPreset);
    }
    if note_scopes & NOTE_WRITE != 0 && note_scopes & NOTE_READ == 0 {
        return Err(ScopeError::NoteWriteWithoutRead);
    }
    if file_scopes & FILE_MANAGE != 0 && file_scopes & FILE_READ == 0 {
        return Err(ScopeError::ManageWithoutRead);
    }
    if file_scopes & FILE_DROP_UPLOAD_ONLY != 0 && file_scopes & FILE_READ != 0 {
        return Err(ScopeError::InvalidPreset);
    }
    Ok(())
}

pub fn can_read_notes(scopes: u32) -> bool {
    scopes & NOTE_READ != 0
}

pub fn can_write_notes(scopes: u32) -> bool {
    scopes & NOTE_WRITE != 0
}

pub fn can_read_files(scopes: u32) -> bool {
    scopes & FILE_READ != 0
}

pub fn can_upload(scopes: u32) -> bool {
    scopes & (FILE_UPLOAD | FILE_DROP_UPLOAD_ONLY) != 0
}

pub fn can_manage_files(scopes: u32) -> bool {
    scopes & FILE_MANAGE != 0
}

/// 128-bit 分享 token（base64url 无填充，22 字符），客户端生成。
pub const TOKEN_BYTES: usize = 16;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenError {
    #[error("随机数源不可用")]
    Random,
    #[error("token 长度或字符集非法")]
    Invalid,
}

pub fn new_share_token() -> Result<String, TokenError> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| TokenError::Random)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// URL 安全的 token 字符集（16–64 字符）——fragment 解析与服务端校验共用。
pub fn is_valid_token_shape(token: &str) -> bool {
    (16..=64).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 服务端只存 `SHA-256(token)`；日志只允许出现前 8 位。
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// 日志/诊断用的脱敏表示：`sha256:xxxxxxxx…`（绝不输出明文 token）。
pub fn token_fingerprint(token: &str) -> String {
    format!("sha256:{}", &hash_token(token)[..8])
}

/// 文件名清洗错误。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NameError {
    #[error("文件名为空")]
    Empty,
    #[error("文件名过长")]
    TooLong,
    #[error("文件类型不允许")]
    ForbiddenExtension,
}

/// 清洗上传文件名：只保留 basename，去掉控制字符，替换 Windows 禁用字符，限制长度。
///
/// `../../etc/passwd` → `passwd`；`a<b>.txt` → `a_b_.txt`。
pub fn sanitize_file_name(raw: &str) -> Result<String, NameError> {
    let base = raw
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("");
    let mut cleaned = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_control() {
            continue;
        }
        cleaned.push(match ch {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            other => other,
        });
    }
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return Err(NameError::Empty);
    }
    if cleaned.chars().count() > 100 {
        return Err(NameError::TooLong);
    }
    if forbidden_extension(&cleaned) {
        return Err(NameError::ForbiddenExtension);
    }
    Ok(cleaned)
}

/// 禁止的扩展名（可执行 / HTML / SVG / 脚本按 `design.md` §3.7 一律拒绝）。
const FORBIDDEN_EXT: &[&str] = &[
    "html", "htm", "xhtml", "shtml", "svg", "svgz", "js", "mjs", "cjs", "jsx", "ts", "tsx", "wasm",
    "exe", "dll", "so", "dylib", "msi", "bat", "cmd", "com", "ps1", "psm1", "sh", "bash", "zsh",
    "fish", "py", "pl", "rb", "php", "jar", "app", "apk", "deb", "rpm", "scr", "vbs", "hta",
];

pub fn forbidden_extension(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((_, ext)) => {
            let ext = ext.to_ascii_lowercase();
            FORBIDDEN_EXT.contains(&ext.as_str())
        }
        None => false,
    }
}

/// 允许上传的 MIME 前缀/精确值（另加扩展名双判，见 `sanitize_file_name`）。
const ALLOWED_MIME_EXACT: &[&str] = &[
    "application/pdf",
    "application/zip",
    "application/x-zip-compressed",
    "text/plain",
    "text/markdown",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-powerpoint",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.oasis.opendocument.text",
    "application/vnd.oasis.opendocument.spreadsheet",
    "application/vnd.oasis.opendocument.presentation",
];

const ALLOWED_MIME_PREFIX: &[&str] = &["image/", "audio/", "video/"];

pub fn is_allowed_mime(mime: &str) -> bool {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if mime.is_empty() {
        return false;
    }
    if mime == "image/svg+xml" || mime == "text/html" || mime.ends_with("+xml") {
        return false;
    }
    if ALLOWED_MIME_EXACT.contains(&mime.as_str()) {
        return true;
    }
    ALLOWED_MIME_PREFIX
        .iter()
        .any(|prefix| mime.starts_with(prefix))
}

/// 下载响应头：永不内联（`Content-Disposition: attachment` + `nosniff`）。
pub const DOWNLOAD_DISPOSITION: &str = "attachment";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_map_to_bits() {
        assert_eq!(note_scopes(NoteMode::Off), 0);
        assert_eq!(note_scopes(NoteMode::Ro), NOTE_READ);
        assert_eq!(note_scopes(NoteMode::Rw), NOTE_READ | NOTE_WRITE);
        assert_eq!(file_scopes(FilesMode::Ro), FILE_READ);
        assert_eq!(file_scopes(FilesMode::Rw), FILE_READ | FILE_UPLOAD);
        assert_eq!(
            file_scopes(FilesMode::Rwm),
            FILE_READ | FILE_UPLOAD | FILE_MANAGE
        );
        assert_eq!(file_scopes(FilesMode::Drop), FILE_DROP_UPLOAD_ONLY);
    }

    #[test]
    fn scope_dependencies_are_enforced() {
        assert_eq!(
            validate_scopes(NOTE_WRITE, 0),
            Err(ScopeError::NoteWriteWithoutRead)
        );
        assert_eq!(
            validate_scopes(0, FILE_MANAGE),
            Err(ScopeError::ManageWithoutRead)
        );
        assert_eq!(
            validate_scopes(0, FILE_DROP_UPLOAD_ONLY | FILE_READ),
            Err(ScopeError::InvalidPreset)
        );
        assert_eq!(validate_scopes(0, 1 << 9), Err(ScopeError::InvalidPreset));
        assert_eq!(validate_scopes(NOTE_READ | NOTE_WRITE, FILE_READ), Ok(()));
    }

    #[test]
    fn permissions_answer_questions() {
        let ro_files = file_scopes(FilesMode::Ro);
        assert!(can_read_files(ro_files));
        assert!(!can_upload(ro_files));
        assert!(!can_manage_files(ro_files));

        let drop = file_scopes(FilesMode::Drop);
        assert!(can_upload(drop));
        assert!(!can_read_files(drop));
        assert!(!can_manage_files(drop));

        let rwm = file_scopes(FilesMode::Rwm);
        assert!(can_upload(rwm) && can_manage_files(rwm) && can_read_files(rwm));
    }

    #[test]
    fn token_is_128_bit_and_hashes() {
        let token = new_share_token().expect("token");
        assert_eq!(token.len(), 22);
        assert!(is_valid_token_shape(&token));
        assert_ne!(token, new_share_token().expect("token2"));

        let hash = hash_token(&token);
        assert_eq!(hash.len(), 64);
        assert_eq!(hash, hash_token(&token));
        assert_ne!(hash, hash_token(&new_share_token().expect("other")));
        assert!(token_fingerprint(&token).starts_with("sha256:"));
        assert!(!token_fingerprint(&token).contains(&token[..8]));
    }

    #[test]
    fn token_shape_rejects_bad_input() {
        assert!(!is_valid_token_shape("short"));
        assert!(!is_valid_token_shape("aaaaaaaaaaaaaaaa+="));
        assert!(!is_valid_token_shape(""));
        assert!(is_valid_token_shape("0123456789abcdef"));
    }

    #[test]
    fn file_names_are_sanitized() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), Ok("passwd".into()));
        assert_eq!(
            sanitize_file_name("C:\\Users\\x\\报价.pdf"),
            Ok("报价.pdf".into())
        );
        assert_eq!(sanitize_file_name("a<b>:c?.txt"), Ok("a_b__c_.txt".into()));
        assert_eq!(sanitize_file_name("  报告.docx  "), Ok("报告.docx".into()));
        assert_eq!(sanitize_file_name(".."), Err(NameError::Empty));
        assert_eq!(sanitize_file_name(""), Err(NameError::Empty));
        assert_eq!(
            sanitize_file_name(&"x".repeat(101)),
            Err(NameError::TooLong)
        );
        assert_eq!(
            sanitize_file_name("evil.html"),
            Err(NameError::ForbiddenExtension)
        );
        assert_eq!(
            sanitize_file_name("EVIL.SVG"),
            Err(NameError::ForbiddenExtension)
        );
        assert_eq!(sanitize_file_name("photo.PNG"), Ok("photo.PNG".into()));
    }

    #[test]
    fn mime_whitelist_blocks_active_content() {
        assert!(is_allowed_mime("image/png"));
        assert!(is_allowed_mime("application/pdf"));
        assert!(is_allowed_mime("video/mp4"));
        assert!(is_allowed_mime("text/plain; charset=utf-8"));
        assert!(!is_allowed_mime("text/html"));
        assert!(!is_allowed_mime("image/svg+xml"));
        assert!(!is_allowed_mime("application/javascript"));
        assert!(!is_allowed_mime("application/octet-stream"));
        assert!(!is_allowed_mime(""));
    }
}
