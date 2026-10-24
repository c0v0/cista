import { afterEach, describe, expect, it, vi } from "vitest";

import { desktopSecretStore, deviceSecretKey, tauriInvoke } from "./native";

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("tauriInvoke", () => {
  it("非桌面环境返回 null", () => {
    expect(tauriInvoke()).toBeNull();
    expect(desktopSecretStore()).toBeNull();
  });

  it("桌面环境返回可用调用器", () => {
    const invoke = vi.fn(async () => null);
    vi.stubGlobal("__TAURI_INTERNALS__", { invoke });
    expect(tauriInvoke()).toBeTypeOf("function");
  });
});

describe("desktopSecretStore", () => {
  it("把三个命令映射到 Tauri invoke", async () => {
    const invoke = vi.fn(async (command: string) => {
      if (command === "secret_load") return "c2VlZA";
      return null;
    });
    vi.stubGlobal("__TAURI_INTERNALS__", { invoke });
    const store = desktopSecretStore();
    expect(store).not.toBeNull();

    await store!.save("device-seed:abc", "seed");
    expect(invoke).toHaveBeenCalledWith("secret_save", { key: "device-seed:abc", value: "seed" });

    expect(await store!.load("device-seed:abc")).toBe("c2VlZA");

    await store!.remove("device-seed:abc");
    expect(invoke).toHaveBeenCalledWith("secret_delete", { key: "device-seed:abc" });
  });

  it("load 返回非字符串时归一为 null", async () => {
    vi.stubGlobal("__TAURI_INTERNALS__", { invoke: vi.fn(async () => 42) });
    expect(await desktopSecretStore()!.load("k")).toBeNull();
  });
});

describe("deviceSecretKey", () => {
  it("按设备 id 生成稳定键名", () => {
    const id = "0123456789abcdef0123456789abcdef";
    expect(deviceSecretKey(id)).toBe(`device-seed:${id}`);
  });

  it("拒绝非法 deviceId", () => {
    expect(() => deviceSecretKey("not-hex")).toThrow(/32 位 hex/);
  });
});
