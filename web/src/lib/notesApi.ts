/**
 * 私有笔记 API（B+ 签名 + 客户端加密），桌面与 Web 共用。
 *
 * 不变量：
 * - MK 与设备私钥默认只在内存（`Session`）；原生端可显式写入系统安全存储用于自动解锁，
 *   但绝不写 localStorage/IndexedDB/URL，也绝不发给服务端。
 * - 每次请求新 nonce + 当前时间戳；body 原始字节参与签名。
 * - `sealed.rev` 必须等于服务端将写入的 rev（AAD 绑定），所以保存前要算好。
 * - 冲突（409）不自动合并：交给调用方生成冲突副本。
 */
import { deviceFromSeed, signRequest } from "./authClient";
import type { DeviceIdentity, SignedRequestMeta } from "./authClient";
import { generateKey, openNote, sealNote } from "./noteCrypto";
import type { NotePlaintext, SealedNote } from "./noteCrypto";

export interface Session {
  apiBase: string;
  device: DeviceIdentity;
  /** 私有主密钥：只放内存。 */
  mk: Uint8Array;
}

export interface NoteRecord {
  id: string;
  rev: number;
  updated_at: number;
  sealed: SealedNote;
}

export interface NotesPage {
  notes: NoteRecord[];
  next_since: number;
}

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly rev?: number;

  constructor(status: number, code: string, rev?: number) {
    super(`api ${status} ${code}`);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.rev = rev;
  }

  get isRevConflict(): boolean {
    return this.status === 409 && this.code === "rev_conflict";
  }
}

const encoder = new TextEncoder();

async function errorFrom(response: Response): Promise<ApiError> {
  let code = "http_error";
  let rev: number | undefined;
  try {
    const body: unknown = await response.json();
    if (body && typeof body === "object") {
      const record = body as Record<string, unknown>;
      if (typeof record.error === "string") code = record.error;
      if (typeof record.rev === "number") rev = record.rev;
    }
  } catch {
    // 非 JSON 错误体：保留默认 code。
  }
  return new ApiError(response.status, code, rev);
}

/** 发一个已签名的 JSON 请求；`body` 为 `undefined` 表示无请求体。 */
export async function signedFetch(
  session: Session,
  method: string,
  path: string,
  body?: unknown,
  extraHeaders: Record<string, string> = {},
): Promise<Response> {
  const bytes =
    body === undefined ? new Uint8Array() : encoder.encode(JSON.stringify(body));
  const meta: SignedRequestMeta = {
    method,
    path,
    body: bytes,
    nonce: crypto.getRandomValues(new Uint8Array(16)),
    ts: Math.floor(Date.now() / 1000),
  };
  const headers = signRequest(session.device, meta);
  const response = await fetch(`${session.apiBase}${path}`, {
    method,
    headers: { ...headers, ...extraHeaders },
    body: body === undefined ? undefined : bytes,
  });
  if (!response.ok) throw await errorFrom(response);
  return response;
}

/** 用一次性 bootstrap token 入网；设备身份交给调用方放进内存 `Session`。 */
export async function enrollDevice(
  apiBase: string,
  bootstrapToken: string,
  name: string,
  seed: Uint8Array,
  transport: typeof fetch = fetch,
): Promise<DeviceIdentity> {
  const device = deviceFromSeed(seed);
  const pubkeyHex = Array.from(device.pubkey, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
  const response = await transport(`${apiBase}/api/devices/enroll`, {
    method: "POST",
    headers: {
      authorization: `Bootstrap ${bootstrapToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({ name, pubkey: pubkeyHex }),
  });
  if (!response.ok) throw await errorFrom(response);
  const payload: unknown = await response.json();
  const returned =
    payload && typeof payload === "object"
      ? (payload as Record<string, unknown>).device_id
      : undefined;
  if (returned !== device.id) {
    throw new Error("服务端返回的 device_id 与本地公钥不一致");
  }
  return device;
}

export async function listNotes(session: Session, since = 0): Promise<NotesPage> {
  const response = await signedFetch(session, "GET", `/api/notes?since=${since}`);
  const payload = (await response.json()) as NotesPage;
  return { notes: payload.notes, next_since: payload.next_since };
}

export async function getNote(session: Session, id: string): Promise<NoteRecord> {
  const response = await signedFetch(
    session,
    "GET",
    `/api/notes/${encodeURIComponent(id)}`,
  );
  return (await response.json()) as NoteRecord;
}

/**
 * 保存一条笔记：每次生成新 `CK`，`rev` 由调用方给出（新建 1，更新为当前 rev+1）。
 * `ifMatch` 有值时带上 `If-Match`，冲突抛 `ApiError.isRevConflict`。
 */
export async function saveNote(
  session: Session,
  id: string,
  rev: number,
  note: NotePlaintext,
  ifMatch?: number,
): Promise<NoteRecord> {
  const sealed = sealNote(session.mk, id, rev, generateKey(), note);
  const headers: Record<string, string> = {};
  if (ifMatch !== undefined) headers["if-match"] = String(ifMatch);
  const response = await signedFetch(
    session,
    "PUT",
    `/api/notes/${encodeURIComponent(id)}`,
    { sealed },
    headers,
  );
  return (await response.json()) as NoteRecord;
}

/** 解一条服务端记录；MK 不对或密文被改都会抛错（AAD 绑定 id + rev）。 */
export function decryptNote(session: Session, record: NoteRecord): NotePlaintext {
  return openNote(session.mk, record.id, record.sealed);
}

/** 新建笔记 ID：服务端只要求 URL 安全且唯一（32 hex）。 */
export function newNoteId(): string {
  const random = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(random, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
