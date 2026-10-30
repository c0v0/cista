# 规格：Phase 6 —— 加固与发布

> 验收来源：`design.md` §11「Phase 6：加固与发布」。

## 目标 / 非目标

- 目标：把「单机可用」变成「可对外、可回滚」：限速、分页硬上限、日志脱敏、
  每端点鉴权矩阵、密钥扫描、发布清单证据。
- 非目标：不建审计表/审计 UI、不建 metrics/Prometheus、不做公网 admin API（运维只走 SSH）；
  不加 `/readyz`。

## 1. 限速与资源限制

| 维度 | 规则 | 失败码 |
|---|---|---|
| 每 IP | `/api/*`、`/public/*`、`/web/*`、`/ws` 各自独立桶 | 429 + `Retry-After` |
| 每设备 | B+ 认证成功后按 `device_id` 限速 | 429 |
| 每链接 | 分享读写/上传/confirm 按 `share_links.id` 限速；`drop` 更严 | 429 |
| 认证失败 | 同 IP/设备连续失败计数，超阈值临时锁定（指数退避） | 429 / 401 |
| 请求体 | `/api/notes` 6 MiB、笔记附件单文件 100 MiB、其它 JSON 64 KiB | 413 |
| 超时 | 读写超时 300s（反代同值）；`/ws` 空闲 10 分钟卸载 Doc | 关闭连接 |
| 分页 | list/搜索默认 50、最大 200；`since` 单次最多 500 | 400（越界） |

- 实现：进程内令牌桶（按 key 懒创建 + 定期清理），不依赖外部存储；单实例前提。
- 429 响应体只含 `{"error":"rate_limited"}`，不泄漏桶状态细节。
- **已实现（ADR 0007）**：`dev:<device_id>`（验签后）、`share:<link_id>`（token 命中后）与
  `ip:<peer>`（最外层中间件，`/healthz` 不计）三个维度；
  配置 `CISTA_RATE_LIMIT_PER_MIN` / `CISTA_RATE_LIMIT_BURST`；
  测试 `crates/server/src/rate_limit.rs`（7 项单元）与 `tests/ratelimit.rs`（4 项集成）。
- **已实现（认证失败退避）**：同 key（设备 id，缺省匿名桶）5 分钟内连续失败 5 次后
  按 1/2/4…（上限 60s）锁定，成功验签清零；实现于 `rate_limit::FailureTracker`，
  接入 `auth::require_signed`。
- **范围**：`drop` 链接更严额度与反向代理侧全局限速属部署侧配置与调参，应用层以 `ip:<peer>` 桶兜底。
- **已实现（分页硬上限）**：`GET /public/workspaces/files?limit=&offset=` 默认 50、最大 200
  （越界钳制而非 400），响应带 `next_offset`；Web 客户端 `listAllFiles()` 按 `next_offset` 翻页。
  测试：`tests/sharing.rs::workspace_file_list_is_paginated_and_capped`、`shareClient.test.ts` 2 项。

## 2. 日志

- 结构化 JSON（journald），字段：`ts, level, msg, route, status, bytes, duration_ms, device_hash?, link_hash?, ip_hash?`。
- **禁止**：正文、标题、文件名、MK/CK/FK、设备 token、分享 token、票据、passphrase、q。
- 允许：`token_fingerprint`（`sha256:` + 前 8 位）、IP 的 HMAC 截断。
- 轮转约 30 天（journald 侧配置）；不建审计表。
- 测试：`tests/logging.rs` 用捕获层断言响应/日志不含 token、passphrase、笔记正文标记。
  **已实现**：`crates/server/tests/logging.rs`（自定义 `MakeWriter` 捕获全部 tracing 输出，
  断言 token、口令、错口令、笔记标记均不出现，且捕获层确实收到日志）。

## 3. 每端点鉴权矩阵（必须全部有测试）

| 端点组 | 无凭据 | 设备 A → B 的对象 | 分享 token 越界 | 只读写 |
|---|---|---|---|---|
| `/api/notes*` | 401 | 404（未知 id 同形） | 401 | — |
| `/api/files` | 401 | — | 401 | — |
| `/api/share*` | 401 | 404 | 401 | — |
| `/api/workspaces*` | 401 | 404 | 401 | — |
| `/public/*` | 401 | 404（伪造 token） | 404（对象不匹配） | 403 |
| `/ws` | 关闭 | 关闭 | 关闭 | 丢弃 update |
