-- 私有库（private.db）：设备公钥、笔记密文、增量 oplog、私有 blob 元数据。
-- 服务端只看到密文信封（sealed 是客户端产出的 JSON 字符串），不解密。

CREATE TABLE devices (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  pubkey        BLOB NOT NULL,
  created_at    INTEGER NOT NULL,
  revoked_at    INTEGER,
  last_seen     INTEGER,
  last_acked_seq INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX idx_devices_pubkey ON devices(pubkey);

CREATE TABLE notes (
  id          TEXT PRIMARY KEY,
  rev         INTEGER NOT NULL,
  sealed      TEXT NOT NULL,
  updated_at  INTEGER NOT NULL,
  deleted_at  INTEGER
);

CREATE INDEX idx_notes_rev ON notes(rev);

-- 增量同步日志：只做私有增量，不做复制；不建审计表。
CREATE TABLE oplog (
  seq         INTEGER PRIMARY KEY AUTOINCREMENT,
  entity      TEXT NOT NULL,
  entity_id   TEXT NOT NULL,
  op          TEXT NOT NULL,
  payload     TEXT NOT NULL,
  created_at  INTEGER NOT NULL
);

CREATE INDEX idx_oplog_entity ON oplog(entity, entity_id);

-- 私有 blob 元数据；文件本体在 blobs/private/<hash>。共享 blob 在 shared.db + blobs/shared/。
CREATE TABLE blobs (
  hash        TEXT NOT NULL,
  scope       TEXT NOT NULL CHECK (scope = 'private'),
  size        INTEGER NOT NULL,
  refcount    INTEGER NOT NULL DEFAULT 1 CHECK (refcount >= 0),
  created_at  INTEGER NOT NULL,
  PRIMARY KEY (hash, scope)
);
