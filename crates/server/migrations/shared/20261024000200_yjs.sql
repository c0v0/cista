-- Phase 3：Yjs 文档持久化（ADR 0006）。
-- 快照在 workspace_notes；其后是尾部 update；快照成功后清理已合并部分。
-- 注意：列名 `update` 是 SQL 关键字，必须加双引号。

CREATE TABLE workspace_notes (
  workspace_id  TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  rev           INTEGER NOT NULL DEFAULT 0,
  yjs_state     BLOB,
  state_vector  BLOB,
  updated_at    INTEGER NOT NULL
);

CREATE TABLE doc_updates (
  seq           INTEGER PRIMARY KEY AUTOINCREMENT,
  workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  "update"      BLOB NOT NULL,
  created_at    INTEGER NOT NULL
);

CREATE INDEX idx_updates_ws ON doc_updates(workspace_id, seq);
