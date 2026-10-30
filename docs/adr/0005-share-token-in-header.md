# ADR 0005：公网分享 token 只走 header（偏离 `design.md` §8 的路径草图）

- 状态：接受（Phase 2）
- 日期：2026-10-24

## 背景

`design.md` §8 的分享路由草图写成 `/public/files/{token}/meta` 这种「token 在路径里」的形式；
但同一份设计的 §3.2 与不变量 2 要求：**分享 token 不进日志/query/Referer**。
反向代理（Caddy）默认会把 URI 写进访问日志，路径里带 token 等于必然泄漏。

`design.md` §3.2 已给出正确做法：链接用 fragment（`#/s/<token>`、`#/w/<token>`），
SPA 调 API 时放 header（`Authorization: Share <token>` 或 `X-Share-Token`）。

## 选择

以不变量为准：**公网路由路径里没有 token**。

| 规格草图（§8） | 实现 |
|---|---|
| `GET /public/files/{token}/meta` | `GET /public/files/meta` + `Authorization: Share <t>` |
| `POST /public/files/{token}/confirm` | `POST /public/files/confirm` + 同上 |
| `GET /public/files/{token}/blob` | `GET /public/files/blob?ticket=…` + 同上 |
| `GET /public/workspaces/{token}/meta` | `GET /public/workspaces/meta` |
| `GET /public/workspaces/{token}/files` | `GET /public/workspaces/files` |
| `POST /public/workspaces/{token}/files/upload` | `POST /public/workspaces/files/upload` |
| `PATCH/DELETE /public/workspaces/{token}/files/{id}` | `PATCH/DELETE /public/workspaces/files/{id}` |
| `GET /public/workspaces/{token}/files/{id}/blob` | `GET /public/workspaces/files/{id}/blob` |
| `GET /public/workspaces/{token}/events` | `GET /public/workspaces/events` |

其它并发/生命周期决定：

- `confirm` 用**条件 UPDATE**（`used_count = used_count + 1 WHERE … used_count < max_uses`）原子消费，
  并发下只有一次成功；不做「先读后写」。
- 文件上传 v1 在 handler 内缓冲（路由层 `DefaultBodyLimit = max_file_bytes` 硬限）。
  真正的流式写入 + 硬截断放 Phase 6（不影响「超限不留半文件」：超限在落盘前就 413）。
- 配额来自环境变量（`CISTA_INSTANCE_QUOTA_BYTES` 等），部署时按卷容量 0.80/0.25/0.10 计算后写入；
  代码不做 `statvfs`（避免为一次计算引入 `libc`/`unsafe`）。

## 后果

- 好处：token 不可能出现在访问日志、Referer、浏览器历史（fragment 不发往服务器）。
- 代价：与 §8 的草图路由不同名；客户端必须用 header（已在 `web/src/App.tsx` 与 spec 中约定）。
- 验证：`tests/sharing.rs::tokens_never_appear_in_responses`；route 表见 `crates/server/src/lib.rs`。
