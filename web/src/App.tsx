import { resolveApiBase } from "./lib/apiBase";
import { parseShareFragment } from "./lib/shareLink";

/**
 * SPA 外壳（desktop 与 web 共用）。
 * 私有内容服务端不可读；共享内容服务端可读——这条边界必须出现在 UI 上。
 */
export function App() {
  const apiBase = resolveApiBase(import.meta.env.VITE_API_BASE);
  const target = parseShareFragment(window.location.hash);

  return (
    <main>
      <h1>Cista</h1>
      {target ? (
        <p>
          分享链接（{target.kind}）：token 已从 fragment 读取（{target.token.length} 字符），
          只作为 `Authorization: Share` header 发送。
        </p>
      ) : (
        <p>私有笔记端到端加密：服务端只存密文。共享内容服务端可读。</p>
      )}
      <p>API base：{apiBase === "" ? "同源" : apiBase}</p>
    </main>
  );
}
