/**
 * 分享 API 客户端（Web 访客端与测试共用）。
 *
 * 不变量（RULES/`design.md` §3.2）：分享 token 只在
 * `Authorization: Share <token>` 头里，绝不进 URL/query/日志。
 * 这里在发请求前显式拒绝把 token 拼进 path，避免调用方写错。
 *
 * 服务端响应一律经运行时校验（`parseShareMeta` 等）后再使用：
 * 客户端与服务端版本漂移时给出明确错误，而不是静默读出 undefined。
 */

import { resolveApiBase } from "./apiBase";

export type ShareKind = "file" | "workspace";

export interface ShareMeta {
  kind: ShareKind;
  title: string;
  note_scopes: number;
  file_scopes: number;
  expires_at: number | null;
  requires_passphrase: boolean;
  max_uses: number | null;
  used_count: number;
}

export interface ConfirmResponse {
  ticket: string;
  expires_at: number;
}

export interface PublicFileEntry {
  id: string;
  name: string;
  size: number;
  mime: string;
  rev: number;
  created_at: number;
}

export class ShareError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string) {
    super(`share request failed: ${status} ${code}`);
    this.name = "ShareError";
    this.status = status;
    this.code = code;
  }

  get isGone(): boolean {
    return this.status === 410;
  }
}

type Transport = typeof fetch;

/** 对象检查：返回 `Record<string, unknown>`，各字段仍是 unknown，必须逐个校验。 */
function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  // 结构化窄化：Object 到 Record<string, unknown> 只放宽索引访问，不虚构字段。
  return value as Record<string, unknown>;
}

function requireRecord(value: unknown, what: string): Record<string, unknown> {
  const record = asRecord(value);
  if (!record) throw new Error(`${what} 不是对象`);
  return record;
}

function readString(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  if (typeof value !== "string") throw new Error(`响应字段 ${key} 不是字符串`);
  return value;
}

function readNumber(record: Record<string, unknown>, key: string): number {
  const value = record[key];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`响应字段 ${key} 不是数字`);
  }
  return value;
}

function readBoolean(record: Record<string, unknown>, key: string): boolean {
  const value = record[key];
  if (typeof value !== "boolean") throw new Error(`响应字段 ${key} 不是布尔值`);
  return value;
}

function readNullableNumber(
  record: Record<string, unknown>,
  key: string,
): number | null {
  const value = record[key];
  if (value === null || value === undefined) return null;
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`响应字段 ${key} 不是数字/null`);
  }
  return value;
}

export function parseShareMeta(raw: unknown): ShareMeta {
  const record = requireRecord(raw, "meta 响应");
  const kind = readString(record, "kind");
  if (kind !== "file" && kind !== "workspace") {
    throw new Error(`meta.kind 非法：${kind}`);
  }
  return {
    kind,
    title: readString(record, "title"),
    note_scopes: readNumber(record, "note_scopes"),
    file_scopes: readNumber(record, "file_scopes"),
    expires_at: readNullableNumber(record, "expires_at"),
    requires_passphrase: readBoolean(record, "requires_passphrase"),
    max_uses: readNullableNumber(record, "max_uses"),
    used_count: readNumber(record, "used_count"),
  };
}

export function parseConfirm(raw: unknown): ConfirmResponse {
  const record = requireRecord(raw, "confirm 响应");
  return {
    ticket: readString(record, "ticket"),
    expires_at: readNumber(record, "expires_at"),
  };
}

export function parseFileEntry(raw: unknown): PublicFileEntry {
  const record = requireRecord(raw, "文件响应");
  return {
    id: readString(record, "id"),
    name: readString(record, "name"),
    size: readNumber(record, "size"),
    mime: readString(record, "mime"),
    rev: readNumber(record, "rev"),
    created_at: readNumber(record, "created_at"),
  };
}

function parseFileList(raw: unknown): PublicFileEntry[] {
  const record = requireRecord(raw, "文件列表响应");
  const files = record["files"];
  if (!Array.isArray(files)) throw new Error("响应字段 files 不是数组");
  return files.map(parseFileEntry);
}

function readErrorCode(body: unknown): string {
  if (body && typeof body === "object" && "error" in body) {
    const code = body.error;
    if (typeof code === "string") return code;
  }
  return "http_error";
}

export class ShareClient {
  private readonly apiBase: string;

  constructor(
    private readonly transport: Transport,
    private readonly token: string,
    options: { apiBase?: string; passphrase?: string } = {},
  ) {
    this.apiBase = resolveApiBase(options.apiBase);
    this.passphrase = options.passphrase;
  }

  private readonly passphrase?: string;

  private async request(path: string, init: RequestInit = {}): Promise<Response> {
    if (path.includes(this.token)) {
      throw new Error("分享 token 不能出现在 URL/path 里，只能走 Authorization 头");
    }
    const headers = new Headers(init.headers);
    headers.set("authorization", `Share ${this.token}`);
    if (this.passphrase) {
      headers.set("x-share-passphrase", this.passphrase);
    }
    const response = await this.transport(`${this.apiBase}${path}`, { ...init, headers });
    if (!response.ok) {
      let code = "http_error";
      try {
        code = readErrorCode(await response.json());
      } catch {
        // 非 JSON 错误体：保留默认 code。
      }
      throw new ShareError(response.status, code);
    }
    return response;
  }

  async meta(kind: ShareKind): Promise<ShareMeta> {
    const path = kind === "file" ? "/public/files/meta" : "/public/workspaces/meta";
    const response = await this.request(path);
    return parseShareMeta(await response.json());
  }

  async confirm(): Promise<ConfirmResponse> {
    const response = await this.request("/public/files/confirm", { method: "POST" });
    return parseConfirm(await response.json());
  }

  /** 单文件链接：凭票据流式下载（票据 45s、单对象、确认后窗口内可重试）。 */
  async downloadTicket(ticket: string): Promise<Blob> {
    const response = await this.request(
      `/public/files/blob?ticket=${encodeURIComponent(ticket)}`,
    );
    return await response.blob();
  }

  async listFiles(): Promise<PublicFileEntry[]> {
    const response = await this.request("/public/workspaces/files");
    return parseFileList(await response.json());
  }

  async upload(name: string, body: Blob, contentType: string): Promise<PublicFileEntry> {
    const response = await this.request("/public/workspaces/files/upload", {
      method: "POST",
      headers: { "x-file-name": name, "content-type": contentType },
      body,
    });
    return parseFileEntry(await response.json());
  }

  async rename(fileId: string, name: string): Promise<PublicFileEntry> {
    const response = await this.request(
      `/public/workspaces/files/${encodeURIComponent(fileId)}`,
      {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ name }),
      },
    );
    return parseFileEntry(await response.json());
  }

  async remove(fileId: string): Promise<void> {
    await this.request(`/public/workspaces/files/${encodeURIComponent(fileId)}`, {
      method: "DELETE",
    });
  }

  async downloadFile(fileId: string): Promise<Blob> {
    const response = await this.request(
      `/public/workspaces/files/${encodeURIComponent(fileId)}/blob`,
    );
    return await response.blob();
  }
}
