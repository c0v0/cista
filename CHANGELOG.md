# CHANGELOG

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
- Tauri 2 壳嵌入同一 SPA（编译与完整门禁已验证）

### 工程

- `./scripts/verify.sh`：web build → fmt → clippy → test → deny → web tests
- 恢复脚本与演练种子/校验用例（`tests/drill.rs`）
- 跨语言 KAT（`cargo run -p cista-core --example vectors`）保证 JS 与 Rust 加密/签名一致
