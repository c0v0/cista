import { useEffect, useState } from "react";

import { resolveApiBase } from "./lib/apiBase";
import { parseShareFragment } from "./lib/shareLink";
import { SharePage } from "./pages/SharePage";

/**
 * SPA 外壳（desktop 与 web 共用）。
 * 私有内容服务端不可读；共享内容服务端可读——这条边界必须出现在 UI 上。
 *
 * 分享链接是 fragment 路由（`#/s/<token>`、`#/w/<token>`）：token 不进 query、
 * 不进 Referer、不被服务端日志记录。
 */
export function App() {
  const [hash, setHash] = useState(() => window.location.hash);

  useEffect(() => {
    const onChange = () => setHash(window.location.hash);
    window.addEventListener("hashchange", onChange);
    return () => window.removeEventListener("hashchange", onChange);
  }, []);

  const apiBase = resolveApiBase(import.meta.env.VITE_API_BASE);
  const target = parseShareFragment(hash);

  return (
    <main>
      {target ? (
        <SharePage target={target} />
      ) : (
        <>
          <h1>Cista</h1>
          <p>私有笔记端到端加密：服务端只存密文。共享内容服务端可读。</p>
        </>
      )}
      <footer>API：{apiBase === "" ? "同源" : apiBase}</footer>
    </main>
  );
}
