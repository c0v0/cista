import { describe, expect, it } from "vitest";

import { resolveApiBase } from "./apiBase";

describe("resolveApiBase", () => {
  it("同源部署返回相对 base", () => {
    expect(resolveApiBase(undefined)).toBe("");
    expect(resolveApiBase(null)).toBe("");
    expect(resolveApiBase("")).toBe("");
    expect(resolveApiBase("   ")).toBe("");
    expect(resolveApiBase("/")).toBe("");
  });

  it("跨域部署返回去掉尾斜杠的 origin", () => {
    expect(resolveApiBase("https://api.example.com")).toBe("https://api.example.com");
    expect(resolveApiBase("https://api.example.com/")).toBe("https://api.example.com");
    expect(resolveApiBase(" https://api.example.com// ")).toBe("https://api.example.com");
  });

  it("保留 base 里的路径前缀", () => {
    expect(resolveApiBase("https://example.com/cista")).toBe("https://example.com/cista");
    expect(resolveApiBase("https://example.com/cista/")).toBe("https://example.com/cista");
  });
});
