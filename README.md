# cista（私匣）

自托管多端笔记：Rust 服务端（axum + SQLite 双库：`private.db`/`shared.db`）+ Tauri 2 客户端（React SPA）+ Android；Web 临时端。
私有内容 E2E（服务端不可读）；共享内容服务端可读（静态加密）；归档后转私有。单租户、单实例。

设计、决策与验收标准见 `docs/design.md`、`docs/specs/`、`docs/adr/`。

## 快速开始

```bash
cp .env.example .env            # 至少设置 CISTA_BOOTSTRAP_TOKEN / CISTA_STATIC_KEY
cargo run -p cista-server       # 默认 127.0.0.1:8787，数据在 ./data
./scripts/verify.sh             # 本地与 CI 共用的门禁（fmt/clippy/test/deny/web）
```

配置全部走环境变量；没有管理 API/后台，运维只走 SSH。

## 部署与运维

- 默认形态：单二进制 + systemd（`deploy/cista.service`），Docker 只是可选包装。
- 反代：`deploy/Caddyfile`（TLS/HSTS、110MB 请求体上限、WS 不缓冲、JSON 日志）。
- 备份：`scripts/backup.sh`（`VACUUM INTO` 两份库 + blob 清单 + restic），
  由 `deploy/cista-backup.{service,timer}` 每日触发；失败进 journal。
- 恢复演练：`scripts/restore-drill.sh <空目录>`，记录模板见 `docs/specs/restore-drill-log.md`；
  人工部分（MK 解密一条笔记）必须补上。
- 可用性：外部 uptime 每 5 分钟打 `/healthz`。
- **告警接收人 = 所有者本人：`owner@example.invalid`（部署时改成真实邮箱/手机）。**
  没有 uptime 告警之前，不开放公网分享。
- 日志：结构化 JSON 到 journald/Docker logs，轮转约 30 天；不记正文、不记 token 明文
  （分享 token 只出现在 `Authorization: Share` 头里，且日志只写 `sha256:` 前 8 位指纹）。

## 三把钥匙（分开保存）

| 钥匙 | 丢失后果 | 存放 |
|---|---|---|
| MK（私有主密钥） | 私有数据永久不可恢复 | 恢复码离线 |
| 静态加密密钥（共享 blob） | 共享 blob 无法解密 | 离线托管 |
| 备份仓库密码 | 所有备份打不开 | 离线托管 |

## 结构

```text
crates/core     协议/加密/B+ 认证/同步状态机/客户端索引（无 IO，可 wasm）
crates/server   axum API、私有库 + 共享库、共享 blob
apps/desktop    Tauri 2 原生壳（复用 web/）
web/            React + Vite + TS SPA
scripts/        verify.sh、backup.sh、restore-drill.sh
deploy/         systemd unit/timer、Caddyfile
```
