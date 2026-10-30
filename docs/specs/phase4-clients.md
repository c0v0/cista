# 规格：Phase 4 —— 客户端（桌面 / Web 临时端 / Android）与本地搜索

> 验收来源：`design.md` §11「Phase 4：客户端搜索 + Web 临时端 + Android」→
> 「搜索附件→定位所属笔记；Web 索引不落盘；配对→内存密钥；Android 真机 + 旧版本升级；Keystore 可用」。

## 目标 / 非目标

- 目标：桌面（Tauri 2）与 Web 临时端共用同一 React SPA；本地文件索引与统一搜索；设备配对（B+）与密钥存放。
- 非目标：离线同步全量落地（桌面真正离线队列按需加）；Android 全功能（只做 spike 结论与最小闭环）。

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
| web | 改：搜索索引、搜索 UI、配对页、分享访客页、服务器地址设置 |
| desktop | 改：Tauri 壳与命令桥（`secret_save/load/delete`），复用 `web/` |
| android | 改：Tauri mobile + `plugins/keystore`（AndroidKeyStore） |
| server | 不改：设备入网/吊销在 Phase 1 已就绪 |
| core | 不改：`search` 与配对协议已在 Phase 1/3 就绪 |

## 关联 ADR

- 0002：B+ 逐请求签名（不做 bearer 双栈）。
- 0006：Yjs 持久化顺序（客户端编辑器与 `/ws`）。
- 0008：Android 客户端方案（Tauri mobile + AndroidKeyStore）。

## 实现选择

- UI：手写 React + 原生控件，不引入组件库。
- 编辑器：`<textarea>` 纯文本编辑（Markdown 原文），不做渲染预览。
- 本地索引：Web/桌面均为内存索引，不落盘。
- Android：JDK 17、Android platform 36、build-tools 36.0.0、NDK 26.3.11579264（CI 安装）。
