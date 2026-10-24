/**
 * 分享落地页的纯逻辑（不依赖 React，便于单测）。
 *
 * 与 `crates/core/src/share.rs` 的 scope 位保持一致；服务端是唯一权威，
 * 这里只决定 UI 显示哪些按钮。
 */
import { ShareError } from "../lib/shareClient";

export const FILE_READ = 1 << 2;
export const FILE_UPLOAD = 1 << 3;
export const FILE_MANAGE = 1 << 4;

export interface FilePermissions {
  canRead: boolean;
  canUpload: boolean;
  canManage: boolean;
}

export function filePermissions(fileScopes: number): FilePermissions {
  return {
    canRead: (fileScopes & FILE_READ) !== 0,
    canUpload: (fileScopes & FILE_UPLOAD) !== 0,
    canManage: (fileScopes & FILE_MANAGE) !== 0,
  };
}

/** 面向访客的错误文案：不暴露服务端内部细节，也不回显 token。 */
export function describeShareError(error: unknown): string {
  if (error instanceof ShareError) {
    switch (error.status) {
      case 401:
        return "需要口令，或口令不正确。";
      case 404:
        return "链接不存在，或对象已被删除。";
      case 403:
        return "此链接没有该操作的权限。";
      case 410:
        return "链接已过期、已撤销，或使用次数已用尽。";
      case 413:
        return "文件超过允许的大小。";
      case 429:
        return "请求过于频繁，请稍后再试。";
      default:
        return `请求失败（HTTP ${error.status}）。`;
    }
  }
  return "网络错误，请检查连接后重试。";
}

/** 下载文件名：用展示名，去掉控制字符与路径分隔符，空则用兜底名。 */
export function downloadName(title: string, fallback = "download"): string {
  // eslint-disable-next-line no-control-regex
  const cleaned = title.replace(/[\\/\u0000-\u001f]/g, "").trim();
  return cleaned.length > 0 ? cleaned : fallback;
}

/** 人类可读大小（B/KB/MB），用于文件列表。 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}
