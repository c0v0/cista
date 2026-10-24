//! 客户端本地文件索引的纯逻辑（无 IO）。
//!
//! 规格：`docs/specs/phase4-clients.md` §1。桌面/手机用 SQLite 实现同样的语义，
//! Web 临时端直接用本模块的内存实现；索引不同步、不上传、只存文件名/标题/ID。

use serde::{Deserialize, Serialize};

/// 附件（v1 唯一会写入的类型）。
pub const SOURCE_NOTE_ATTACHMENT: &str = "note_attachment";
/// 私有网盘节点（预留；v1 不写入）。
pub const SOURCE_DRIVE_NODE: &str = "drive_node";

/// 默认/最大返回条数（与 `design.md` §8 的分页上限一致）。
pub const SEARCH_DEFAULT_LIMIT: usize = 50;
pub const SEARCH_MAX_LIMIT: usize = 200;

/// 索引行：只存文件名/标题/ID，不存正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIndexRow {
    pub source_type: String,
    pub source_id: String,
    pub note_id: Option<String>,
    pub name: String,
    pub note_title: Option<String>,
    pub updated_at: i64,
}

/// SQL `LIKE` 转义（配合 `ESCAPE '\'`）：`\` → `\\`，`%` → `\%`，`_` → `\_`。
pub fn escape_like(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' | '%' | '_' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// 与 SQL `LIKE '%q%' ESCAPE '\'` 等价的子串匹配（大小写不敏感）。
pub fn like_contains(haystack: &str, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    haystack.to_lowercase().contains(&query.to_lowercase())
}

/// 分页上限：默认 50，硬上限 200。
pub fn clamp_limit(limit: usize) -> usize {
    if limit == 0 {
        SEARCH_DEFAULT_LIMIT
    } else {
        limit.min(SEARCH_MAX_LIMIT)
    }
}

/// 内存索引（Web 临时端 / 测试 / 全量重建的中间态）。
#[derive(Debug, Default, Clone)]
pub struct LocalFileIndex {
    rows: Vec<FileIndexRow>,
}

impl LocalFileIndex {
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }

    /// 插入或按 `(source_type, source_id)` 覆盖。
    pub fn upsert(&mut self, row: FileIndexRow) {
        if let Some(existing) = self.rows.iter_mut().find(|existing| {
            existing.source_type == row.source_type && existing.source_id == row.source_id
        }) {
            *existing = row;
        } else {
            self.rows.push(row);
        }
    }

    /// 删除单个索引项（附件被删/改名走 upsert）。
    pub fn remove(&mut self, source_type: &str, source_id: &str) {
        self.rows
            .retain(|row| !(row.source_type == source_type && row.source_id == source_id));
    }

    /// 笔记删除：级联移除该笔记下的所有索引项（避免死链）。
    pub fn remove_note(&mut self, note_id: &str) {
        self.rows
            .retain(|row| row.note_id.as_deref() != Some(note_id));
    }

    /// 全量重建（索引可丢可重建，从笔记/附件表重新灌入）。
    pub fn rebuild(&mut self, rows: Vec<FileIndexRow>) {
        self.rows = rows;
    }

    /// 子串搜索：空查询返回空；按 `updated_at` 新的在前、同名按 `name` 升序；受分页上限约束。
    pub fn search(&self, query: &str, limit: usize) -> Vec<&FileIndexRow> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<&FileIndexRow> = self
            .rows
            .iter()
            .filter(|row| like_contains(&row.name, query))
            .collect();
        hits.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        hits.truncate(clamp_limit(limit));
        hits
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(
        source_id: &str,
        note_id: Option<&str>,
        name: &str,
        updated_at: i64,
    ) -> FileIndexRow {
        FileIndexRow {
            source_type: SOURCE_NOTE_ATTACHMENT.into(),
            source_id: source_id.into(),
            note_id: note_id.map(str::to_string),
            name: name.into(),
            note_title: note_id.map(|_| "周报".to_string()),
            updated_at,
        }
    }

    fn drive(source_id: &str, name: &str, updated_at: i64) -> FileIndexRow {
        FileIndexRow {
            source_type: SOURCE_DRIVE_NODE.into(),
            source_id: source_id.into(),
            note_id: None,
            name: name.into(),
            note_title: None,
            updated_at,
        }
    }

    #[test]
    fn like_escaping_covers_wildcards_and_backslash() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("c:\\tmp"), "c:\\\\tmp");
        assert_eq!(escape_like("正常"), "正常");
    }

    #[test]
    fn matching_is_literal_and_case_insensitive() {
        assert!(like_contains("100%_draft.pdf", "100%_"));
        // `%`/`_` 不做通配：用普通字符查询不会误命中。
        assert!(!like_contains("abc.pdf", "%"));
        assert!(like_contains("Report.PDF", "report"));
        assert!(!like_contains("abc", ""));
    }

    #[test]
    fn search_is_scoped_ordered_and_paged() {
        let mut index = LocalFileIndex::new();
        index.upsert(attachment("a1", Some("n1"), "报价单.pdf", 300));
        index.upsert(attachment("a2", Some("n1"), "报价单-v2.pdf", 200));
        index.upsert(attachment("a3", Some("n2"), "会议记录.md", 400));

        let hits = index.search("报价", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].source_id, "a1", "updated_at 新的在前");
        assert_eq!(hits[1].source_id, "a2");

        assert!(index.search("   ", 10).is_empty());
        assert_eq!(index.search("报价", 1).len(), 1, "limit 生效");
    }

    #[test]
    fn limit_is_clamped_to_max() {
        assert_eq!(clamp_limit(0), SEARCH_DEFAULT_LIMIT);
        assert_eq!(clamp_limit(10), 10);
        assert_eq!(clamp_limit(10_000), SEARCH_MAX_LIMIT);
    }

    #[test]
    fn upsert_remove_and_note_cascade() {
        let mut index = LocalFileIndex::new();
        index.upsert(attachment("a1", Some("n1"), "旧名.pdf", 1));
        index.upsert(attachment("a1", Some("n1"), "新名.pdf", 2));
        assert_eq!(index.len(), 1);
        assert_eq!(index.search("新名", 10)[0].name, "新名.pdf");

        index.upsert(attachment("a2", Some("n1"), "附件2.pdf", 3));
        index.remove_note("n1");
        assert!(index.is_empty(), "笔记删除后不能留下死链");

        // 同 source_id 但不同 source_type：remove 只影响指定类型。
        index.upsert(attachment("x1", Some("n2"), "同名.pdf", 4));
        index.upsert(drive("x1", "同名.pdf", 5));
        index.remove(SOURCE_NOTE_ATTACHMENT, "x1");
        assert_eq!(index.len(), 1);
        assert_eq!(index.search("同名", 10)[0].source_type, SOURCE_DRIVE_NODE);
    }

    #[test]
    fn rebuild_replaces_everything() {
        let mut index = LocalFileIndex::new();
        index.upsert(attachment("a1", Some("n1"), "x.pdf", 1));
        index.rebuild(vec![attachment("b1", Some("n2"), "y.pdf", 2)]);
        assert_eq!(index.len(), 1);
        assert_eq!(index.search("y", 10)[0].source_id, "b1");
    }

    #[test]
    fn drive_node_rows_have_no_note_binding() {
        let mut index = LocalFileIndex::new();
        index.upsert(drive("drive-1", "归档.zip", 5));
        let hits = index.search("归档", 10);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].note_id.is_none());
    }
}
