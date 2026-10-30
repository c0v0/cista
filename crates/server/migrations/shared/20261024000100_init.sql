-- 共享库（shared.db）：工作区、文件区、分享链接、上传、票据、共享 blob 元数据。
-- 私有表不存在于此库；分享路由只拿这个库的连接池（ADR 0001）。

CREATE TABLE workspaces (
  id              TEXT PRIMARY KEY,
  title           TEXT NOT NULL,
  origin          TEXT NOT NULL CHECK (origin IN ('blank','fork','projection')),
  source_note_id  TEXT,
  state           TEXT NOT NULL CHECK (state IN ('active','expired','quarantined','archived','purged')),
  expires_at      INTEGER,
  quarantined_at  INTEGER,
  archived_at     INTEGER,
  created_at      INTEGER NOT NULL
);

CREATE INDEX idx_workspaces_state ON workspaces(state);

CREATE TABLE workspace_files (
  id             TEXT PRIMARY KEY,
  workspace_id   TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  parent_id      TEXT,
  name           TEXT NOT NULL,
  blob_hash      TEXT NOT NULL,
  size           INTEGER NOT NULL,
  mime           TEXT NOT NULL,
  rev            INTEGER NOT NULL,
  deleted_at     INTEGER,
  created_at     INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_workspace_files_name
  ON workspace_files(workspace_id, name) WHERE deleted_at IS NULL;
CREATE INDEX idx_workspace_files_ws ON workspace_files(workspace_id);

CREATE TABLE share_links (
  id              TEXT PRIMARY KEY,
  token_hash      TEXT NOT NULL UNIQUE,
  kind            TEXT NOT NULL CHECK (kind IN ('file','workspace')),
  object_id       TEXT NOT NULL,
  title           TEXT NOT NULL DEFAULT '',
  note_scopes     INTEGER NOT NULL,
  file_scopes     INTEGER NOT NULL,
  expires_at      INTEGER,
  max_uses        INTEGER,
  used_count      INTEGER NOT NULL DEFAULT 0 CHECK (used_count >= 0),
  passphrase_hash TEXT,
  revoked_at      INTEGER,
  created_at      INTEGER NOT NULL
);

CREATE INDEX idx_links_token ON share_links(token_hash);

CREATE TABLE uploads (
  id            TEXT PRIMARY KEY,
  link_id       TEXT NOT NULL REFERENCES share_links(id),
  workspace_id  TEXT NOT NULL REFERENCES workspaces(id),
  blob_hash     TEXT,
  name          TEXT,
  mime          TEXT,
  size          INTEGER NOT NULL DEFAULT 0,
  wrapped_fk    TEXT,
  status        TEXT NOT NULL CHECK (status IN ('pending','complete','failed','aborted')),
  created_at    INTEGER NOT NULL
);

CREATE INDEX idx_uploads_link ON uploads(link_id);

-- 一次性下载票据（只存哈希；短 TTL，窗口内可重试）
CREATE TABLE blob_tickets (
  ticket_hash  TEXT PRIMARY KEY,
  link_id      TEXT NOT NULL,
  file_id      TEXT,
  blob_hash    TEXT NOT NULL,
  expires_at   INTEGER NOT NULL,
  created_at   INTEGER NOT NULL
);

-- 共享 blob 元数据；本体在 blobs/shared/<hash>
CREATE TABLE blobs (
  hash        TEXT NOT NULL,
  scope       TEXT NOT NULL CHECK (scope = 'shared'),
  size        INTEGER NOT NULL,
  refcount    INTEGER NOT NULL DEFAULT 1 CHECK (refcount >= 0),
  created_at  INTEGER NOT NULL,
  PRIMARY KEY (hash, scope)
);

-- 配额：实例总量与单工作区用量（上传预留/释放都在事务里做）
CREATE TABLE usage_counters (
  scope  TEXT NOT NULL CHECK (scope IN ('instance','workspace')),
  key    TEXT NOT NULL,
  bytes  INTEGER NOT NULL DEFAULT 0 CHECK (bytes >= 0),
  PRIMARY KEY (scope, key)
);

INSERT INTO usage_counters (scope, key, bytes) VALUES ('instance', 'total', 0);
