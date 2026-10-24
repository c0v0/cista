# 规格：Phase 6 —— 加固与发布

> 验收来源：`design.md` §11「Phase 6：加固与发布」→「每端点 A→B；恢复演练；外部评审 auth/E2EE；发布检查」。

## 目标 / 非目标

- 目标：把「单机可用」变成「可对外、可审计、可回滚」：限速、分页硬上限、日志脱敏、
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

## 2. 日志

- 结构化 JSON（journald），字段：`ts, level, msg, route, status, bytes, duration_ms, device_hash?, link_hash?, ip_hash?`。
- **禁止**：正文、标题、文件名、MK/CK/FK、设备 token、分享 token、票据、passphrase、q。
- 允许：`token_fingerprint`（`sha256:` + 前 8 位）、IP 的 HMAC 截断。
- 轮转约 30 天（journald 侧配置）；不建审计表。
- 测试：`tests/logging.rs` 用捕获层断言响应/日志不含 token、passphrase、笔记正文标记。

## 3. 每端点鉴权矩阵（必须全部有测试）

| 端点组 | 无凭据 | 设备 A → B 的对象 | 分享 token 越界 | 只读写 |
|---|---|---|---|---|
| `/api/notes*` | 401 | 404（未知 id 同形） | 401 | — |
| `/api/files` | 401 | — | 401 | — |
| `/api/share*` | 401 | 404 | 401 | — |
| `/api/workspaces*` | 401 | 404 | 401 | — |
| `/public/*` | 401 | 404（伪造 token） | 404（对象不匹配） | 403 |
| `/ws` | 关闭 | 关闭 | 关闭 | 丢弃 update |

## 4. 发布检查（逐条给命令与输出）

```text
./scripts/verify.sh 全绿（贴原始输出）
gitleaks detect --no-git -s .（或人工 grep）0 命中
前端 bundle 无密钥/sourcemap/.env：pnpm -C web build && grep -rIn "MK\\|PRIVATE KEY\\|sourceMappingURL" web/dist
备份 timer 已跑；scripts/restore-drill.sh 通过并记录耗时
迁移回滚演练：副本上前向 + 独立回滚路径
每端点 A→B / 撤销 401 / 只读 403 用例全绿
外部 uptime 测试告警到达所有者
Yjs 双客户端/离线并发/重连/重启收敛测试通过
Android 真机安装 + 旧版本升级，记录 commit/build ID
外部评审 auth/E2EE 通过（未通过不开放公网分享）
版本号 / CHANGELOG / 回滚命令（systemctl + 上一版二进制）
```

## 5. 已知缺口（进入发布前必须关闭或有书面豁免）

1. **过期单文件链接的共享副本回收**：v1 只在访问时 410，不自动删副本（ADR 0004）。
   计划：`cista-server gc --links` 一次性命令（SSH 执行），批量撤销过期链接并释放引用。
2. **私有 blob 未计入实例配额**：只在共享面记账（ADR 0004）。计划：上传 `/api/files` 时
   按内容去重记 `blobs(scope='private')` 用量。
3. **上传在内存缓冲**：100 MiB × 并发数会顶到 `MemoryMax`。计划：流式写入 + 硬截断。
4. **Range 请求**：下载不支持断点续传的 `Range`（反代可关）。
5. **Android/桌面构建**：依赖开发机 JDK/SDK 与 WebView2，需在 CI 上验证。

## 影响面

server（限速中间件/日志/GC 子命令/流式上传）、web（错误码 UI）、deploy（journald 轮转、反代超时）、
docs（发布清单证据）。

## 需要的 ADR

- 0007：限速算法与 key 选择（进程内令牌桶，单实例）。
- 0008：日志脱敏字段与哈希口径（token 指纹、IP HMAC）。
