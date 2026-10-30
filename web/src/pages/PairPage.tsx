import { useEffect, useMemo, useState } from "react";

import type { DeviceIdentity } from "../lib/authClient";
import { desktopSecretStore } from "../lib/native";
import type { Session } from "../lib/notesApi";
import { enrollDevice } from "../lib/notesApi";
import { fromRecoveryCode, generateKey, recoveryCode } from "../lib/noteCrypto";
import { rememberDevice, rememberMasterKey, restoreDevice } from "../lib/rememberedDevice";
import { describeServer } from "../lib/serverSettings";

/**
 * 配对页：bootstrap token 只能入网一次；私钥种子与 MK 默认只在内存，
 * 原生端可显式写入系统安全存储（自动解锁）。
 *
 * - 新设备：生成 32B Ed25519 种子 → 入网 → 生成 MK（并展示恢复码，用户自行离线保存）。
 * - 已有恢复码：粘贴即可用同一把 MK 解密历史笔记。
 * - 服务器地址在这里可见、可更换（首次启动已在 `#/setup` 填过）。
 */
export function PairPage({
  apiBase,
  onPaired,
  onChangeServer,
}: {
  apiBase: string;
  onPaired: (session: Session) => void;
  onChangeServer?: () => void;
}) {
  const [bootstrap, setBootstrap] = useState("");
  const [name, setName] = useState("我的设备");
  const [recovery, setRecovery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [remembered, setRemembered] = useState<DeviceIdentity | null>(null);
  const store = useMemo(() => desktopSecretStore(), []);

  // 桌面端：钥匙串里有设备种子就复用，不必再拿一次性 bootstrap token。
  useEffect(() => {
    if (!store) return;
    let cancelled = false;
    void restoreDevice(store).then((device) => {
      if (!cancelled) setRemembered(device);
    });
    return () => {
      cancelled = true;
    };
  }, [store]);

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const device =
        remembered ??
        (await enrollDevice(
          apiBase,
          bootstrap.trim(),
          name.trim() || "设备",
          crypto.getRandomValues(new Uint8Array(32)),
        ));
      if (!remembered && store) {
        // 桌面端写系统钥匙串；Android 走 Keystore 插件，失败则仅本次会话可用。
        try {
          await rememberDevice(store, device);
        } catch {
          // 本次会话仍可正常使用，只是刷新后需要重新入网。
        }
      }
      const mk = recovery.trim() ? fromRecoveryCode(recovery) : generateKey();
      if (store) {
        // 原生端：MK 交给系统安全存储，下次启动自动解锁（Web 端 store 为 null）。
        try {
          await rememberMasterKey(store, mk);
        } catch {
          // 失败则退化为内存-only，不影响本次使用。
        }
      }
      onPaired({ apiBase, device, mk });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "配对失败");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section>
      <h1>配对这台设备</h1>
      <p className="hint">
        服务器：<code>{describeServer(apiBase)}</code>
        {onChangeServer ? (
          <>
            {" · "}
            <button type="button" onClick={onChangeServer}>
              更换服务器
            </button>
          </>
        ) : null}
      </p>
      <p className="hint">
        {store
          ? "设备种子与主密钥保存在本机系统安全存储（桌面钥匙串 / Android Keystore）；「锁定」会清除本机保存的主密钥。"
          : "Web 端没有系统安全存储：设备私钥与主密钥只在内存里，刷新即锁定。"}
      </p>
      {remembered ? (
        <p className="hint">
          已记住设备 <code>{remembered.id.slice(0, 8)}</code>：只需粘贴恢复码（或留空新建 MK）。
        </p>
      ) : null}
      <form className="stack" onSubmit={(event) => void submit(event)}>
        <label htmlFor="bootstrap">Bootstrap token（服务端环境变量给出，仅一次有效）</label>
        <input
          id="bootstrap"
          type="password"
          autoComplete="off"
          value={bootstrap}
          onChange={(event) => setBootstrap(event.target.value)}
          required={!remembered}
        />
        <label htmlFor="device-name">设备名</label>
        <input
          id="device-name"
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
        <label htmlFor="recovery">恢复码（可选：已有 MK 时填写，留空则新建）</label>
        <input
          id="recovery"
          autoComplete="off"
          value={recovery}
          onChange={(event) => setRecovery(event.target.value)}
          placeholder="base64url，例如 xyz…"
        />
        <button type="submit" disabled={busy || (!remembered && bootstrap.trim().length === 0)}>
          {busy ? "配对中…" : "配对"}
        </button>
      </form>
      {error ? (
        <p className="error" role="alert">
          {error}
        </p>
      ) : null}
      <p className="hint">新建设备时会生成 MK；请立即在笔记页「显示恢复码」并离线保存。</p>
    </section>
  );
}

/** 供笔记页展示恢复码（用户离线保存 MK）。 */
export function RecoveryCodePanel({ mk }: { mk: Uint8Array }) {
  const [shown, setShown] = useState(false);
  const code = recoveryCode(mk);
  return (
    <p className="hint">
      恢复码：
      {shown ? <code>{code}</code> : <span>（点击显示，勿截图外传）</span>}
      <button type="button" onClick={() => setShown((value) => !value)}>
        {shown ? "隐藏" : "显示恢复码"}
      </button>
    </p>
  );
}
