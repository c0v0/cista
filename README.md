# cista（私匣）

简体中文 | [English](README.en.md)

[![ci](https://github.com/c0v0/cista/actions/workflows/ci.yml/badge.svg)](https://github.com/c0v0/cista/actions/workflows/ci.yml)
[![build](https://github.com/c0v0/cista/actions/workflows/build.yml/badge.svg)](https://github.com/c0v0/cista/actions/workflows/build.yml)
[![release](https://img.shields.io/github/v/release/c0v0/cista?include_prereleases&sort=semver)](https://github.com/c0v0/cista/releases)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

自托管多端笔记：Rust 服务端（axum + SQLite 双库：`private.db`/`shared.db`）+ Tauri 2 客户端（React SPA）+ Android；Web 临时端。
私有内容 E2E（服务端不可读）；共享内容服务端可读（静态加密）；归档后转私有。单租户、单实例。

设计、决策与验收标准见 `docs/design.md`、`docs/specs/`、`docs/adr/`，文档导读见 [`docs/README.md`](docs/README.md)。

## 功能

| 模块 | 内容 |
|---|---|
| 服务端 | axum + 两份 SQLite（`private.db` / `shared.db`），单二进制；分享路由只持有共享库连接 |
| 私有笔记 | B+ 逐请求签名、XChaCha20-Poly1305 端到端加密、`If-Match` 并发控制、增量同步、设备入网/吊销 |
| 分享 | 单文件 / 工作区、可选口令、次数与过期、两步下载票据、只读强制、三层配额、共享 blob 静态加密 |
| 协作 | Yjs 文档、`/ws` 握手（所有者/分享）、只读连接丢弃 update、快照 + 尾部持久化 |
| 运维 | 归档/隔离区/恢复、`cista-server gc`、限速与认证退避、结构化脱敏日志、CORS 白名单 |
| 客户端 | Web / Linux / Windows / Android 共用一套 SPA：首次启动填服务器地址、配对、笔记编辑器、附件、本地搜索索引（内存、不落盘） |
| 原生能力 | Tauri 2 壳 + `plugins/keystore`：桌面走 Secret Service / Credential Manager，Android 走 AndroidKeyStore；只允许存设备种子，MK 不入系统凭据库 |

测试规模：Rust **123 项**（另有 2 个 `#[ignore]` 演练用例，见 `docs/specs/restore-drill-log.md`），
Web **96 项**（另有 1 项真服务端 e2e，设置 `CISTA_E2E_URL` / `CISTA_E2E_BOOTSTRAP` 后运行）。
门禁 `./scripts/verify.sh`：web build → rustfmt → clippy → test → deny → web tests。

数据边界与删除流程见 `docs/privacy.md`。

## 快速开始

```bash
cp .env.example .env            # 至少设置 CISTA_BOOTSTRAP_TOKEN / CISTA_STATIC_KEY
cargo run -p cista-server       # 默认 127.0.0.1:8787，数据在 ./data
./scripts/verify.sh             # 本地与 CI 共用的门禁（fmt/clippy/test/deny/web）
```

配置全部走环境变量；没有管理 API/后台，运维只走 SSH。

## 客户端连接服务器

桌面端与 Android **首次启动必须先填服务器地址**（原生 WebView 没有「当前站点」可依赖），
Web 端默认同源、也可改成跨域地址：

1. 启动后进入「连接服务器」页，填 `https://<你的域名>`（可带路径前缀，如 `https://example.com/cista`）；
2. 「测试连接」会请求 `<地址>/healthz`，返回 `ok` 才通过；
3. 地址存本机 `localStorage` 的 `cista.server.base`（非秘密），换服务器会锁定当前会话；
4. 之后在页脚「更改」或配对页「更换服务器」重新设置。

自建 Web 也可以把默认地址烧进构建：`VITE_API_BASE=https://cista.example.com pnpm -C web build`。
同源部署时 Caddy 需要同时托管 `web/dist` 与反代 API，见 `deploy/Caddyfile` 的 `handle` 块。

## 部署与运维

- 默认形态：单二进制 + systemd（`deploy/cista.service`）。Docker 也支持：
  `cp .env.example .env` → `sudo install -d -o 10001 -g 10001 ./data` → `docker compose up -d --build`
  （镜像见 `Dockerfile`，容器以 uid 10001 运行，数据挂载到 `./data`）。
- 反代：`deploy/Caddyfile`（TLS/HSTS、110MB 请求体上限、WS 不缓冲、JSON 日志）。
- 备份：`scripts/backup.sh`（`VACUUM INTO` 两份库 + blob 清单 + restic），
  由 `deploy/cista-backup.{service,timer}` 每日触发；失败进 journal。
- 恢复演练：`scripts/restore-drill.sh <空目录>`，记录模板见 `docs/specs/restore-drill-log.md`；
  人工部分（MK 解密一条笔记）必须补上。
- 迁移演练：`scripts/migration-drill.sh <空目录>` 在副本上做前向迁移，并验证迁移前快照仍可启动
  （等价于「回滚 = 换回旧二进制 + 恢复备份」的数据侧检查）。
- 桌面打包：`scripts/package-desktop.sh [deb|deb,appimage]`（先构建 `web/dist`，再调 tauri CLI；
  产物在 `target/release/bundle/`）。安装包不提交进仓库。
- 自动构建：`.github/workflows/build.yml` 在 push/PR 时编译服务端（Linux/Windows）与桌面包；
  推送 `v*` tag 时把产物自动挂到 Release。
- 校验发布产物（从 Release 下载 `cista-release-public.asc`、`SHA256SUMS`、`SHA256SUMS.asc`）：
  ```bash
  gpg --import cista-release-public.asc
  gpg --verify SHA256SUMS.asc SHA256SUMS
  sha256sum -c SHA256SUMS --ignore-missing
  # 单独校验某个 deb 的签名
  gpg --verify Cista_0.1.0_debian13_amd64.deb.asc Cista_0.1.0_debian13_amd64.deb
  ```
  密钥指纹见 `cista-release-public.asc`；签名由 CI 在发布 job 内用仓库 secret 完成。
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
web/            React + Vite + TS SPA（含服务器地址设置页）
plugins/keystore  Tauri 移动端插件：Android Keystore 适配
docs/           design / privacy / adr / specs（阶段规格与验收）
scripts/        verify.sh、backup.sh、restore-drill.sh、打包与演练脚本
deploy/         systemd unit/timer、Caddyfile（静态站点 + API 反代）
.github/        CI、构建/发布、dependabot、issue/PR 模板、CODEOWNERS
```

## 参与与法律

- 贡献流程与门禁：`CONTRIBUTING.md`
- 行为准则：`CODE_OF_CONDUCT.md`
- 漏洞报告（请勿开公开 issue）：`SECURITY.md`
- 变更记录：`CHANGELOG.md`
- 许可证：MIT（见 `LICENSE`）

## 公网部署安全基线（审计后新增）

- **必填 `CISTA_STATIC_KEY`**：未设置时服务端拒绝启动；仅本地开发可显式设 `CISTA_ALLOW_PLAINTEXT=1`。
- **反代后必须设 `CISTA_TRUST_PROXY=1`**（systemd unit 已内置）：只对来自回环地址的请求采信
  `X-Forwarded-For`，否则所有客户端会共用一个限速桶。
- **Caddy 访问日志已剔除凭据头**（`Authorization`/`Cookie`/`X-Share-Token`/`X-Share-Passphrase`）；
  部署后可用 `grep -c '"Authorization"' /var/log/caddy/cista.log` 应得到 0。
- **GC 定时器**：`deploy/cista-gc.{service,timer}` 每小时清理过期链接/票据、把到期工作区转入隔离区，
  并删除无数据库行的孤儿共享 blob；不装定时器时磁盘与配额会持续漂移。
- **磁盘水位**：`CISTA_MIN_FREE_BYTES`（默认 512 MiB），低于该值上传直接 507。
- 上传/下载均为流式：单请求内存占用与文件大小无关（下载分块解密）。
