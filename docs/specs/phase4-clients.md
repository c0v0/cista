# 规格：Phase 4 —— 客户端（桌面 / Web 临时端 / Android）与本地搜索

> 验收来源：`design.md` §11「Phase 4：客户端搜索 + Web 临时端 + Android」→
> 「搜索附件→定位所属笔记；Web 索引不落盘；配对→内存密钥；Android 真机 + 旧版本升级；Keystore 可用」。

## 目标 / 非目标

- 目标：桌面（Tauri 2）与 Web 临时端共用同一 React SPA；本地文件索引与统一搜索；设备配对（B+）与密钥存放。
- 非目标：离线同步全量落地（桌面真正离线队列按需加）；Android 全功能（只做 spike 结论与最小闭环）。

## 0. 进度（2026-10-24）
- **已完成**：
  - `cista-core::search`（LIKE 子串、转义、大小写不敏感、默认 50/最大 200）与其 Web 等价实现
    `web/src/lib/searchIndex.ts`（内存、不落盘；两套各自单测，语义一致）。
  - 分享客户端 `web/src/lib/shareClient.ts`（token 只进 header、运行时校验响应）与访客分享页
    `web/src/pages/SharePage.tsx`（文件链接两步下载、工作区列表/下载/上传、口令、fragment 路由）。
  - 服务端跨域白名单 `CISTA_CORS_ORIGIN`（`design.md` §4.2），预检与实际请求均有测试。
  - **桌面壳可编译**：`cargo build -p cista-desktop` 通过，完整 `./scripts/verify.sh`（workspace 含 desktop）
    为绿；补了缺失的 Tauri 图标与 `scripts/gen-icons.py`，并修正 `verify.sh` 顺序
    （先构建 `web/dist`，再跑 clippy/test，否则 `frontendDist` 不存在会导致编译失败）。
  - **配对 + 笔记编辑器**：`web/src/lib/{authClient,noteCrypto,notesApi}.ts` 与
    `web/src/pages/{PairPage,NotesPage}.tsx`。B+ 签名素材、XChaCha20-Poly1305 AAD、base64 格式
    与 Rust 用 KAT 向量逐字节对齐（`cargo run -p cista-core --example vectors`）；
    MK/设备私钥只存在内存，保存带 `If-Match`，409 只提示不覆盖。
  - 端到端（无浏览器）：`web/src/lib/notesApi.e2e.test.ts` 在真实服务端上跑
    配对 → 加密保存 → 解密读取 → 冲突 → MK 隔离。
- **未完成**：Android（方案与解除条件见 `docs/adr/0008-android-spike.md`，缺 SDK/NDK 与真机）；
  桌面/移动原生能力（文件选择、Keychain/Keystore）未接。
- **验收方式**：逻辑与协议用单元测试 + 跨语言 KAT + 真服务端 e2e 覆盖；
  布局与交互的目视验收属于发布检查（`phase6-hardening.md` §4），不写入仓库。

## 1. 本地文件索引（`design.md` §2.5）

```sql
-- 桌面/手机本地 SQLite；Web 用内存实现；不同步、不上传、可全量重建
file_index(source_type TEXT, source_id TEXT, note_id TEXT NULL, name TEXT, note_title TEXT NULL, updated_at INTEGER)
```

- `source_type`：`note_attachment`（v1 只写这个）| `drive_node`（预留；v1 不建私有网盘表，也不写这个值）。
- 只索引私有内容；只存文件名/标题/ID，**不存正文**。
- 查询：`LIKE` 子串匹配，转义 `%`、`_`、转义符本身（`\`）；`source_type` 与 `name` 大小写不敏感。
- 增量：附件新增/改名/删除、笔记改名/删除时同步更新；`rebuild()` 可全量重建。
- 排序：`updated_at DESC, name ASC`；分页：默认 50、最大 200。

| 统一搜索行为 | 规则 |
|---|---|
| 默认范围 | 只查本地私有索引，不发网络请求 |
| 切到共享 | 用户显式切换 → `POST /api/search/shared`，`q` 在 body；只搜 active 工作区标题与文件名 |
| 离线 | 只返回本地私有结果（共享结果置空并提示离线） |
| 日志 | 本地搜索不写日志；共享搜索 q 不进 URL、不进服务端访问日志 |

## 2. 密钥与配对

- 桌面/手机：Ed25519 设备私钥放 OS keychain/Keystore；MK 放 keychain，不进 localStorage/IndexedDB/cookie。
- Web 临时端：浏览器生成临时密钥对（内存）→ 扫码由已入网设备签名入网 → MK 只在内存；
  15 分钟会话；刷新即重配对；**不落盘任何长期密钥**。
- 配对流程：`POST /web/pair/start`（二维码：公钥 + nonce）→ `POST /web/pair/approve`（已入网设备签名）→
  `POST /web/pair/session`（临时会话标识 + MK 传输）。

## 3. 验收标准（可执行）

1. `pnpm -C web test --run` 覆盖：
   - `searchLocal("根 目录")`：`%`/`_`/`\` 被当作字面量（插入名含 `100%_draft` 的记录，用 `%_` 查询能命中，用其他查询不误命中）；
   - 空查询返回空；大小写不敏感；`drive_node` 记录能被查到但 `note_id` 为空；
   - 删除附件后查询不再返回该行（无死链）；
   - 排序：`updated_at` 新的在前；
   - 分页：默认 50、上限 200 被强制。
2. `pnpm -C web test --run` 覆盖共享搜索开关：默认范围不调用网络层（注入的 transport 记录调用次数为 0）；
   切换后 `q` 出现在 body 且 URL 不含 `q`。
3. `web` 端存储检查：测试断言 `localStorage`/`sessionStorage`/`indexedDB` 未被写入密钥字段（密钥只走内存对象）。
4. 桌面：`cargo build -p cista-desktop` 成功；`cargo tauri build` 产出 bundle（Linux 本地验证）。
5. Android spike（独立分支，结论写入 `docs/adr`）：
   `cargo tauri android init` + 构建到模拟器/真机；WebView 调 Rust command 返回字符串；
   私有目录读写；Ed25519 生成/存 Keystore + 签名往返；连本地 `/ws` 应用一次 update；后台存活 10 分钟结论。
6. `./scripts/verify.sh` 全绿。

## 影响面

| 面 | 改/不改 |
|---|---|
| web | 改：搜索索引、搜索 UI、配对页、share 访客页 |
| desktop | 改：keychain、本地 SQLite 索引、附件 IO、Tauri command 暴露 core |
| android | 改：spike 分支（结论） |
| server | 改：`/web/pair/*` 端点（配对与临时会话） |
| core | 改：`search`（纯逻辑：转义/匹配/排序）、`protocol` 增加配对载荷类型 |

## 需要的 ADR

- 0007：客户端搜索索引的存储选择（桌面 SQLite / Web 内存）与「不上传、不同步」边界的验证方式。
- 0008：Web 临时端密钥生命周期（内存 + 15 分钟会话 + 刷新重配对的取舍）。

## 待确认

- UI 组件库、Markdown 编辑器（CodeMirror 6 候选）；Android 构建环境（JDK/SDK/NDK 版本）。
