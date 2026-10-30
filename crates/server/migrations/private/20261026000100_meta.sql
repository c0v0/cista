-- 键值元数据：目前只用于记录 bootstrap token 已消费（一次性入网）。
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
