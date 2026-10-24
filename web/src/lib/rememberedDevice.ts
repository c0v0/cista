/**
 * 「记住这台设备」：桌面端把 **设备私钥种子** 放系统钥匙串，id 放 localStorage（非秘密）。
 *
 * 约束：
 * - 只存种子，不存 MK；MK 仍需恢复码或用户输入（丢失钥匙串 ≠ 丢失笔记）。
 * - 恢复时用种子重算设备 id 并与记录比对，防止钥匙串里是别的设备。
 * - Web（无钥匙串）返回 `null`，调用方回退到每次重新配对。
 */
import { deviceFromSeed } from "./authClient";
import type { DeviceIdentity } from "./authClient";
import { deviceSecretKey } from "./native";
import type { SecretStore } from "./native";
import { fromBase64, toBase64 } from "./noteCrypto";

const DEVICE_ID_KEY = "cista.device.id";
const DEVICE_ID = /^[0-9a-f]{32}$/;

function localStore(): Pick<Storage, "getItem" | "setItem" | "removeItem"> | null {
  const scope = globalThis as { localStorage?: Storage };
  return scope.localStorage ?? null;
}

export async function rememberDevice(
  store: SecretStore,
  device: DeviceIdentity,
): Promise<void> {
  await store.save(deviceSecretKey(device.id), toBase64(device.seed));
  localStore()?.setItem(DEVICE_ID_KEY, device.id);
}

export async function restoreDevice(
  store: SecretStore,
): Promise<DeviceIdentity | null> {
  const id = localStore()?.getItem(DEVICE_ID_KEY) ?? "";
  if (!DEVICE_ID.test(id)) return null;
  const stored = await store.load(deviceSecretKey(id));
  if (!stored) return null;
  const device = deviceFromSeed(fromBase64(stored));
  return device.id === id ? device : null;
}

export async function forgetRememberedDevice(store: SecretStore): Promise<void> {
  const id = localStore()?.getItem(DEVICE_ID_KEY) ?? "";
  if (DEVICE_ID.test(id)) await store.remove(deviceSecretKey(id));
  localStore()?.removeItem(DEVICE_ID_KEY);
}
