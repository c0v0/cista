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
- 代码层补强：nonce 缓存硬上限；WS 校验 Origin；空闲 Yjs 文档与广播通道回收；
  私有附件改为流式落盘 + 摘要验签（100 MiB 上限，不再整包进内存）；
  Android 宿主彻底移除明文密钥读写（仅保留旧文件清理）；Keystore 键名白名单。
- 下载票据改走 `X-Share-Ticket` 头（URL/日志不再出现票据）；Caddy 日志同步剔除该头。
- 签名失败退避只按客户端地址计数，未验签的设备 id 不再能用来锁死他人设备。
- WS 广播队列降到 16，慢客户端落后即断开重连（避免内存堆积）。
- 新增 Tauri `capabilities/default.json`（core:default）；Web 生产构建注入 CSP meta；
  Caddy CSP 允许 `https:`/`wss:` 以支持运行期填写的跨域服务器地址。
- CORS 预检放行 `X-Share-Ticket`/`X-Share-Token`，跨域单文件下载恢复。
- Android Keystore 链路打通：注册 `register_android_plugin`、新增插件 `permissions/default.toml`、
  capability 授予 `cista-keystore:default`、Kotlin 命令改为 Tauri 派发使用的 camelCase。
- 私有附件下载改为分块流式；Caddy 版本要求改为 >=2.10（`request_body`）。
- Docker 镜像不再默认信任反代头（仅 compose 的 127.0.0.1 映射开启）；备份 unit 设 `HOME=/srv/cista`
  并给出可执行的 ssh 密钥放置方式；README 安装步骤补全二进制/依赖/`/etc/cista`。
### 第三轮审计修复
- Yjs 快照改为「先读 max(seq) 再编码状态」，消除并发 update 被删但未入快照的丢失窗口。
- 文档大小按已应用 update 字节数增量校验（5 MiB），不再只在快照时检查；快照后重置。
- 空闲回收在持有 active 锁期间完成卸载，避免 TOCTOU 双文档分叉。
- 私有附件上传增加磁盘水位、设备限速；GC 清理私有目录的 `.upload-*.tmp` 残留。
- WS/SSE 增加全局连接上限；WS 每连接 240 帧/分钟限速；无未快照更新时关闭不写快照。
- 部署文档/配置：Caddy 2.10、Docker 默认不信任反代头（仅 compose 开启）、备份 HOME/ssh 密钥路径、
  README 安装步骤补全。
### 第四轮审计修复（发布候选）
- `/api` 在占用 body 预算前做设备/时间戳/nonce/签名形状预检，并把失败退避检查前移；
  未认证慢速请求不再能饿死已签名请求。
- 单文件分享创建串行化（`link_create_budget=1`），Argon2 移入 blocking 线程池；
  共享副本写入移出 SQLite 事务；链接插入失败时补偿 refcount/实例配额。
- 公网慢上传加 300s 墙钟超时与 2 并发上限；上传前要求 `水位 + 8 × 单文件上限` 可用空间。
- 上传事务内复核工作区仍 active，归档/删除后不能再插入文件或配额。
- `uploads` 增加稳定 `file_id`（迁移 `20261030000100`），删除文件只释放自己的链接配额。
- `restore` 只允许 expired/quarantined/已到期 active；归档/删除会撤销工作区链接并卸载内存 Yjs 文档；
  状态变更与清库在同一事务，堵住 purge 后打开中的 WS 再写回密文的窗口。
- 单文件链接强制 `FILE_READ`；SSE 要求 `FILE_READ`，加每链接 4 条上限并周期复核。
- WS 拆分握手/会话预算，增加每链接 8 / 每设备 16 条上限；撤销/过期复核缩短到 10s。
- 票据下载先确认 blob 存在，再消费票据/次数。
- 所有 API/分享响应 `Cache-Control: no-store`；Caddy 仅在对应 handle 内设置 CSP，避免覆盖更严格的值。
- Docker 入口 `umask 0077`；`gc`/`reset-bootstrap` 不再要求静态密钥，备份单元无需保存第二份密钥。
- 发布 job 缺少 GPG 密钥时失败而不是发布未签名产物；Debian 打包 job 改用固定 Node 版本 + SHA256，
  不再 `curl | bash` NodeSource。
- 新增 `cista-server reset-bootstrap`（SSH 运维）用于 Web 临时端/第二台设备重新配对。
