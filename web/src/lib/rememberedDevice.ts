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

/** 主密钥（MK）在系统安全存储里的键名；与设备种子分开存放。 */
export const MASTER_KEY_KEY = "master-key";

/**
 * 「本机自动解锁」：把 MK 交给系统安全存储（Keystore/Keychain/CredManager）保管。
 *
 * 只应在原生端启用；Web 没有等价的硬件保护，保持内存-only。
 * 服务端永远拿不到 MK——这里只是本机缓存的加密副本。
 */
export async function rememberMasterKey(
  store: SecretStore,
  mk: Uint8Array,
): Promise<void> {
  await store.save(MASTER_KEY_KEY, toBase64(mk));
}

export async function restoreMasterKey(
  store: SecretStore,
): Promise<Uint8Array | null> {
  const stored = await store.load(MASTER_KEY_KEY);
  if (!stored) return null;
  const mk = fromBase64(stored);
  return mk.length === 32 ? mk : null;
}

export async function forgetMasterKey(store: SecretStore): Promise<void> {
  await store.remove(MASTER_KEY_KEY);
}
