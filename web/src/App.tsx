import { useEffect, useState } from "react";

import { resolveApiBase } from "./lib/apiBase";
import { desktopSecretStore } from "./lib/native";
import type { Session } from "./lib/notesApi";
import { forgetMasterKey, restoreDevice, restoreMasterKey } from "./lib/rememberedDevice";
import { parseShareFragment } from "./lib/shareLink";
import { NotesPage } from "./pages/NotesPage";
import { PairPage } from "./pages/PairPage";
import { SharePage } from "./pages/SharePage";

/**
 * SPA 外壳（desktop 与 web 共用）。
 *
 * 路由（fragment，token 不进 query/Referer）：
 * - `#/s/<token>` 单文件分享、`#/w/<token>` 工作区分享 → 访客页；
 * - `#/app` → 私有笔记：未配对先配对，`Session`（设备私钥 + MK）只存在内存。
 */
export function App() {
  const [hash, setHash] = useState(() => window.location.hash);
  const [session, setSession] = useState<Session | null>(null);

  useEffect(() => {
    const onChange = () => setHash(window.location.hash);
    window.addEventListener("hashchange", onChange);
    return () => window.removeEventListener("hashchange", onChange);
  }, []);

  const apiBase = resolveApiBase(import.meta.env.VITE_API_BASE);
  const target = parseShareFragment(hash);
  const wantsApp = hash.startsWith("#/app");

  // 原生端：设备种子 + MK 都在系统安全存储里时，启动即自动解锁（Web 无 store，保持内存-only）。
  useEffect(() => {
    const store = desktopSecretStore();
    if (!store) return;
    let cancelled = false;
    void (async () => {
      try {
        const device = await restoreDevice(store);
        const mk = await restoreMasterKey(store);
        if (!cancelled && device && mk) setSession({ apiBase, device, mk });
      } catch {
        // 读不到就按未解锁处理。
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [apiBase]);

  const lock = () => {
    setSession(null);
    const store = desktopSecretStore();
    if (store) void forgetMasterKey(store);
  };

  return (
    <main>
      {target ? <SharePage target={target} /> : null}
      {!target && wantsApp
        ? session
          ? <NotesPage session={session} onLock={lock} />
          : <PairPage apiBase={apiBase} onPaired={setSession} />
        : null}
      {!target && !wantsApp ? (
        <>
          <h1>Cista</h1>
          <p>私有笔记端到端加密：服务端只存密文。共享内容服务端可读。</p>
          <p>
            <a href="#/app">打开笔记</a>
          </p>
        </>
      ) : null}
      <footer>API：{apiBase === "" ? "同源" : apiBase}</footer>
    </main>
  );
}
