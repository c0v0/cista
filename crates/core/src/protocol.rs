//! 客户端/服务端共享的协议类型（serde）。
//!
//! 服务端只做「id + rev + 密文信封」的存取与校验，不理解私有明文。

use serde::{Deserialize, Serialize};

use crate::crypto::SealedNote;
use crate::share::{FilesMode, NoteMode, ShareKind};

/// 私有笔记记录：服务端存储与增量同步的单位。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteRecord {
    pub id: String,
    pub rev: u64,
    pub updated_at: i64,
    pub sealed: SealedNote,
}

/// `GET /api/notes?since=` 的响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotesPage {
    pub notes: Vec<NoteRecord>,
    /// 下一页的 `since`（取本页最后一条 seq；无数据时回显请求值）。
    pub next_since: u64,
}

/// `PUT /api/notes/{id}` 请求体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutNoteRequest {
    pub sealed: SealedNote,
}

/// 设备入网请求：`Authorization: Bootstrap <token>` 或已有设备的 B+ 签名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollRequest {
    pub name: String,
    /// Ed25519 公钥的 hex（64 字符）。
    pub pubkey: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollResponse {
    pub device_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
}

/// 内容寻址的 blob 引用（密文；服务端按 sha256 去重）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    pub hash: String,
    pub size: u64,
}

/// 统一错误体：不回显内部细节（不泄漏对象是否存在）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

impl ApiError {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
        }
    }
}

/// 分页上限（`design.md` §8：默认 50、最大 200；`since` 单次最多 500）。
pub const PAGE_DEFAULT: u64 = 50;
pub const PAGE_MAX: u64 = 200;
pub const SINCE_BATCH_MAX: u64 = 500;

// ---------------------------------------------------------------------------
// 共享面 DTO
// ---------------------------------------------------------------------------

/// 工作区来源：空白 / 编辑分叉 / 只读投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceOrigin {
    Blank,
    Fork,
    Projection,
}

impl WorkspaceOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blank => "blank",
            Self::Fork => "fork",
            Self::Projection => "projection",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "blank" => Some(Self::Blank),
            "fork" => Some(Self::Fork),
            "projection" => Some(Self::Projection),
            _ => None,
        }
    }
}

/// 工作区状态机（`design.md` §6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceState {
    Active,
    Expired,
    Quarantined,
    Archived,
    Purged,
}

impl WorkspaceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Quarantined => "quarantined",
            Self::Archived => "archived",
            Self::Purged => "purged",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "active" => Some(Self::Active),
            "expired" => Some(Self::Expired),
            "quarantined" => Some(Self::Quarantined),
            "archived" => Some(Self::Archived),
            "purged" => Some(Self::Purged),
            _ => None,
        }
    }

    /// 公网路由是否可达（只有 `active` 可达）。
    pub fn is_public(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkspaceRequest {
    pub title: String,
    pub origin: WorkspaceOrigin,
    /// `fork`/`projection` 的来源私有笔记（溯源用，弱引用）。
    pub source_note_id: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub title: String,
    pub origin: WorkspaceOrigin,
    pub source_note_id: Option<String>,
    pub state: WorkspaceState,
    pub expires_at: Option<i64>,
    pub quarantined_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateLinkRequest {
    pub kind: ShareKind,
    /// `file`：私有 blob hash；`workspace`：工作区 id。
    pub object_id: String,
    /// 客户端生成的 128-bit token（服务端只存 SHA-256）。
    pub token: String,
    /// 单文件链接的展示名（工作区链接用工作区标题）。
    #[serde(default)]
    pub title: Option<String>,
    pub note_mode: NoteMode,
    pub files_mode: FilesMode,
    pub expires_at: Option<i64>,
    pub max_uses: Option<u32>,
    /// `drop` 默认要求口令；为空表示不设。
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkInfo {
    pub id: String,
    pub kind: ShareKind,
    pub object_id: String,
    pub note_scopes: u32,
    pub file_scopes: u32,
    pub expires_at: Option<i64>,
    pub max_uses: Option<u32>,
    pub used_count: u32,
    pub revoked_at: Option<i64>,
    pub created_at: i64,
}

/// 分享落地页信息（`GET /public/*/meta`，不计数）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicMeta {
    pub kind: ShareKind,
    pub title: String,
    pub note_scopes: u32,
    pub file_scopes: u32,
    pub expires_at: Option<i64>,
    pub requires_passphrase: bool,
    pub max_uses: Option<u32>,
    pub used_count: u32,
}

/// `confirm` 返回的一次性下载票据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmResponse {
    pub ticket: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicFileEntry {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub rev: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicFileList {
    pub files: Vec<PublicFileEntry>,
    /// 下一页的 `offset`；`None` 表示已到末尾（`PAGE_DEFAULT`/`PAGE_MAX` 见上）。
    #[serde(default)]
    pub next_offset: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{seal_note, Key, NotePlaintext};

    #[test]
    fn note_record_json_roundtrip() {
        let mk = Key::generate().expect("mk");
        let ck = Key::generate().expect("ck");
        let sealed = seal_note(
            &mk,
            "n1",
            1,
            &ck,
            &NotePlaintext {
                title: "t".into(),
                markdown: "m".into(),
                attachments: Vec::new(),
            },
        )
        .expect("seal");
        let record = NoteRecord {
            id: "n1".into(),
            rev: 1,
            updated_at: 1_790_000_000,
            sealed,
        };
        let json = serde_json::to_string(&record).expect("json");
        let back: NoteRecord = serde_json::from_str(&json).expect("parse");
        assert_eq!(back, record);
        // 线格式里没有明文标记。
        assert!(!json.contains("\"markdown\""));
    }

    #[test]
    fn workspace_state_parse_roundtrip() {
        for state in [
            WorkspaceState::Active,
            WorkspaceState::Expired,
            WorkspaceState::Quarantined,
            WorkspaceState::Archived,
            WorkspaceState::Purged,
        ] {
            assert_eq!(WorkspaceState::parse(state.as_str()), Some(state));
        }
        assert_eq!(WorkspaceState::parse("nope"), None);
        assert!(WorkspaceState::Active.is_public());
        assert!(!WorkspaceState::Quarantined.is_public());
    }

    #[test]
    fn create_link_request_tolerates_absent_title() {
        let json = r#"{
            "kind":"file",
            "object_id":"aa",
            "token":"0123456789abcdef",
            "note_mode":"off",
            "files_mode":"ro",
            "expires_at":null,
            "max_uses":null,
            "passphrase":null
        }"#;
        let request: CreateLinkRequest = serde_json::from_str(json).expect("parse");
        assert!(request.title.is_none());
    }
}
