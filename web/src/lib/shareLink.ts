/** 分享目标类型：单文件链接 `#/s/<token>`，工作区链接 `#/w/<token>`。 */
export type ShareKind = "file" | "workspace";

export interface ShareTarget {
  kind: ShareKind;
  token: string;
}

/** token 是 128-bit 随机值的 base64url/hex 表示；只允许 URL 安全字符。 */
const FRAGMENT = /^#?\/([sw])\/([A-Za-z0-9_-]{16,64})$/;

/**
 * 解析分享链接 fragment。
 *
 * token 只存在于 fragment：不进 query、不进 Referer、不进服务端访问日志；
 * 调 API 时放进 `Authorization: Share <token>` header（design §3.2）。
 * 非法/不完整 fragment 返回 `null`，由调用方决定 404 页面。
 */
export function parseShareFragment(hash: string): ShareTarget | null {
  const match = FRAGMENT.exec(hash.trim());
  if (!match) return null;
  const [, prefix, token] = match;
  return { kind: prefix === "s" ? "file" : "workspace", token: token ?? "" };
}
