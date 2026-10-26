/**
 * 服务器地址设置（每个客户端首次启动时必须能填写）。
 *
 * 不变量：
 * - 地址只存 `localStorage`（非秘密），键 `cista.server.base`；空串 = 同源。
 * - 接受的形态：绝对 http(s) origin（可带路径前缀），或空/`/`（同源）。
 * - 拒绝账号密码、query、fragment、非 http(s) 协议；这些会让签名串与日志泄漏。
 */
import { resolveApiBase } from "./apiBase";

export const SERVER_BASE_KEY = "cista.server.base";

export interface ServerBaseCheck {
  ok: boolean;
  /** 归一化后的请求前缀；`""` 表示同源。 */
  base: string;
  error?: string;
}

function storage(): Storage | null {
  const scope = globalThis as { localStorage?: Storage };
  return scope.localStorage ?? null;
}

/** 校验并归一化用户输入。 */
export function checkServerBase(input: string): ServerBaseCheck {
  const raw = input.trim();
  if (raw === "" || raw === "/") return { ok: true, base: "" };
  if (/\s/.test(raw)) return { ok: false, base: "", error: "地址不能包含空白字符" };
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return { ok: false, base: "", error: "请输入完整地址，例如 https://cista.example.com" };
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    return { ok: false, base: "", error: "只支持 http(s) 地址" };
  }
  if (url.username !== "" || url.password !== "") {
    return { ok: false, base: "", error: "地址里不能带账号密码" };
  }
  if (url.search !== "" || url.hash !== "") {
    return { ok: false, base: "", error: "地址不能带查询参数或 fragment" };
  }
  return { ok: true, base: resolveApiBase(url.origin + url.pathname) };
}

/** 读本地已保存的地址；`null` 表示用户从未设置过。 */
export function readStoredServerBase(): string | null {
  const stored = storage()?.getItem(SERVER_BASE_KEY);
  if (stored === null || stored === undefined) return null;
  const checked = checkServerBase(stored);
  return checked.ok ? checked.base : null;
}

/** 保存地址；`""` 表示显式选择同源。 */
export function writeServerBase(base: string): void {
  storage()?.setItem(SERVER_BASE_KEY, resolveApiBase(base));
}

export function clearServerBase(): void {
  storage()?.removeItem(SERVER_BASE_KEY);
}

/** 构建期注入的默认值（自建实例可以把地址烧进 `web/dist`）。 */
export function serverBaseFromEnv(envValue?: string | null): string | null {
  if (envValue === undefined || envValue === null) return null;
  const trimmed = envValue.trim();
  if (trimmed === "") return null;
  const checked = checkServerBase(trimmed);
  return checked.ok ? checked.base : null;
}

/** 首次渲染使用的地址：用户保存的 > 构建期注入 > 同源。 */
export function initialServerBase(envValue?: string | null): string {
  const stored = readStoredServerBase();
  if (stored !== null) return stored;
  return serverBaseFromEnv(envValue) ?? "";
}

/** 是否已经确定过服务器（用户保存过，或构建期注入了非空地址）。 */
export function isServerConfigured(envValue?: string | null): boolean {
  return readStoredServerBase() !== null || serverBaseFromEnv(envValue) !== null;
}

/**
 * 原生客户端（Tauri 桌面/Android）没有「当前站点」可依赖，
 * 首次启动必须先让用户填写服务器地址。
 */
export function requiresServerSetup(native: boolean, configured: boolean): boolean {
  return native && !configured;
}

/** 页脚/配对页展示用。 */
export function describeServer(base: string): string {
  const normalized = resolveApiBase(base);
  return normalized === "" ? "同源（当前站点）" : normalized;
}

export interface ProbeResult {
  ok: boolean;
  message: string;
}

/** 探测 `/healthz`：确认地址可达且确实是 cista 服务端。 */
export async function probeServer(
  base: string,
  fetchImpl: typeof fetch = fetch,
): Promise<ProbeResult> {
  const checked = checkServerBase(base);
  if (!checked.ok) return { ok: false, message: checked.error ?? "地址无效" };
  const url = `${checked.base}/healthz`;
  try {
    const response = await fetchImpl(url, { method: "GET", cache: "no-store" });
    if (!response.ok) {
      return { ok: false, message: `${url} 返回 HTTP ${response.status}` };
    }
    const body = (await response.text()).trim();
    if (body !== "ok") {
      return { ok: false, message: `${url} 的响应不是 cista 服务端（期望 ok）` };
    }
    return { ok: true, message: `已连接 ${url}` };
  } catch (cause) {
    const detail = cause instanceof Error ? cause.message : String(cause);
    return { ok: false, message: `无法连接 ${url}：${detail}` };
  }
}
