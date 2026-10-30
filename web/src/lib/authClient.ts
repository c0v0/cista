/**
 * B+ 请求签名（客户端）；与 `crates/core/src/auth.rs` 逐字节一致。
 *
 * 不变量（`docs/adr/0002-*`）：
 * - 签名素材固定为 `cista.b+1\nMETHOD\nPATH\nsha256(body) hex\nnonce hex\nts`；
 * - PATH 必须与请求行完全一致（含 query），body 是**原始字节**；
 * - 设备 ID = `hex(sha256(pubkey))` 的前 16 字节；
 * - 私钥/种子只放内存或平台安全存储，绝不落 localStorage/服务端。
 */
import { ed25519 } from "@noble/curves/ed25519.js";
import { sha256 } from "@noble/hashes/sha2.js";

import { toHex } from "./noteCrypto";

export const AUTH_DOMAIN = "cista.b+1";
export const DEVICE_SEED_LEN = 32;
export const NONCE_LEN = 16;

export interface SignedRequestMeta {
  method: string;
  /** 与请求行一致（含 query string）。 */
  path: string;
  body: Uint8Array;
  nonce: Uint8Array;
  ts: number;
}

export interface DeviceIdentity {
  /** 32 字节 Ed25519 种子。 */
  seed: Uint8Array;
  pubkey: Uint8Array;
  id: string;
}

export interface SignedHeaders extends Record<string, string> {
  "x-device-id": string;
  "x-nonce": string;
  "x-timestamp": string;
  "x-signature": string;
  "content-type": string;
}

/** 设备 ID = `hex(sha256(pubkey))[..32]`，服务端会复算比对。 */
export function deviceId(pubkey: Uint8Array): string {
  if (pubkey.length !== 32) throw new Error("公钥必须是 32 字节");
  return toHex(sha256(pubkey).slice(0, 16));
}

/** 签名素材；与 Rust `RequestMeta::canonical` 输出必须逐字节相同。 */
export function canonicalRequest(meta: SignedRequestMeta): string {
  return [
    AUTH_DOMAIN,
    meta.method.toUpperCase(),
    meta.path,
    toHex(sha256(meta.body)),
    toHex(meta.nonce),
    String(meta.ts),
  ].join("\n");
}

/** 由种子派生设备身份（公钥与 ID 都可被服务端复算）。 */
export function deviceFromSeed(seed: Uint8Array): DeviceIdentity {
  if (seed.length !== DEVICE_SEED_LEN) throw new Error("设备种子必须是 32 字节");
  const pubkey = ed25519.getPublicKey(seed);
  return { seed, pubkey, id: deviceId(pubkey) };
}

/** 生成签名请求头（`content-type` 一起给出，避免调用方漏掉）。 */
export function signRequest(
  device: DeviceIdentity,
  meta: SignedRequestMeta,
): SignedHeaders {
  if (meta.nonce.length !== NONCE_LEN) throw new Error("nonce 必须是 16 字节");
  const signature = ed25519.sign(
    new TextEncoder().encode(canonicalRequest(meta)),
    device.seed,
  );
  return {
    "x-device-id": device.id,
    "x-nonce": toHex(meta.nonce),
    "x-timestamp": String(meta.ts),
    "x-signature": toHex(signature),
    "content-type": "application/json",
  };
}
