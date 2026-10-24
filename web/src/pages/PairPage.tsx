import { useEffect, useMemo, useState } from "react";

import type { DeviceIdentity } from "../lib/authClient";
import { desktopSecretStore } from "../lib/native";
import type { Session } from "../lib/notesApi";
import { enrollDevice } from "../lib/notesApi";
import { fromRecoveryCode, generateKey, recoveryCode } from "../lib/noteCrypto";
import { rememberDevice, restoreDevice } from "../lib/rememberedDevice";

/**
 * 配对页：bootstrap token 只能入网一次；私钥种子与 MK 都只留在内存。
 *
 * - 新设备：生成 32B Ed25519 种子 → 入网 → 生成 MK（并展示恢复码，用户自行离线保存）。
 * - 已有恢复码：粘贴即可用同一把 MK 解密历史笔记。
 */
export function PairPage({
  apiBase,
  onPaired,
}: {
  apiBase: string;
  onPaired: (session: Session) => void;
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
        // 只在桌面端写系统钥匙串；Web 端保持纯内存。
        await rememberDevice(store, device);
      }
      const mk = recovery.trim() ? fromRecoveryCode(recovery) : generateKey();
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
        主密钥只保存在本页内存里：刷新即锁定，需要粘贴恢复码重新解锁。
        {store
          ? " 桌面端会把设备私钥存进系统钥匙串（不存 MK），刷新后无需重新入网。"
          : " Web 端设备私钥也只存在内存。"}
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
