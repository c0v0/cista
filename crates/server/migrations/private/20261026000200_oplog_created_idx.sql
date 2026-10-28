-- oplog 按时间裁剪（90 天）；索引避免每次 PUT 全表扫描。
CREATE INDEX IF NOT EXISTS idx_oplog_created ON oplog(created_at);
