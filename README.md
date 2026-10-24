# cista（私匣）

自托管多端笔记：Rust 服务端（axum + SQLite 双库：`private.db`/`shared.db`）+ Tauri 2 客户端（React SPA）+ Android；Web 临时端。
私有内容 E2E（服务端不可读）；共享内容服务端可读（静态加密）；归档后转私有。单租户、单实例。

设计、决策与验收标准见 `docs/design.md`、`docs/specs/`、`docs/adr/`。

## 当前状态（2026-10-24）
已完成的部分：

| 阶段 | 内容 | 证据 |
|---|---|---|
| Phase 0 | 工作区、CI 草案、ADR 0001–0009、Phase 规格、门禁脚本 | 仓库结构 |
| Phase 1 | `cista-core` 加密/签名/同步状态机、双库、笔记 CRUD + 增量同步、B+ 中间件 | core 37 + notes 8 + authz 10 + healthz 3 |
| Phase 2 | 分享链接（file/workspace/口令/次数/过期）、两步下载票据、410、配额、共享库隔离、共享 blob 静态加密 | sharing 16 项 |
| Phase 3 | Yjs 文档、快照 + 尾部持久化、`/ws` 握手（分享/所有者）、只读丢弃 | yjs 7 项 + ws_e2e 1 项 |
| Phase 5（部分） | 归档/隔离区、恢复演练（restic 实跑）、`cista-server gc` | drill 记录、gc 1 项 |
| Phase 6（部分） | IP/设备/链接限速 + 认证失败退避、分页硬上限、共享上传流式写入、日志脱敏、依赖策略、CORS | ratelimit 4 + sharing 16 + logging 1 + cors 3 + deny ok |
| Web/桌面 SPA | 访客分享页、分享客户端、本地搜索索引；**配对 + 笔记编辑器**（B+ 签名、MK/CK 端到端、MK 仅内存） | web 58 项（含 KAT 互操作）+ 真服务端 e2e + vite build |
| 门禁 | `./scripts/verify.sh`（web build → rustfmt → clippy → test → deny → web tests） | 脚本即协议 |

**桌面端（Tauri 2）**：壳已接通 `web/dist`；编译需要 GTK/WebKit 开发包
（`libwebkit2gtk-4.1-dev`、`libgtk-3-dev` 等，见 `apps/desktop`）。

**尚未完成**：
- **Android**：方案与结论见 `docs/adr/0008-android-spike.md`；缺 SDK/NDK 与真机，未验证，
  发布检查该项保持未通过。
- **桌面/移动原生能力**：系统钥匙串桥已实现（Tauri 命令 `secret_save/load/delete` + Web 侧
  `native.ts`），只允许存**设备种子**，MK 永不入系统凭据库；尚未接到 UI。
  文件选择器与安装包打包未做；Web 端密钥仍只在内存，刷新即锁定。
- **Phase 6 剩余**：外部安全评审（未通过不开放公网分享）、反向代理侧全局限速
  （应用层 `ip:<peer>` 桶已兜底）。

测试规模：Rust **116 项**（另有 2 个 `#[ignore]` 演练用例，见 `docs/specs/restore-drill-log.md`），
Web **64 项**（另有 1 项真服务端 e2e，设置 `CISTA_E2E_URL` / `CISTA_E2E_BOOTSTRAP` 后运行）。

数据边界与删除流程见 `docs/privacy.md`。

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
- 迁移演练：`scripts/migration-drill.sh <空目录>` 在副本上做前向迁移，并验证迁移前快照仍可启动
  （等价于「回滚 = 换回旧二进制 + 恢复备份」的数据侧检查）。
- 共享 blob 静态加密：生产必须设置 `CISTA_STATIC_KEY`（32 字节 hex，与其它钥匙分开托管）；
  未设置时共享文件明文落盘，仅适合开发/测试（见 ADR 0009）。
- 升级 / 回滚：
  - 升级：停服务 → 备份 → 换二进制（`systemctl stop cista` → 替换 `/usr/local/bin/cista-server`
    → `systemctl start cista`），迁移在启动时前向执行。
  - 回滚：换回上一版二进制 **并** 恢复升级前的备份（迁移只前向，回滚以数据恢复为准）。
  - 版本与变更见 `CHANGELOG.md`。
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
