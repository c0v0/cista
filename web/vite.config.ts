import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// 生产构建注入 CSP（dev 下 Vite HMR 需要内联脚本/ws，故只在 build 生效）。
// 允许 https/wss 是为了支持运行期填写的跨域服务器地址。
const CSP =
  "default-src 'self'; connect-src 'self' https: wss:; " +
  "img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; " +
  "script-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'";

function cspMeta() {
  return {
    name: "cista-csp-meta",
    apply: "build" as const,
    transformIndexHtml(html: string): string {
      return html.replace(
        "</head>",
        `    <meta http-equiv="Content-Security-Policy" content="${CSP}" />\n  </head>`,
      );
    },
  };
}

// 前端不做同源假设：跨域部署时用 VITE_API_BASE 指定后端 origin（design §4.2）。
export default defineConfig({
  plugins: [react(), cspMeta()],
  build: {
    outDir: "dist",
    // 发布检查要求 bundle 里没有 sourcemap。
    sourcemap: false,
  },
  server: {
    port: 5173,
    strictPort: true,
  },
});
