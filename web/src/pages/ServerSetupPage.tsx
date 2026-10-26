import { useState } from "react";

import {
  checkServerBase,
  describeServer,
  probeServer,
} from "../lib/serverSettings";

/**
 * 服务器地址设置页：Web / 桌面 / Android 首次启动都会经过这里。
 *
 * - 原生端（桌面/Android）没有「当前站点」，必须填绝对地址；
 * - Web 端可以选「同源」（由同一个域名提供 SPA 与 API），也可以填跨域地址；
 * - 「测试连接」只打 `/healthz`，不发任何凭据。
 */
export function ServerSetupPage({
  initial,
  native,
  onSave,
  onCancel,
}: {
  initial: string;
  native: boolean;
  onSave: (base: string) => void;
  onCancel?: () => void;
}) {
  const [value, setValue] = useState(initial);
  const [sameOrigin, setSameOrigin] = useState(!native && initial === "");
  const [error, setError] = useState<string | null>(null);
  const [probe, setProbe] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const candidate = sameOrigin ? "" : value;

  const test = async () => {
    setBusy(true);
    setProbe(null);
    setError(null);
    try {
      const result = await probeServer(candidate);
      setProbe(result.message);
    } finally {
      setBusy(false);
    }
  };

  const submit = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setError(null);
    const checked = checkServerBase(candidate);
    if (!checked.ok) {
      setError(checked.error ?? "地址无效");
      return;
    }
    onSave(checked.base);
  };

  return (
    <section>
      <h1>连接服务器</h1>
      <p className="hint">
        笔记以端到端加密存放在你自己的 cista 服务端。地址只保存在本机，
        不会发送给第三方。当前：<code>{describeServer(candidate)}</code>
      </p>
      <form className="stack" onSubmit={submit}>
        {native ? null : (
          <label className="inline">
            <input
              type="checkbox"
              checked={sameOrigin}
              onChange={(event) => setSameOrigin(event.target.checked)}
            />
            使用当前站点（同源部署）
          </label>
        )}
        <label htmlFor="server-base">服务器地址</label>
        <input
          id="server-base"
          inputMode="url"
          autoComplete="off"
          spellCheck={false}
          placeholder="https://cista.example.com"
          value={sameOrigin ? "" : value}
          disabled={sameOrigin}
          onChange={(event) => setValue(event.target.value)}
        />
        <p className="hint">
          只填 origin（可带路径前缀），例如 <code>https://cista.example.com</code> 或
          <code>https://example.com/cista</code>；不要带账号、query、fragment。
        </p>
        <div className="row">
          <button type="submit" disabled={busy}>
            保存并继续
          </button>
          <button type="button" onClick={() => void test()} disabled={busy || (sameOrigin && !native)}>
            {busy ? "测试中…" : "测试连接"}
          </button>
          {onCancel ? (
            <button type="button" className="ghost" onClick={onCancel}>
              取消
            </button>
          ) : null}
        </div>
      </form>
      {probe ? <p className="hint" role="status">{probe}</p> : null}
      {error ? (
        <p className="error" role="alert">
          {error}
        </p>
      ) : null}
    </section>
  );
}
