/**
 * 桌面端原生能力桥（Tauri 2）。Web 环境下 `desktopSecretStore()` 返回 `null`，调用方回退到内存。
 *
 * 约定：钥匙串只允许存**设备私钥种子**；MK 永不进系统凭据库（丢失即全部笔记不可解，
 * 不应长期驻留）。是否“记住这台设备”由用户在桌面上显式选择。
 */
export interface SecretStore {
  save(key: string, value: string): Promise<void>;
  load(key: string): Promise<string | null>;
  remove(key: string): Promise<void>;
}

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

/** 取 Tauri 的 invoke；非桌面环境返回 `null`（不要抛错，调用方按能力探测）。 */
export function tauriInvoke(): Invoke | null {
  const scope = globalThis as { __TAURI_INTERNALS__?: { invoke?: unknown } };
  const invoke = scope.__TAURI_INTERNALS__?.invoke;
  return typeof invoke === "function" ? (invoke as Invoke) : null;
}

export function desktopSecretStore(): SecretStore | null {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  return {
    async save(key, value) {
      await invoke("secret_save", { key, value });
    },
    async load(key) {
      const value = await invoke("secret_load", { key });
      return typeof value === "string" ? value : null;
    },
    async remove(key) {
      await invoke("secret_delete", { key });
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
