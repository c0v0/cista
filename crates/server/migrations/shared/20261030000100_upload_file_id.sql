-- 稳定关联上传行与工作区文件：删除文件时按 file_id 释放链接配额。
-- 旧行的 file_id 为 NULL（保守：不自动释放其链接配额）。
ALTER TABLE uploads ADD COLUMN file_id TEXT;

CREATE INDEX idx_uploads_file ON uploads(file_id);
