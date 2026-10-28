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

### 工程 / 开源
- 补 `CODE_OF_CONDUCT.md`、`.github/PULL_REQUEST_TEMPLATE.md`、feature request 模板、
  issue 模板配置、`.github/CODEOWNERS`、`.editorconfig`、`docs/README.md` 文档索引；
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

## 安全修复（2026-10 审计）

### 服务端
- 认证前按路由限制请求体并加并发预算，`/ws` 设置帧/消息上限；awareness 与同步帧有字节上限，
  广播改为 `Arc` + 32 深度队列。
- 分享上传改为「预留配额 → 落盘 → 记账」，失败清理；GC 增加孤儿 blob 扫描与磁盘水位。
- `/ws` 分享握手强制校验口令（Argon2 移入 blocking 线程池并限并发）；SSE 同样受保护。
- 反代部署按 `CISTA_TRUST_PROXY` 采信回环地址的 `X-Forwarded-For`；失败退避键加入 peer。
- nonce 重放缓存按规范字节建键（大小写变体不再绕过）；bootstrap token 一次性消费。
- 工作区 `expires_at` 立即生效（公网 410）；GC 将到期工作区转入隔离区；
  workspace 链接拒绝 `max_uses`（无可靠计数语义）。
- 下载改为分块流式解密；删除/归档的 refcount 与配额更新加并发保护；票据每链接上限 8 张。
- `CISTA_STATIC_KEY` 缺失时默认拒绝启动（`CISTA_ALLOW_PLAINTEXT=1` 仅限开发）。

### 客户端 / 部署
- Android 敏感键（设备种子/MK）在 Keystore 不可用时 fail-closed，并清理旧版明文条目。
- Tauri CSP 允许 `https:`/`wss:` 与本地开发源，原生端可连接用户填写的服务器。
- 换服务器时清除设备种子与 MK；配对页文案与实际密钥存放一致。
- Caddy 日志剔除 `Authorization`/`Cookie`/`X-Share-Token`/`X-Share-Passphrase`，
  分享面响应加 `default-src 'none'; sandbox`，站点 CSP 加 `frame-ancestors 'none'`。
- systemd/备份 unit 加固（UMask、ProtectSystem、CapabilityBoundingSet 等），compose 只读根、
  `cap_drop: ALL`、内存/PID 限制；新增 `cista-gc.{service,timer}`。
- Android 签名不再把口令写进 `build.gradle.kts`（改用 apksigner `env:`/`keytool -storepass:env`），
  删除 `scripts/android-signing.py`。

### 审计后补强
- 单文件下载票据改为**单次使用**（并发下载只有一个成功；需要重试时重新 confirm 换票）。
- `/api/devices/enroll` 接入失败退避（按反代客户端地址），bootstrap 失败不再无限重试。
- 实例配额按物理副本释放：删除/归档只释放工作区配额，避免与链接回收重复扣减。
- IP 限速按路径首段分桶（`/api`、`/public`、`/ws`），单一路由洪泛不再挤占其他面。
- GitHub Actions 全部固定到 commit SHA（附版本注释），dependabot 继续跟踪。

### 第二轮审计修复
- 认证前 body 预算加 15s 超时（慢速 16 连接不再饿死 `/api`）。
- Yjs 改为「先 apply 再落库」，坏 update 不会毒化工作区；票据改为下载成功时消费次数（confirm 只签票）。
- 客户端 IP 解析统一：仅可信反代对端（回环/私有/链路本地）采信 `X-Forwarded-For` 末段；
  enroll 退避与签名退避共用同一口径。
- 流式下载区分干净 EOF 与帧头截断；私有上传临时文件名随机化。
- Tauri CSP 真正放开 `https:`/`wss:`；桌面端敏感键回退到宿主 keyring，Android 仍 fail-closed；
  Android 启动时清理旧版明文密钥；删除操作不受 fail-closed 限制。
- SSE 端点校验口令并由 Caddy 立即 flush。
- 部署：systemd 加 `WorkingDirectory`、备份 unit 可读写数据目录并在快照前先 gc、
  `deploy/{cista,backup}.env.example`、Docker `init-data` 修正 `./data` 属主、Caddy >=2.7 说明。
- CI：修复 CR 剥离语法、PR 不执行签名、Tauri CLI 固定 2.12.4。
- 去重上传不再重复计实例配额；GC 清理超过 1 小时的 `.tmp` 残留。
- Android：Keystore 插件校验键名白名单；CI 在生成的 manifest 上关闭 `allowBackup` 与明文流量。
- Web：`index.html` 加 `referrer: no-referrer`。
