import { describe, expect, it } from "vitest";

import { parseShareFragment } from "./shareLink";

const TOKEN_B64 = "Zm9vYmFyYmF6cXV1eDEyMzQ1Ng";
const TOKEN_HEX = "0123456789abcdef0123456789abcdef";

describe("parseShareFragment", () => {
  it("识别单文件链接", () => {
    expect(parseShareFragment(`#/s/${TOKEN_B64}`)).toEqual({ kind: "file", token: TOKEN_B64 });
  });

  it("识别工作区链接（允许缺少前导 #）", () => {
    expect(parseShareFragment(`/w/${TOKEN_HEX}`)).toEqual({ kind: "workspace", token: TOKEN_HEX });
  });

  it("拒绝把 token 放进 query 的写法", () => {
    expect(parseShareFragment(`#/s/?token=${TOKEN_B64}`)).toBeNull();
    expect(parseShareFragment(`#/s/${TOKEN_B64}?x=1`)).toBeNull();
  });

  it("拒绝未知类型、过短 token 与非 URL 安全字符", () => {
    expect(parseShareFragment(`#/x/${TOKEN_B64}`)).toBeNull();
    expect(parseShareFragment("#/s/shorttoken")).toBeNull();
    expect(parseShareFragment("#/s/aaaaaaaaaaaaaaaaaaaa+=")).toBeNull();
    expect(parseShareFragment("#/")).toBeNull();
    expect(parseShareFragment("")).toBeNull();
  });
});
