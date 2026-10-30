// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";

/** App 在模块顶层不读 hash，但渲染时会读 `window.location.hash`。 */
function setHash(hash: string) {
  window.history.replaceState(null, "", `/${hash}`);
}

function stubTauri() {
  Reflect.set(globalThis, "__TAURI_INTERNALS__", { invoke: async () => null });
}

beforeEach(() => {
  localStorage.clear();
  Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(globalThis, "__TAURI_INTERNALS__");
  localStorage.clear();
});

describe("原生端首次启动", () => {
  it("未配置服务器时强制先填地址，保存后进入配对页", async () => {
    stubTauri();
    setHash("");
    render(<App />);

    expect(screen.getByRole("heading", { name: "连接服务器" })).toBeTruthy();
    // 原生端没有「当前站点」，不提供同源选项
    expect(screen.queryByLabelText("使用当前站点（同源部署）")).toBeNull();

    fireEvent.change(screen.getByLabelText("服务器地址"), {
      target: { value: "https://cista.example.com/" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存并继续" }));

    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "配对这台设备" })).toBeTruthy();
    });
    expect(localStorage.getItem("cista.server.base")).toBe("https://cista.example.com");
    expect(screen.getAllByText("https://cista.example.com").length).toBeGreaterThan(0);
  });

  it("非法地址不保存，并显示错误", async () => {
    stubTauri();
    setHash("");
    render(<App />);

    fireEvent.change(screen.getByLabelText("服务器地址"), {
      target: { value: "ftp://nope.example" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存并继续" }));

    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      "只支持 http(s) 地址",
    );
    expect(localStorage.getItem("cista.server.base")).toBeNull();
  });
});

describe("测试连接", () => {
  it("healthz 返回 ok 时显示成功，失败时显示原因", async () => {
    stubTauri();
    setHash("");
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response("ok", { status: 200 }))
      .mockResolvedValueOnce(new Response("bad gateway", { status: 502 }));
    vi.stubGlobal("fetch", fetchMock);
    render(<App />);

    fireEvent.change(screen.getByLabelText("服务器地址"), {
      target: { value: "https://cista.example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "测试连接" }));
    await waitFor(() => {
      expect(screen.getByRole("status").textContent).toContain(
        "已连接 https://cista.example.com/healthz",
      );
    });

    fireEvent.click(screen.getByRole("button", { name: "测试连接" }));
    await waitFor(() => {
      expect(screen.getByRole("status").textContent).toContain("502");
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});

describe("Web 端", () => {
  it("默认同源，可进入设置页修改并保存", async () => {
    setHash("");
    render(<App />);

    expect(screen.getByRole("heading", { name: "Cista" })).toBeTruthy();
    expect(screen.getByText(/同源（当前站点）/)).toBeTruthy();

    fireEvent.click(screen.getByRole("link", { name: "服务器设置" }));
    // hashchange 在 jsdom 里由 click 触发
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: "连接服务器" })).toBeTruthy();
    });

    fireEvent.click(screen.getByLabelText("使用当前站点（同源部署）")); // 取消同源
    fireEvent.change(screen.getByLabelText("服务器地址"), {
      target: { value: "https://api.example.com" },
    });
    fireEvent.click(screen.getByRole("button", { name: "保存并继续" }));

    await waitFor(() => {
      expect(screen.getAllByText("https://api.example.com").length).toBeGreaterThan(0);
    });
    expect(localStorage.getItem("cista.server.base")).toBe("https://api.example.com");
  });
});
