import { describe, expect, it } from "vitest";

import {
  canonicalRequest,
  deviceFromSeed,
  deviceId,
  signRequest,
} from "./authClient";
import { cryptoVectors } from "./cryptoVectors";
import { fromHex, toHex } from "./noteCrypto";

const auth = cryptoVectors.auth;

describe("deviceFromSeed", () => {
  it("公钥与设备 ID 与 Rust 一致", () => {
    const device = deviceFromSeed(fromHex(auth.seed_hex));
    expect(toHex(device.pubkey)).toBe(auth.pubkey_hex);
    expect(device.id).toBe(auth.device_id);
    expect(deviceId(device.pubkey)).toBe(auth.device_id);
  });

  it("拒绝非 32 字节种子", () => {
    expect(() => deviceFromSeed(new Uint8Array(31))).toThrow(/32/);
  });
});

describe("canonicalRequest", () => {
  it("签名素材与 Rust 逐字节一致", () => {
    expect(
      canonicalRequest({
        method: auth.method,
        path: auth.path,
        body: fromHex(auth.body_hex),
        nonce: fromHex(auth.nonce_hex),
        ts: auth.ts,
      }),
    ).toBe(auth.canonical);
  });

  it("方法大小写归一、body 参与摘要", () => {
    const base = {
      method: "put",
      path: auth.path,
      body: fromHex(auth.body_hex),
      nonce: fromHex(auth.nonce_hex),
      ts: auth.ts,
    };
    expect(canonicalRequest(base)).toBe(auth.canonical);
    const otherBody = canonicalRequest({ ...base, body: new Uint8Array([1]) });
    expect(otherBody).not.toBe(auth.canonical);
  });
});

describe("signRequest", () => {
  it("签名与 Rust 一致，且头字段齐全", () => {
    const device = deviceFromSeed(fromHex(auth.seed_hex));
    const headers = signRequest(device, {
      method: auth.method,
      path: auth.path,
      body: fromHex(auth.body_hex),
      nonce: fromHex(auth.nonce_hex),
      ts: auth.ts,
    });
    expect(headers["x-signature"]).toBe(auth.signature_hex);
    expect(headers["x-device-id"]).toBe(auth.device_id);
    expect(headers["x-nonce"]).toBe(auth.nonce_hex);
    expect(headers["x-timestamp"]).toBe(String(auth.ts));
    expect(headers["content-type"]).toBe("application/json");
  });

  it("换 body 后签名变化（防篡改）", () => {
    const device = deviceFromSeed(fromHex(auth.seed_hex));
    const base = {
      method: auth.method,
      path: auth.path,
      nonce: fromHex(auth.nonce_hex),
      ts: auth.ts,
    };
    const first = signRequest(device, { ...base, body: fromHex(auth.body_hex) });
    const second = signRequest(device, { ...base, body: new Uint8Array([0]) });
    expect(first["x-signature"]).not.toBe(second["x-signature"]);
  });

  it("拒绝错误长度的 nonce", () => {
    const device = deviceFromSeed(fromHex(auth.seed_hex));
    expect(() =>
      signRequest(device, {
        method: auth.method,
        path: auth.path,
        body: fromHex(auth.body_hex),
        nonce: new Uint8Array(8),
        ts: auth.ts,
      }),
    ).toThrow(/nonce/);
  });
});
