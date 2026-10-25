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
});

describe("desktopSecretStore", () => {
  it("优先调用移动插件命令，参数透传", async () => {
    const invoke = vi.fn(async (command: string) => {
      if (command.endsWith("secret_load")) return { value: "c2VlZA" };
      return null;
    });
    vi.stubGlobal("__TAURI_INTERNALS__", { invoke });
    const store = desktopSecretStore();

    await store!.save("device-seed:abc", "seed");
    expect(invoke).toHaveBeenCalledWith("plugin:cista-keystore|secret_save", {
      key: "device-seed:abc",
      value: "seed",
    });

    expect(await store!.load("device-seed:abc")).toBe("c2VlZA");

    await store!.remove("device-seed:abc");
    expect(invoke).toHaveBeenCalledWith("plugin:cista-keystore|secret_delete", {
      key: "device-seed:abc",
    });
  });

  it("插件不可用时回退到宿主命令", async () => {
    const invoke = vi.fn(async (command: string) => {
      if (command.startsWith("plugin:")) throw new Error("plugin missing");
      if (command === "secret_load") return "c2VlZA";
      return null;
    });
    vi.stubGlobal("__TAURI_INTERNALS__", { invoke });
    const store = desktopSecretStore();

    await store!.save("k", "v");
    expect(invoke).toHaveBeenCalledWith("secret_save", { key: "k", value: "v" });
    expect(await store!.load("k")).toBe("c2VlZA");
  });

  it("load 返回非字符串/非 value 对象时归一为 null", async () => {
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
