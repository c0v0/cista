import { afterEach, describe, expect, it, vi } from "vitest";

import type { SecretStore } from "./native";
import { generateKey } from "./noteCrypto";
import {
  MASTER_KEY_KEY,
  forgetMasterKey,
  rememberMasterKey,
  restoreMasterKey,
} from "./rememberedDevice";

function fakeStore() {
  const values: Record<string, string> = {};
  const store: SecretStore = {
    async save(key, value) {
      values[key] = value;
    },
    async load(key) {
      return values[key] ?? null;
    },
    async remove(key) {
      delete values[key];
    },
  };
  return { store, values };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("MK 持久化（本机自动解锁）", () => {
  it("保存后在系统安全存储里可读回，且不落普通 localStorage", async () => {
    const { store, values } = fakeStore();
    const mk = generateKey();

    await rememberMasterKey(store, mk);
    expect(Object.keys(values)).toEqual([MASTER_KEY_KEY]);

    const restored = await restoreMasterKey(store);
    expect(restored).not.toBeNull();
    expect(Array.from(restored!)).toEqual(Array.from(mk));
  });

  it("卸载/清除后读不到", async () => {
    const { store } = fakeStore();
    await rememberMasterKey(store, generateKey());
    await forgetMasterKey(store);
    expect(await restoreMasterKey(store)).toBeNull();
  });

  it("长度不是 32 字节的记录视为无效", async () => {
    const { store, values } = fakeStore();
    values[MASTER_KEY_KEY] = btoa("short");
    expect(await restoreMasterKey(store)).toBeNull();
  });
});
