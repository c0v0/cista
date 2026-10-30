# CHANGELOG

## [Unreleased]

### 客户端
- 新增「连接服务器」初始化页（Web / 桌面 / Android 共用）：原生端首次启动强制填写地址，
  可「测试连接」（`GET /healthz`）、选同源（Web）、随时在页脚/配对页更换；地址存
  `localStorage:cista.server.base`，换服务器会锁定当前会话。
- 分享落地页支持跨域部署（`ShareClient` 接收 `apiBase`）。

### 部署
- `deploy/Caddyfile` 同时托管 `web/dist`（`try_files` 回退 `index.html`）与反代
  `/api/*`、`/public/*`、`/healthz`、`/ws`，并加 CSP/HSTS 头。

### 工程
- 补 `.editorconfig`、`docs/README.md` 文档索引；
  README 增加 CI/Release/License 徽章与「客户端连接服务器」章节。
- 部署：新增多阶段 `Dockerfile` 与 `compose.yaml`（`docker compose up -d --build`），CI 增加 docker job 构建镜像并冒烟 `/healthz`。
- 文档：新增英文 `README.en.md` 与语言切换；修正 design 中的 `/web` 路由与 `notes` 服务名。
- Release 只挂可执行产物（服务端二进制、Debian/Ubuntu 两个 deb、AppImage、NSIS 安装包、签名 APK），
  并附 `SHA256SUMS` 与 GPG 分离签名（`SHA256SUMS.asc`、每个 deb 的 `.asc`）和公钥；不再附带 `web/dist`
  的 html/js/css；`web` job 仅保留 typecheck/test/build 验证。

## [0.1.0] - 2026-10-24

首个可用版本：单实例、单租户、自托管。

### 服务端 / core

- 私有笔记端到端加密（XChaCha20-Poly1305，MK 包裹 CK，AAD 绑定 note_id + rev，恢复码）
- B+ 设备认证（Ed25519 逐请求签名、±120s 时间窗、nonce 防重放），bootstrap 入网与设备吊销
- 私有库 / 共享库分库；分享路由只持有共享库连接
- 分享链接（单文件 / 工作区、口令、次数、过期）、两步下载票据、410 语义、三层配额
- 共享 blob 静态加密（分块 AEAD，流式写入）
- Yjs 文档：快照 + 尾部持久化、`/ws` 握手（分享 / 所有者）、只读连接丢弃 update
- 归档 / 隔离区 / 恢复、`cista-server gc` 回收过期链接与共享副本
- 限速（IP / 设备 / 链接）与认证失败指数退避、文件列表分页硬上限
- 结构化日志与脱敏测试；跨域白名单

### Web / 桌面

- 访客分享页（文件 / 工作区、口令、下载 / 上传）
- 配对与笔记编辑器（B+ 签名、MK/CK 端到端，密钥仅内存）
- 本地搜索索引（内存、不落盘），与 core 语义一致
- 私有附件：客户端加密（随机 FK，MK 包裹）→ 内容寻址上传 → 编辑器内下载解密
- Tauri 2 壳嵌入同一 SPA（编译与完整门禁已验证）

### 工程

- `./scripts/verify.sh`：web build → fmt → clippy → test → deny → web tests
- 恢复脚本与演练种子/校验用例（`tests/drill.rs`）
- 跨语言 KAT（`cargo run -p cista-core --example vectors`）保证 JS 与 Rust 加密/签名一致
