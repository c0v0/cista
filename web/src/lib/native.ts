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

let platformOs: Promise<string | null> | null = null;

function readOs(raw: unknown): string | null {
  if (raw && typeof raw === "object" && "os" in raw) {
    const os = raw.os;
    return typeof os === "string" ? os : null;
  }
  return null;
}

/** 桌面端用宿主 keyring；Android 用插件。结果缓存一次。 */
function currentOs(invoke: Invoke): Promise<string | null> {
  if (!platformOs) {
    platformOs = invoke("platform_info")
      .then(readOs)
      .catch(() => null);
  }
  return platformOs;
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
    // 删除永远允许：清理旧密钥不能被 fail-closed 挡住。
    if (!isSensitiveKey(key) || command === "secret_delete") {
      return await invoke(command, args);
    }
    // Android 的宿主实现只是明文兜底：敏感键必须失败。
    if ((await currentOs(invoke)) === "android") throw error;
    // 桌面端插件未注册命令，宿主 keyring 才是正确路径。
    return await invoke(command, args);
  }
}

function readValue(raw: unknown): string | null {
  if (typeof raw === "string") return raw;
  if (raw && typeof raw === "object" && "value" in raw) {
    const value = raw.value;
    return typeof value === "string" ? value : null;
  }
  return null;
}

/** Android 升级后清理旧版本写入明文 JSON 的敏感条目；其他平台 no-op。 */
export async function purgeLegacySecrets(): Promise<void> {
  const invoke = tauriInvoke();
  if (!invoke) return;
  if ((await currentOs(invoke)) !== "android") return;
  try {
    await invoke("purge_legacy_secrets");
  } catch {
    // 旧版本没有该命令时忽略。
  }
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
