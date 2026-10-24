/**
 * 私有内容的客户端加密（与 `crates/core/src/crypto.rs` 逐字节兼容）。
 *
 * 不变量：
 * - XChaCha20-Poly1305（24B nonce；每次加密新随机，测试可显式指定）。
 * - AAD = `domain ‖ 0x00 ‖ (len:u32be ‖ part)*`，与 Rust `aad()` 完全一致。
 * - 传输格式：JSON 里 `nonce`/`ct` 是 base64 **STANDARD（带填充）**。
 * - 明文是 `{"title":…,"markdown":…}` 的 UTF-8 JSON（字段顺序固定）。
 */
import { xchacha20poly1305 } from "@noble/ciphers/chacha.js";
import { randomBytes } from "@noble/ciphers/utils.js";

export const KEY_LEN = 32;
export const NONCE_LEN = 24;
export const NOTE_VERSION = 1;

export interface Sealed {
  nonce: string;
  ct: string;
}

export interface SealedNote {
  v: number;
  rev: number;
  wrap: Sealed;
  body: Sealed;
}

export interface NoteAttachment {
  hash: string;
  name: string;
  size: number;
  /** 被 MK 包裹的文件密钥（FK）。 */
  fk_wrap: Sealed;
}

export interface NotePlaintext {
  title: string;
  markdown: string;
  /** 空/缺省时不参与序列化（与 Rust `skip_serializing_if` 一致）。 */
  attachments?: NoteAttachment[];
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** 32 字节对称密钥（MK 或每对象的 CK/FK）。调用方负责只放内存。 */
export type Key = Uint8Array;

export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function fromHex(hex: string): Uint8Array {
  if (hex.length % 2 !== 0) throw new Error("hex 长度必须是偶数");
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i += 1) {
    const byte = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
    if (Number.isNaN(byte)) throw new Error("非法 hex");
    out[i] = byte;
  }
  return out;
}

export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

export function fromBase64(value: string): Uint8Array {
  const binary = atob(value);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) out[i] = binary.charCodeAt(i);
  return out;
}

