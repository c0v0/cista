import { useEffect, useMemo, useState } from "react";

import { resolveApiBase } from "./lib/apiBase";
import { desktopSecretStore, purgeLegacySecrets, tauriInvoke } from "./lib/native";
import type { Session } from "./lib/notesApi";
import {
  forgetMasterKey,
  forgetRememberedDevice,
  restoreDevice,
  restoreMasterKey,
} from "./lib/rememberedDevice";
import {
  describeServer,
  initialServerBase,
  isServerConfigured,
  requiresServerSetup,
  writeServerBase,
} from "./lib/serverSettings";
import { parseShareFragment } from "./lib/shareLink";
import { NotesPage } from "./pages/NotesPage";
import { PairPage } from "./pages/PairPage";
import { ServerSetupPage } from "./pages/ServerSetupPage";
import { SharePage } from "./pages/SharePage";

/**
 * SPA 外壳（desktop / Android / web 共用）。
 *
 * 路由（fragment，token 不进 query/Referer）：
 * - `#/s/<token>` 单文件分享、`#/w/<token>` 工作区分享 → 访客页；
 * - `#/app` → 私有笔记：未配对先配对，`Session`（设备私钥 + MK）只存在内存；
 * - `#/setup` → 服务器地址设置（首次启动的强制步骤，也可随时修改）。
 *
 * 服务器地址：原生客户端必须由用户填写（没有「当前站点」），Web 默认同源，
 * 可在设置页改成跨域地址；地址只存 localStorage。
 */
export function App() {
  const [hash, setHash] = useState(() => window.location.hash);
  const [session, setSession] = useState<Session | null>(null);

  const native = useMemo(() => tauriInvoke() !== null, []);
  const envBase = import.meta.env.VITE_API_BASE as string | undefined;
  const [serverBase, setServerBase] = useState(() => initialServerBase(envBase));
  const [configured, setConfigured] = useState(() => isServerConfigured(envBase));

  useEffect(() => {
    const onChange = () => setHash(window.location.hash);
    window.addEventListener("hashchange", onChange);
    return () => window.removeEventListener("hashchange", onChange);
  }, []);

  const apiBase = resolveApiBase(serverBase);
  const target = parseShareFragment(hash);
  const wantsApp = hash.startsWith("#/app");
  const wantsSetup = hash.startsWith("#/setup");
  const needsSetup = requiresServerSetup(native, configured);

  // 原生端：设备种子 + MK 都在系统安全存储里时，启动即自动解锁（Web 无 store，保持内存-only）。
  useEffect(() => {
    const store = desktopSecretStore();
    if (!store) return;
    let cancelled = false;
    void (async () => {
      try {
        // Android 旧版本可能把敏感键写进明文 JSON：启动时先清理。
        await purgeLegacySecrets();
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
    if (store) void forgetMasterKey(store).catch(() => {});
  };

  const saveServer = (base: string) => {
    writeServerBase(base);
    setServerBase(base);
    setConfigured(true);
    // 换服务器等于换身份：清掉内存会话与系统存储里的设备种子/MK。
    setSession(null);
    const store = desktopSecretStore();
    if (store) {
      void (async () => {
        try {
          await forgetMasterKey(store);
        } catch {
          // 旧版本/钥匙串异常不应阻止更换服务器。
        }
        try {
          await forgetRememberedDevice(store);
        } catch {
          // 同上。
        }
      })();
    }
    window.location.hash = wantsSetup || needsSetup ? "#/app" : hash;
  };

  const serverFooter = (
    <footer>
      服务器：{describeServer(serverBase)}
      {" · "}
      <a href="#/setup">更改</a>
    </footer>
  );

  if (needsSetup && !wantsSetup) {
    return (
      <main>
        <ServerSetupPage initial={serverBase} native={native} onSave={saveServer} />
        {serverFooter}
      </main>
    );
  }

  return (
    <main>
      {wantsSetup ? (
        <ServerSetupPage
          initial={serverBase}
          native={native}
          onSave={saveServer}
          onCancel={configured ? () => { window.location.hash = session ? "#/app" : ""; } : undefined}
        />
      ) : null}
      {!wantsSetup && target ? <SharePage target={target} apiBase={apiBase} /> : null}
      {!wantsSetup && !target && wantsApp
        ? session
          ? <NotesPage session={session} onLock={lock} />
          : <PairPage apiBase={apiBase} onPaired={setSession} onChangeServer={() => { window.location.hash = "#/setup"; }} />
        : null}
      {!wantsSetup && !target && !wantsApp ? (
        <>
          <h1>Cista</h1>
          <p>私有笔记端到端加密：服务端只存密文。共享内容服务端可读。</p>
          <p>
            <a href="#/app">打开笔记</a>
            {" · "}
            <a href="#/setup">服务器设置</a>
          </p>
        </>
      ) : null}
      {serverFooter}
    </main>
  );
}
