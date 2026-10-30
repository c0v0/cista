import { afterEach, describe, expect, it, vi } from "vitest";
import {
  SERVER_BASE_KEY,
  checkServerBase,
  clearServerBase,
  describeServer,
  initialServerBase,
  isServerConfigured,
  probeServer,
  readStoredServerBase,
  requiresServerSetup,
  serverBaseFromEnv,
  writeServerBase,
} from "./serverSettings";

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

describe("checkServerBase", () => {
  it("空值表示同源", () => {
    for (const input of ["", "   ", "/"]) {
      expect(checkServerBase(input)).toEqual({ ok: true, base: "" });
    }
  });

  it("接受 http(s) origin 并去掉尾斜杠", () => {
    expect(checkServerBase("https://cista.example.com").base).toBe("https://cista.example.com");
    expect(checkServerBase("https://cista.example.com/").base).toBe("https://cista.example.com");
    expect(checkServerBase("  http://127.0.0.1:8787  ").base).toBe("http://127.0.0.1:8787");
  });

  it("保留路径前缀（反代子路径部署）", () => {
    expect(checkServerBase("https://example.com/cista/").base).toBe("https://example.com/cista");
  });

  it("拒绝缺少协议、空白、非法协议、账号密码、query、fragment", () => {
    for (const input of [
      "cista.example.com",
      "https://exa mple.com",
      "ftp://example.com",
      "https://user:pass@example.com",
      "https://example.com/?a=1",
      "https://example.com/#/app",
    ]) {
      const checked = checkServerBase(input);
      expect(checked.ok, input).toBe(false);
      expect(checked.error).toBeTruthy();
    }
  });
});

describe("本地保存", () => {
  it("写入后可读回，显式同源也能保存", () => {
    fakeLocalStorage();
    writeServerBase("https://cista.example.com/");
    expect(readStoredServerBase()).toBe("https://cista.example.com");
    writeServerBase("");
    expect(readStoredServerBase()).toBe("");
  });

  it("未保存返回 null；坏数据按未保存处理；clear 删除", () => {
    const values = fakeLocalStorage({ [SERVER_BASE_KEY]: "not a url" });
    expect(readStoredServerBase()).toBeNull();
    values[SERVER_BASE_KEY] = "https://ok.example.com";
    expect(readStoredServerBase()).toBe("https://ok.example.com");
    clearServerBase();
    expect(readStoredServerBase()).toBeNull();
  });
});

describe("初始值与配置判定", () => {
  it("优先级：保存值 > 构建期注入 > 同源", () => {
    fakeLocalStorage();
    expect(initialServerBase("https://env.example.com")).toBe("https://env.example.com");
    writeServerBase("https://stored.example.com");
    expect(initialServerBase("https://env.example.com")).toBe("https://stored.example.com");
    clearServerBase();
    expect(initialServerBase(undefined)).toBe("");
  });

  it("空/非法的构建期值不算配置", () => {
    fakeLocalStorage();
    expect(serverBaseFromEnv("   ")).toBeNull();
    expect(serverBaseFromEnv("nonsense")).toBeNull();
    expect(isServerConfigured("")).toBe(false);
    expect(isServerConfigured("https://env.example.com")).toBe(true);
    writeServerBase("");
    expect(isServerConfigured(undefined)).toBe(true);
  });

  it("原生端未配置时必须先设置；Web 端不强制", () => {
    expect(requiresServerSetup(true, false)).toBe(true);
    expect(requiresServerSetup(true, true)).toBe(false);
    expect(requiresServerSetup(false, false)).toBe(false);
  });

  it("describeServer 区分同源与绝对地址", () => {
    expect(describeServer("")).toBe("同源（当前站点）");
    expect(describeServer("https://cista.example.com/")).toBe("https://cista.example.com");
  });
});

describe("probeServer", () => {
  it("健康检查返回 ok 即成功", async () => {
    const calls: string[] = [];
    const fakeFetch = (async (input: RequestInfo | URL) => {
      calls.push(String(input));
      return new Response("ok", { status: 200 });
    }) as typeof fetch;
    const result = await probeServer("https://cista.example.com/", fakeFetch);
    expect(result.ok).toBe(true);
    expect(calls).toEqual(["https://cista.example.com/healthz"]);
  });

  it("HTTP 错误、响应不是 ok、网络异常都报失败", async () => {
    const status = await probeServer(
      "https://a.example.com",
      (async () => new Response("nope", { status: 502 })) as typeof fetch,
    );
    expect(status.ok).toBe(false);
    expect(status.message).toContain("502");

    const body = await probeServer(
      "https://a.example.com",
      (async () => new Response("<html>", { status: 200 })) as typeof fetch,
    );
    expect(body.ok).toBe(false);

    const network = await probeServer(
      "https://a.example.com",
      (async () => {
        throw new Error("dns");
      }) as typeof fetch,
    );
    expect(network.ok).toBe(false);
    expect(network.message).toContain("dns");
  });

  it("非法地址直接失败，不发请求", async () => {
    const fakeFetch = vi.fn() as unknown as typeof fetch;
    const result = await probeServer("ftp://x", fakeFetch);
    expect(result.ok).toBe(false);
    expect(fakeFetch).not.toHaveBeenCalled();
  });
});
