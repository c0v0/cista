# 规格：Phase 2 —— 共享工作区、文件区、单文件链接

> 验收来源：`design.md` §11「Phase 2：共享工作区 + 文件区 + 单文件链接」→
> 「只读链接不能写；上传配额并发正确；过期 410；文件/链接 A→B 403/404」。

## 目标 / 非目标

- 目标：所有者把私有内容投影/分叉成共享工作区；访客凭 capability token（可选口令）读/写/上传；
  单文件链接；服务端强制权限、配额、过期与撤销。
- 非目标：Yjs 正文（Phase 3）；归档与过渡区人工处理流程（Phase 5，但本阶段落 schema：`state` 含 `quarantined`）；
  私有网盘；共享文件树（v1 扁平，schema 保留 `parent_id`）。

## 1. 权限模型（`design.md` §3.3）

内部 bitmask（存储用 bit，UI 用预设）：

```text
note:  off | ro | rw        -> bit: NOTE_READ=1<<0, NOTE_WRITE=1<<1
files: off | ro | rw | rwm | drop
        ro=FILE_READ(1<<2)  rw=+FILE_UPLOAD(1<<3)  rwm=+FILE_MANAGE(1<<4)
        drop=FILE_DROP_UPLOAD_ONLY(1<<5)
```

- `FILE_MANAGE` 依赖 `FILE_READ`；`NOTE_WRITE` 依赖 `NOTE_READ`；服务端在创建链接时校验预设组合，
  在每次请求时按 bit 判定（`drop` 不做目录列举）。
- 只读强制在服务端：不带 `FILE_UPLOAD` 的上传 → 403；不带 `FILE_MANAGE` 的改名/删除 → 403；
  `note=ro` 的 Yjs 连接丢弃 `update`（Phase 3 落实）。

## 2. 链接与 token（`design.md` §3.2）

- token：128-bit CSPRNG，**客户端生成**；服务端只存 `SHA-256(token)`（`share_links.token_hash`），
  明文 token 只在创建响应里出现一次。
- 链接 URL：`https://<frontend>/#/s/<token>`（单文件）、`/#/w/<token>`（工作区）；token 在 fragment。
- API 鉴权：`Authorization: Share <token>`；口令走单独 header `X-Share-Passphrase`（`drop` 默认必需）。
- 两步下载：`GET /public/files/{token}/meta`（落地页，不计数）→ `POST /public/files/{token}/confirm`
  （签发一次性 30–60s 票据；次数在下载成功时原子消费）→ `GET /public/files/{token}/blob?ticket=...`（流式）。
- 时限关系：`link.expires_at <= workspace.expires_at`。
- 状态：`active | expired | revoked | exhausted`；过期/撤销/用尽 → **410**（`410 Gone`）。
- 单文件链接过期只删共享副本（refcount 减），私有原件不动。

## 3. 配额与限制（`design.md` §9）

```text
instance_quota  = 0.80 × 卷容量
workspace_quota = 0.25 × instance_quota
link_quota      = 0.10 × instance_quota
max_file_bytes  = 100 MiB（可配）
disk_watermark  = 0.90 × 卷容量
```

- 上传前原子预留（DB 事务内检查 + 预留列），流式写入并硬截断（不信任 `Content-Length`）；
  超限 → 413 且不留半文件（临时文件删除、不写 blob、不增 refcount）。
- 水位触发时拒绝新上传，保留读取。

## 4. 上传安全（`design.md` §3.7）

- 白名单：图片、音频、视频、PDF、Office、纯文本、zip；禁止可执行/HTML/SVG/脚本（按 MIME + 扩展名双判）。
- 永不内联：响应带 `Content-Disposition: attachment; filename="..."`、`X-Content-Type-Options: nosniff`、
  `Content-Security-Policy: default-src 'none'`。
- 文件名清洗：去路径分隔符与控制字符，限长；同工作区重名拒绝或自动后缀（v1 自动后缀 `-2`、`-3`）。
- 上传件是敌意输入：不执行、可删除、按链接/IP 限速。

## 5. 端点

```text
所有者（B+ 签名）
GET    /api/share                     列出链接
POST   /api/share                     创建 file/workspace 链接（返回 token 一次）
DELETE /api/share/{id}                撤销
GET    /api/workspaces/{id}           读工作区元信息
PUT    /api/workspaces/{id}/note      投影推送
GET    /api/quarantine                过渡区列表（Phase 5 完成语义）

公网（token）
GET    /public/files/{token}/meta
POST   /public/files/{token}/confirm
GET    /public/files/{token}/blob     凭票据；支持 Range
GET    /public/workspaces/{token}/meta
GET    /public/workspaces/{token}/files
POST   /public/workspaces/{token}/files/upload
PATCH  /public/workspaces/{token}/files/{id}
DELETE /public/workspaces/{token}/files/{id}
GET    /public/workspaces/{token}/events     SSE：只发 rev
```

## 6. 验收标准（可执行）

1. `cargo test -p cista-server` 覆盖（每条对应一个测试）：
   - 无 token `GET /public/workspaces/{t}/meta` → 401；伪造 token → 404（同形，不泄漏存在性）。
   - `files=ro` 链接上传 → 403；`files=rw` 上传 → 201；`files=drop` 列目录 → 403、上传 → 201。
   - 单文件链接：`meta` 200 不计数（多次 GET 后 `used_count` 仍为 0）；`confirm` 原子消费；
     `max_uses=1` 时第二次 `confirm` → 410；并发两次 `confirm`（`tokio::join!`）恰好一次 200 一次 410。
   - 撤销后 `meta`/`confirm`/`blob` → 410；过期（构造 `expires_at` 过去）→ 410；次数用尽 → 410。
   - 链接 A 的 token 访问链接 B 的对象/file_id → 404（`share_links.object_id` 校验）。
   - 分享 token 访问 `/api/notes` → 401；私有签名访问 `/public/*` → 401。
   - 上传超限（配 `max_file_bytes` 很小）→ 413，且 blob 目录无新增文件、`used_bytes` 未增加。
   - MIME 伪装（`.html` + `text/html`）→ 400 拒绝；合法 PDF → 下载响应头含 `attachment` 与 `nosniff`。
   - 文件名 `../../etc/passwd` → 清洗为 `etc/passwd` 或拒绝；重名 → 自动后缀且同工作区内唯一。
   - 工作区隔离：workspace A 的 token 看不到 workspace B 的文件。
   - 日志断言：创建/消费链接的日志（`tracing` 捕获）不含 token 明文，只含 `token_hash` 前 8 位与前缀标记。
2. `./scripts/verify.sh` 全绿。

## 影响面

| 面 | 改/不改 |
|---|---|
| server | 改：`api/share.rs`、`api/workspaces.rs`、`public/{files,workspaces}.rs`、`quota.rs`、迁移 0002（shared.db） |
| db | 改：shared.db 迁移（workspaces/workspace_files/share_links/uploads/blob_tickets） |
| core | 改：`protocol::{Scope, ShareKind}`、token 生成/哈希辅助（无 IO） |
| desktop/web | 不改（Phase 4 接 UI）；web 已有 fragment 解析 |
| android | 不改 |

## 相关 ADR

- 0004：共享 blob 静态加密与 `refcount` 生命周期（含过期只删共享副本）。
- 0005：两步消费用「原子 UPDATE ... WHERE used_count < max_uses」而非应用层计数（并发正确性）。

## 决定

- 单文件链接支持可选口令（`passphrase` 字段）；`drop` 链接默认要求口令。
- 口令失败与链接请求共用按链接/IP 限速桶（`CISTA_RATE_LIMIT_PER_MIN/BURST`）。
