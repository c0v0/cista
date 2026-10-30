/**
 * 私有附件：客户端加密 + 内容寻址上传/取回。
 *
 * 格式：上传到 `/api/files` 的字节是 `nonce(24) ‖ 密文`，AAD 固定 `cista.file.v1`；
 * 文件密钥 FK 随机生成，用 MK 包裹后放进笔记明文的 `attachments[].fk_wrap`。
 * 服务端只见密文与哈希（哈希按上传字节算，与去重语义一致）。
 */
import { sha256 } from "@noble/hashes/sha2.js";

import { signRequest } from "./authClient";
import { fromBase64, open, seal, toBase64, toHex } from "./noteCrypto";
import type { NoteAttachment } from "./noteCrypto";
import type { Session } from "./notesApi";

const FILE_AAD = new TextEncoder().encode("cista.file.v1");
const FK_AAD = new TextEncoder().encode("cista.fk.v1");
const NONCE_LEN = 24;

/** 只依赖读取能力，浏览器 `File` 与测试替身都能满足。 */
export interface AttachmentSource {
  name: string;
  arrayBuffer(): Promise<ArrayBuffer>;
}

export interface PreparedAttachment {
  attachment: NoteAttachment;
  /** 实际上传的密文字节。 */
  bytes: Uint8Array<ArrayBuffer>;
}

function signedHeaders(session: Session, method: string, path: string, body: Uint8Array) {
  return signRequest(session.device, {
    method,
    path,
    body,
    nonce: crypto.getRandomValues(new Uint8Array(16)),
    ts: Math.floor(Date.now() / 1000),
  });
}

export async function prepareAttachment(
  session: Session,
  source: AttachmentSource,
): Promise<PreparedAttachment> {
  const plaintext = new Uint8Array(await source.arrayBuffer());
  const fk = crypto.getRandomValues(new Uint8Array(32));
  const sealed = seal(fk, FILE_AAD, plaintext);
  const nonce = fromBase64(sealed.nonce);
  const ciphertext = fromBase64(sealed.ct);
  const bytes = new Uint8Array(new ArrayBuffer(nonce.length + ciphertext.length));
  bytes.set(nonce);
  bytes.set(ciphertext, nonce.length);

  return {
    bytes,
    attachment: {
      hash: toHex(sha256(bytes)),
      name: source.name,
      size: plaintext.length,
      fk_wrap: seal(session.mk, FK_AAD, fk),
    },
  };
}

export async function uploadAttachment(
  session: Session,
  prepared: PreparedAttachment,
): Promise<NoteAttachment> {
  const path = "/api/files";
  const response = await fetch(`${session.apiBase}${path}`, {
    method: "POST",
    headers: {
      ...signedHeaders(session, "POST", path, prepared.bytes),
      "idempotency-key": prepared.attachment.hash,
    },
    body: prepared.bytes,
  });
  if (!response.ok) throw new Error(`附件上传失败（HTTP ${response.status}）`);
  return prepared.attachment;
}

export async function downloadAttachment(
  session: Session,
  attachment: NoteAttachment,
): Promise<Uint8Array> {
  if (!/^[0-9a-f]{64}$/.test(attachment.hash)) {
    throw new Error("附件 hash 非法");
  }
  const path = `/api/files/${attachment.hash}`;
  const empty = new Uint8Array();
  const response = await fetch(`${session.apiBase}${path}`, {
    method: "GET",
    headers: signedHeaders(session, "GET", path, empty),
  });
  if (!response.ok) throw new Error(`附件下载失败（HTTP ${response.status}）`);
  const stored = new Uint8Array(await response.arrayBuffer());
  if (stored.length <= NONCE_LEN) throw new Error("附件密文被截断");

  const fk = open(session.mk, FK_AAD, attachment.fk_wrap);
  return open(
    fk,
    FILE_AAD,
    {
      nonce: toBase64(stored.slice(0, NONCE_LEN)),
      ct: toBase64(stored.slice(NONCE_LEN)),
    },
  );
}
