import { describe, expect, it } from "vitest";

import { ShareError } from "../lib/shareClient";
import {
  FILE_MANAGE,
  FILE_READ,
  FILE_UPLOAD,
  describeShareError,
  downloadName,
  filePermissions,
  formatBytes,
} from "./sharePageModel";

describe("filePermissions", () => {
  it("按位拆出读/上传/管理权限", () => {
    expect(filePermissions(FILE_READ | FILE_UPLOAD | FILE_MANAGE)).toEqual({
      canRead: true,
      canUpload: true,
      canManage: true,
    });
    expect(filePermissions(FILE_UPLOAD)).toEqual({
      canRead: false,
      canUpload: true,
      canManage: false,
    });
    expect(filePermissions(0)).toEqual({
      canRead: false,
      canUpload: false,
      canManage: false,
    });
  });
});

describe("describeShareError", () => {
  it("410 说明过期/撤销/用尽", () => {
    expect(describeShareError(new ShareError(410, "gone"))).toContain("过期");
  });

  it("401 提示口令", () => {
    expect(describeShareError(new ShareError(401, "unauthorized"))).toContain("口令");
  });

  it("429 提示稍后再试", () => {
    expect(describeShareError(new ShareError(429, "rate_limited"))).toContain("稍后");
  });

  it("未知错误不泄漏细节", () => {
    const message = describeShareError(new Error("boom: internal path /srv/x"));
    expect(message).not.toContain("boom");
    expect(message).not.toContain("/srv");
  });
});

describe("downloadName", () => {
  it("去掉路径分隔符与控制字符", () => {
    expect(downloadName("../a/b\nc.txt")).toBe("..abc.txt");
  });

  it("空展示名用兜底", () => {
    expect(downloadName("   ")).toBe("download");
    expect(downloadName("", "分享文件")).toBe("分享文件");
  });
});

describe("formatBytes", () => {
  it("按量级选单位", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
    expect(formatBytes(20 * 1024 * 1024)).toBe("20 MB");
  });

  it("非法输入返回占位符", () => {
    expect(formatBytes(-1)).toBe("—");
  });
});
