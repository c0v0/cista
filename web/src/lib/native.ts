/**
 * 原生能力桥（Tauri 2）。Web 环境返回 `null`，调用方回退到内存。
 *
 * 密钥存储优先走移动插件（Android Keystore），失败再回退到宿主命令
 * （桌面 keyring / Android 应用私有目录兜底）。
 */
export interface SecretStore {
  save(key: string, value: string): Promise<void>;
  load(key: string): Promise<string | null>;
  remove(key: string): Promise<void>;
}

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

const PLUGIN = "cista-keystore";

export function tauriInvoke(): Invoke | null {
  const scope = globalThis as { __TAURI_INTERNALS__?: { invoke?: unknown } };
  const invoke = scope.__TAURI_INTERNALS__?.invoke;
  return typeof invoke === "function" ? (invoke as Invoke) : null;
}

/** 设备种子与 MK 只能进系统安全存储；插件不可用时必须失败而不是降级。 */
function isSensitiveKey(key: string): boolean {
  return key === "master-key" || key.startsWith("device-seed:");
}

async function invokeSecret(
  invoke: Invoke,
  command: string,
  args: Record<string, unknown>,
): Promise<unknown> {
  const key = typeof args.key === "string" ? args.key : "";
  try {
    return await invoke(`plugin:${PLUGIN}|${command}`, args);
  } catch (error) {
    // 敏感键绝不回退到宿主命令（Android 的宿主实现只是非敏感键兜底）。
    if (isSensitiveKey(key)) throw error;
    return await invoke(command, args);
  }
}

function readValue(raw: unknown): string | null {
  if (typeof raw === "string") return raw;
  if (raw && typeof raw === "object" && "value" in raw) {
    const value = (raw as Record<string, unknown>).value;
    return typeof value === "string" ? value : null;
  }
  return null;
}

export function desktopSecretStore(): SecretStore | null {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  return {
    async save(key, value) {
      await invokeSecret(invoke, "secret_save", { key, value });
    },
    async load(key) {
      return readValue(await invokeSecret(invoke, "secret_load", { key }));
    },
    async remove(key) {
      await invokeSecret(invoke, "secret_delete", { key });
    },
  };
}

/** 设备种子在钥匙串里的键名：一个设备一条，避免多设备互相覆盖。 */
export function deviceSecretKey(deviceId: string): string {
  if (!/^[0-9a-f]{32}$/.test(deviceId)) {
    throw new Error("deviceId 必须是 32 位 hex");
  }
  return `device-seed:${deviceId}`;
}
