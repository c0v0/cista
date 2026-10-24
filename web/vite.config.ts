import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// 前端不做同源假设：跨域部署时用 VITE_API_BASE 指定后端 origin（design §4.2）。
export default defineConfig({
  plugins: [react()],
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
