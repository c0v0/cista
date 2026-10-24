import { afterEach, describe, expect, it, vi } from "vitest";

import { deviceFromSeed } from "./authClient";
import type { SecretStore } from "./native";
import { toBase64 } from "./noteCrypto";
import {
  forgetRememberedDevice,
  rememberDevice,
  restoreDevice,
} from "./rememberedDevice";

const seed = new Uint8Array(32).fill(7);
const device = deviceFromSeed(seed);

function fakeStore(initial: Record<string, string> = {}) {
  const values: Record<string, string> = { ...initial };
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

function fakeLocalStorage(initial: Record<string, string> = {}) {
  const values: Record<string, string> = { ...initial };
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values[key] ?? null,
    setItem: (key: string, value: string) => {
      values[key] = value;
    },
    removeItem: (key: string) => {
      delete values[key];
    },
  });
  return values;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("rememberDevice / restoreDevice", () => {
  it("记住后能还原出同一设备（种子重算 id 一致）", async () => {
    const local = fakeLocalStorage();
    const { store, values } = fakeStore();

    await rememberDevice(store, device);
    expect(local["cista.device.id"]).toBe(device.id);
    expect(values[`device-seed:${device.id}`]).toBe(toBase64(seed));

    const restored = await restoreDevice(store);
    expect(restored?.id).toBe(device.id);
    expect(restored?.pubkey).toEqual(device.pubkey);
  });

  it("没有记录或钥匙串缺条目时返回 null", async () => {
    fakeLocalStorage();
    const { store } = fakeStore();
    expect(await restoreDevice(store)).toBeNull();

    fakeLocalStorage({ "cista.device.id": device.id });
    expect(await restoreDevice(store)).toBeNull();
  });

  it("id 与种子不匹配时拒绝还原", async () => {
    const other = deviceFromSeed(new Uint8Array(32).fill(9));
    fakeLocalStorage({ "cista.device.id": other.id });
    const { store } = fakeStore({ [`device-seed:${other.id}`]: toBase64(seed) });
    expect(await restoreDevice(store)).toBeNull();
  });
});

describe("forgetRememberedDevice", () => {
  it("删除钥匙串条目与本地 id", async () => {
    const local = fakeLocalStorage();
    const { store, values } = fakeStore();
    await rememberDevice(store, device);

    await forgetRememberedDevice(store);
    expect(local["cista.device.id"]).toBeUndefined();
    expect(values[`device-seed:${device.id}`]).toBeUndefined();
  });
});