/** `domain NUL (len:u32be ‖ part)*`（与 Rust `aad()` 一致）。 */
function aad(domain: string, parts: Uint8Array[]): Uint8Array {
  const domainBytes = encoder.encode(domain);
  const total =
    domainBytes.length + 1 + parts.reduce((sum, part) => sum + 4 + part.length, 0);
  const out = new Uint8Array(total);
  const view = new DataView(out.buffer);
  let offset = 0;
  out.set(domainBytes, offset);
  offset += domainBytes.length;
  out[offset] = 0;
  offset += 1;
  for (const part of parts) {
    view.setUint32(offset, part.length);
    offset += 4;
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

export function ckAad(noteId: string): Uint8Array {
  return aad("cista.ck.v1", [encoder.encode(noteId)]);
}

export function noteAad(noteId: string, rev: number | bigint): Uint8Array {
  const revBytes = new Uint8Array(8);
  new DataView(revBytes.buffer).setBigUint64(0, BigInt(rev));
  return aad("cista.note.v1", [encoder.encode(noteId), revBytes]);
}

export function generateKey(): Key {
  return randomBytes(KEY_LEN);
}

/** 恢复码：32B 的 base64url（无填充），与 Rust `Key::recovery_code` 一致。 */
export function recoveryCode(key: Key): string {
  if (key.length !== KEY_LEN) throw new Error("密钥长度必须为 32 字节");
  return toBase64(key).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function fromRecoveryCode(code: string): Key {
  const normalized = code.trim().replace(/-/g, "+").replace(/_/g, "/");
  const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
  const key = fromBase64(padded);
  if (key.length !== KEY_LEN) throw new Error("恢复码长度不正确");
  return key;
}

export function seal(
  key: Key,
  aadBytes: Uint8Array,
  plaintext: Uint8Array,
  nonce: Uint8Array = randomBytes(NONCE_LEN),
): Sealed {
  if (key.length !== KEY_LEN) throw new Error("密钥长度必须为 32 字节");
  if (nonce.length !== NONCE_LEN) throw new Error("nonce 长度必须为 24 字节");
  const ct = xchacha20poly1305(key, nonce, aadBytes).encrypt(plaintext);
  return { nonce: toBase64(nonce), ct: toBase64(ct) };
}

export function open(key: Key, aadBytes: Uint8Array, sealed: Sealed): Uint8Array {
  if (key.length !== KEY_LEN) throw new Error("密钥长度必须为 32 字节");
  const nonce = fromBase64(sealed.nonce);
  if (nonce.length !== NONCE_LEN) throw new Error("nonce 长度必须为 24 字节");
  return xchacha20poly1305(key, nonce, aadBytes).decrypt(fromBase64(sealed.ct));
}

/** 与 Rust `NotePlaintext` 的 serde 序列化一致（字段顺序 title → markdown）。 */
export function encodePlaintext(note: NotePlaintext): Uint8Array {
  // 字段顺序与 Rust serde 一致（title → markdown → attachments）；空附件不输出。
  const payload =
    note.attachments && note.attachments.length > 0
      ? { title: note.title, markdown: note.markdown, attachments: note.attachments }
      : { title: note.title, markdown: note.markdown };
  return encoder.encode(JSON.stringify(payload));
}

export function decodePlaintext(bytes: Uint8Array): NotePlaintext {
  const parsed: unknown = JSON.parse(decoder.decode(bytes));
  if (!parsed || typeof parsed !== "object") throw new Error("明文不是对象");
  const record = parsed as Record<string, unknown>;
  if (typeof record.title !== "string" || typeof record.markdown !== "string") {
    throw new Error("明文缺少 title/markdown");
  }
  const rawAttachments = record.attachments;
  if (rawAttachments === undefined) {
    return { title: record.title, markdown: record.markdown };
  }
  if (!Array.isArray(rawAttachments)) throw new Error("attachments 不是数组");
  const attachments = rawAttachments.map((entry) => {
    if (!entry || typeof entry !== "object") throw new Error("附件不是对象");
    const item = entry as Record<string, unknown>;
    if (typeof item.hash !== "string" || typeof item.name !== "string") {
      throw new Error("附件缺少 hash/name");
    }
    if (typeof item.size !== "number" || !Number.isFinite(item.size)) {
      throw new Error("附件 size 不是数字");
    }
    const wrap = item.fk_wrap as Record<string, unknown> | undefined;
    if (!wrap || typeof wrap.nonce !== "string" || typeof wrap.ct !== "string") {
      throw new Error("附件 fk_wrap 非法");
    }
    return {
      hash: item.hash,
      name: item.name,
      size: item.size,
      fk_wrap: { nonce: wrap.nonce, ct: wrap.ct },
    };
  });
  return { title: record.title, markdown: record.markdown, attachments };
}

export interface SealNoteOptions {
  /** 仅测试用：固定 nonce（生产省略，自动随机）。 */
  wrapNonce?: Uint8Array;
  bodyNonce?: Uint8Array;
}

/** MK 包裹 CK、CK 加密正文；返回可上行的信封。 */
export function sealNote(
  mk: Key,
  noteId: string,
  rev: number,
  ck: Key,
  note: NotePlaintext,
  options: SealNoteOptions = {},
): SealedNote {
  const wrap = seal(mk, ckAad(noteId), ck, options.wrapNonce);
  const body = seal(ck, noteAad(noteId, rev), encodePlaintext(note), options.bodyNonce);
  return { v: NOTE_VERSION, rev, wrap, body };
}

/** 解信封：CK 包裹与正文的 AAD 都绑定 noteId/rev，篡改或搬运都会失败。 */
export function openNote(mk: Key, noteId: string, sealed: SealedNote): NotePlaintext {
  if (sealed.v !== NOTE_VERSION) throw new Error(`不支持的密封版本 ${sealed.v}`);
  const ck = open(mk, ckAad(noteId), sealed.wrap);
  return decodePlaintext(open(ck, noteAad(noteId, sealed.rev), sealed.body));
}
