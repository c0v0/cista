# cista（私匣） · 最终设计

> 规模：单人维护、小范围真实数据；单租户；单实例（一进程/一容器）。
> 服务端：Rust（axum）+ SQLite；客户端：Tauri 2 + React SPA + Android；Web 临时端。
> 信任边界：私有 E2E；共享服务端可读（静态加密）；归档后转私有。

## 规模与偏离（先读）

- 这是单人维护、小范围真实数据的设计；**刻意不采用“托管后端”模板**：自托管、E2EE、Rust/Tauri/Android 是产品约束，不是默认最佳实践。
- 偏离的补偿控制必须可核验：每日备份 + 恢复演练、外部 uptime 告警、每端点 A→B 403/404、发布门禁、公开分享前的一次性外部评审。缺任一项，不开放公网分享。
- 先跑一个闭环：私有笔记 + 设备同步 + 一个共享工作区/单文件链接；Yjs、归档、更多客户端按 Phase 串行，不并行堆功能。
- **决策（2026-10-24）：保留 B+。** v1 只实现 B+，不做 bearer 双栈；B+ 冻结不再扩展。外部评审仍是硬门禁；评审不可行才评估切 bearer（§2.2），不在同一版本并存。
- 不承诺 SLA；单所有者 + 单实例，目标是“数据不丢、不泄、能恢复”，不是高可用。

---

## 0. 术语

| 术语 | 含义 | 不是什么 |
|---|---|---|
| 实例 / 租户 | 一个实例（进程/容器）+ 一份数据 + 一把 MK + 一个所有者 | 不是应用内用户账号 |
| 所有者 | 拥有实例的人；设备配对、创建分享 | — |
| 设备 | 配对过的桌面/手机/临时 Web | 不是账号 |
| 访客 | 拿到分享链接的人 | 没有账号、没有私有空间 |
| 空间 | 该实例的总存储与配额 | 不是登录账号 |

多人使用 = 再部署一个实例（新租户），各有各的 MK、域名/地址、数据、备份；不共享私有空间。

---

## 1. 三个信任域

| 域 | 谁访问 | 凭据 | 服务端可见 | 泄漏后果 |
|---|---|---|---|---|
| 私有 | 所有者的设备 | Ed25519 设备密钥（B+） | 密文、元数据 | 设备被偷 = 公钥吊销；内容仍 E2E |
| 分享 | 任何拿到链接的人 | capability token（+ 可选口令） | 共享明文、策略、访问日志 | 该对象在有效期内可被访问 |
| 运维 | 所有者 | SSH key（宿主机） | 全部 | 服务器被控 |

硬规则：分享路由只能触达共享库；私有 API 公网可达，逐请求 B+ 签名；运维只走 SSH。

---

## 2. 私有面

### 2.1 加密

- `MK`：32B CSPRNG，首个客户端生成，永不离开设备；恢复码离线保存。
- 每篇笔记随机 `CK`、每个文件随机 `FK`，用 MK 包裹；AEAD `XChaCha20-Poly1305`，AAD 绑定对象 ID/rev。
- 本地明文只存在设备；服务端只有密文。
- 恢复：恢复码 + 一次性入网；完成后吊销旧公钥。没有 MK = 私有数据无法恢复。

### 2.2 认证：B+

- 每设备一对 Ed25519；服务端只存公钥。
- 首设备：一次性 bootstrap token 注册公钥。
- 新设备：二维码带公钥 + nonce；旧设备验证新设备的 MK 证明后签名新公钥。
- 请求签名覆盖 `method‖path‖sha256(body)‖nonce‖ts`；服务端有重放缓存与时钟偏移窗口（±120s）。
- 吊销：删除公钥；记日志。
- Web：浏览器生成临时密钥对，手机/桌面扫码后签名入网；MK 只在内存；15 分钟会话；刷新即重配对。
- 恢复码是残余风险：限速、一次性、记日志。
- **降级方案（外部评审不可行时）**：每设备 256-bit 随机 bearer token，服务端只存 `SHA-256`；TLS + OS keychain；吊销删 token；请求用 `Authorization: Bearer`。去掉 nonce/timestamp/签名/公钥注册；E2EE 不变。两套不要并存。
- **决策**：保留 B+，本项目把“逐请求签名/无 bearer”视为硬需求；v1 只做 B+。外部评审必须通过；若评审失败，才单独评估切 bearer，不在同一版本实现两套。

### 2.3 暴露

```text
/api/*       公网 + B+ 逐请求签名
/public/*    公网：分享读写/上传/下载
/ws          公网：Yjs；首条消息认证或 30 秒 ticket
/healthz     公网部署必需：仅 200（systemd/Docker healthcheck / 外部 uptime）
运维         仅 SSH（宿主机）；不提供远程管理 API / 管理后台
```

- 公网私有路由必须：每 IP/设备/路由限速、鉴权失败锁定、请求体/超时限制、无匿名端点。

### 2.4 同步

- 服务端权威：`rev` + `base_rev`；冲突返回 409，客户端拉取并生成冲突副本。
- 删除用 tombstone；账号级 `oplog.seq` 只支撑私有增量同步，不做复制。
- 私有笔记不实时协作；实时只用于共享工作区。

### 2.5 文件与搜索

- 笔记附件：扁平，挂在单篇笔记下，随笔记生命周期。
- 共用 `blobs(hash, scope, size, refcount)`；私有与共享物理分离。
- 客户端本地索引（桌面/手机 SQLite，Web 内存；`LIKE` 子串，转义 `%/_`）：`source_type, source_id, note_id, name, note_title`；只索引私有内容；搜索文件名（`name`；不搜正文/OCR）→ 附件打开所属笔记并定位；`note_title` 仅展示；`drive_node` 打开网盘位置。
- 索引随附件同步/改名/删除、笔记改名/删除增量更新，可全量重建；不同步、不上传；只存文件名/标题/ID，不存正文。
- `source_type` 取值 `note_attachment`（v1 只写这一个；`drive_node` 为保留值，不建私有网盘表）。
- 统一搜索框默认只查本地私有；切到共享范围才请求共享搜索（q 会到服务端）。共享结果打开工作区；私有内容/索引永不出客户端；服务端只查共享表。
- 共享搜索 v1 只搜 active 工作区标题与文件名，不搜 Yjs 正文；离线只返回本地私有结果。

### 2.6 备份

- 本地：每日 systemd timer/cron 执行 §10 的备份命令（`VACUUM INTO` + blob 清单 + age/restic），失败写 journal；手动可重跑。
- 远端：可选；第二台只存 restic/SFTP 加密归档，和本地同一次触发，不做常驻复制服务。
- 保留：restic 7 daily / 4 weekly / 6 monthly；恢复演练首次 + 每季度/迁移后。
- 冷备副本 v1 不做；不预留 `/replica` 或复制端点。
- 三把钥匙分开：MK、静态加密密钥、备份仓库密码。
- 恢复演练：恢复 DB + blob → 起服务 → 用 MK 解一条私有笔记 → 打开共享链接。
- 删除实例：停服 → 删 DB/blob/日志/备份（按保留期）→ 轮换静态密钥；备份内残余如实披露。

---

## 3. 共享面

### 3.1 内容与加密

- 正文：`Y.Text` + Markdown 源码 + 客户端预览；CommonMark + GFM；禁止原始 HTML。
- 文件引用：`![alt](file:<file-id>)`；预览按工作区权限解析；归档时重写为私有附件引用。
- 共享内容服务端可读；每实例静态密钥 + TLS；归档后转 E2E。
- 隐私说明必须写明：私有 = 服务端不可读；共享 = 服务端可读。

### 3.2 链接

- `share_links.kind = file | workspace`；同一个表、同一套过期/次数/撤销/日志。
- token：128-bit 随机，客户端生成；服务端只存 `SHA-256(token)`。
- 链接：`https://<frontend>/#/s/<token>`、`/#/w/<token>`；token 在 fragment，不进前端的日志/Referer。
- SPA 调 API 时放 header（`Authorization: Share <token>` 或 `X-Share-Token`），不放 query。
- 两步：`GET` 落地页不计数 → `confirm` 只校验并签发一次性票据（不消费次数）→ 凭票据下载成功时原子消费次数；中断后重新 confirm 换新票据。
- 过期/撤销/次数用尽 → 410。
- 时限关系：`link.expires_at <= workspace.expires_at`；单个链接过期只让该链接 410，workspace 过期才进入过渡区。

### 3.3 工作区

| 区域 | 模式 | 访客能做什么 |
|---|---|---|
| note | `off` | 不显示笔记 |
| note | `ro` | 只读 |
| note | `rw` | 编辑（Yjs） |
| files | `off` | 不显示文件 |
| files | `ro` | 列目录/下载/预览 |
| files | `rw` | + 上传 |
| files | `rwm` | + 改名/删除 |
| files | `drop` | 只上传，不列目录 |

- 内部权限位：read / upload / manage；`manage` 依赖 `read`；`upload` 独立；`write`（笔记）依赖 `read`。
- UI 用预设；存储用 bitmask；日志记 bit。
- 文件区 v1 扁平；schema 保留 `parent_id`；同工作区文件名唯一。
- 工作区之间文件完全隔离；所有者客户端可管理全部，链接只能管本工作区。
- 配额：实例总量、单工作区、单链接、单文件。

### 3.4 单文件链接

- 独立 `kind=file`；只读；落地页 → 确认 → 下载。
- 共享 blob 用 refcount；删链接只减引用；私有原件不动。
- 过期只删共享副本，**无归档任务**。
- 创建单文件链接 = 主动把私有文件复制到共享域；UI 必须提示“服务器可见”。

### 3.5 只读投影与编辑分叉

- 只读笔记链接：**客户端推送投影**。所有者客户端每次保存私有笔记，把 Markdown 推到共享工作区；离线时显示最后一次保存。
- 编辑链接：**分叉**。创建时解密复制成独立工作区，有自己的 DK；不回写源。
- 源笔记删除 → 所有只读投影级联删除，链接 410；**有访客上传件时阻止删除**：取消 / 移入共享收件箱 / 强制删除（记日志）。
- 共享收件箱是所有者侧的 `uploads` 视图（upload-only 权限或未被引用的上传），不是独立存储。
- 编辑分叉独立，源删除不影响。

### 3.6 过期与归档

- 工作区过期（分叉/空白）：所有链接 410；进入 `quarantined` 过渡区；公网无任何路由可达；仅所有者 B+ API 可见。
- 过渡区照常计配额；不静默删除；不自动清理，需要时人工处理。
- 归档由客户端完成（服务端没有 MK）：取回明文 → 用 MK 加密 → 新建私有笔记/附件 → 校验 → `archive-complete` → 服务端 purge。
- **分叉/空白工作区归档时总是新建私有笔记**，不覆盖源；记 `source_note_id` 溯源。
- 分叉/空白工作区：新建笔记 + 文件拍平。
- 只读投影：不复制正文；有上传件时为上传件新建一篇笔记；无上传件直接删投影。
- 源已删除的投影：按 3.5 级联规则处理。
- 归档不改变“过去已经暴露过”的事实；旧备份按保留期过期。

### 3.7 上传

- 白名单：图片、音频、视频、PDF、Office、纯文本、zip；禁止可执行/HTML/SVG/脚本。
- 单文件 100 MiB（可配）；`drop` 默认要求口令（Argon2id）；`rw`/`rwm` 可选。
- 永不内联：`Content-Disposition: attachment` + `nosniff` + CSP；下载可放独立子域（可选）。
- 可选 ClamAV；文件名清洗、MIME 嗅探、同名拒绝/自动后缀、按链接/IP 限速。
- 上传件是敌意输入：不执行、不内联、可删除。

### 3.8 Yjs

- Rust `yrs` + `y-sync`；每工作区一个 `Doc`。
- 只读强制在服务端：`note=ro` 的连接可收 sync/awareness，丢弃其 `update`。
- 持久化在 Rust DB：`workspace_notes(yjs_state, state_vector, rev)` + `doc_updates(seq, update)`；每次 update 同步落库。
- 快照：每 100 次 update 或 5 分钟做一次当前快照；快照成功后清空已合并的 update。
- 不提供历史版本/恢复 UI；只有当前状态 + 崩溃恢复（快照 + 尾部 update）。
- 限制：单文档 5 MiB、单 update 1 MiB、每连接限速；空闲 10 分钟卸载 Doc。
- 归档导出 `Y.Text` 为 Markdown。

---

## 4. 客户端与部署

### 4.1 客户端

- Tauri 2 + 同一 React SPA + Android。
- 1 天 Android spike：构建、Rust command、文件 IO、Keystore 存 Ed25519 设备密钥、B+ 签名、Yjs WS、后台任务。
- 失败回退：Tauri 桌面 + Flutter Android（共享 `crates/core`）；不选 Flutter 全端（Web 会变第二套 UI）。
- 桌面/手机：本地 SQLite + 文件索引、离线、同步、附件。
- Web 临时端：在线、内存密钥、内存索引（已同步范围）、不落盘。

### 4.2 部署形态（同源或跨域，二者都支持）

默认同源：`https://<frontend>`，反代 `/api`、`/public`、`/ws`、`/healthz` 到后端；不需要 CORS。
可选跨域：前端 B（静态 SPA）+ 后端 A；此时启用 CORS。代码不得假设一定分域。

- 前端用 `apiBase` 配置：同源为空/相对路径；跨域时为 A。
- **每个客户端首次启动必须能填写服务器地址**（`web/src/pages/ServerSetupPage.tsx`）：
  原生端（桌面/Android）在 WebView 里没有真实站点 origin，因此未配置前不得进入配对/分享页；
  Web 端默认同源，也可改成跨域地址。地址优先级：用户保存值（`localStorage:cista.server.base`）
  > 构建期 `VITE_API_BASE` > 同源；只接受 `http(s)` origin（可带路径前缀），
  拒绝账号密码/query/fragment；「测试连接」只请求 `<base>/healthz`，不带任何凭据；
  修改地址等同换身份，当前内存会话立即丢弃。
- 分享链接始终指向前端 origin；同源/跨域都不变。
- 跨域：只允许 B 的 origin；允许 `Authorization/X-Share-Token/X-Device-Id/X-Nonce/X-Timestamp/X-Signature/Content-Type/Range/Idempotency-Key`；暴露 `Content-Disposition/Content-Range/Accept-Ranges`；**不开 credentials**。
- 鉴权：短会话标识（内存）+ 每请求 B+ 签名；不用 cookie；刷新即重配对。
- CSP：同源 `connect-src 'self'`；跨域再加 A 的 HTTP/WS。
- 下载：`fetch` + Blob；大文件 Range 分块；不把 token 放 URL。
- WS：同源 `wss://host/ws`；跨域 `wss://A/ws`；浏览器校验 `Origin` 为 B，原生允许无 Origin；首条消息认证或 30 秒 ticket。

### 4.3 部署

- 单实例；**不强制容器**：默认单二进制 + systemd；Docker 只是可选包装。
- 域名布局：同源或前后端分域都支持；不假设一定两个域名；单实例部署。
- 约束：代码与配置不得要求 Docker、前后端分域或 VPN；Docker/分域只是支持，VPN 不采用。
- Caddy：TLS/HSTS/请求体限制/超时/限速；WebSocket 代理关闭缓冲。
- 若用容器：非 root、只读根文件系统、`no-new-privileges`、内存/CPU/PID 限制；若用 systemd：专用用户、只读数据目录、limits。
- 升级：systemd 替换二进制 + `systemctl restart cista`；Docker `docker compose pull && docker compose up -d`；升级前备份；保留上一个版本。
- 日志：systemd `journalctl -u cista` 或 Docker logs；结构化 JSON；日志脱敏；记录链接/设备/上传/归档操作（token 哈希、IP 哈希、字节、状态）；轮转约 30 天；不记正文/token 明文；不建审计表/审计 UI、不建 metrics。
- 合规：地区未定；设计保持地区无关；中国大陆需 ICP/公安备案。

### 4.4 运维（最小）

- SSH 到服务器；systemd `systemctl restart cista`，或 Docker `docker compose up -d` / `docker compose restart`。
- 配置手动改 `.env` 或配置文件，重启生效；不做配置 API、热加载、管理后台。
- 备份/恢复由 timer 调用脚本（`VACUUM INTO` + blob/restic），不做常驻备份服务。
- 可用性：外部 uptime 每 5 分钟打 `/healthz`，告警到所有者邮箱/手机；不做 `/readyz`、`/metrics`、Prometheus/Grafana。
- 告警接收人 = 所有者本人，写进 README；没有告警不开放公网。
- 关键账号（域名/DNS/VPS/备份仓库）留恢复码；可选第二所有者，不共享密码。
- 事故顺序：先换凭据/吊销设备，再修；记录时间线；删除/轮换/恢复按 runbook。
- 不提供远程管理 API、Web 管理后台、远程执行/重启接口。

---

## 5. 十条不变量

1. 私有内容服务端不可读；共享内容服务端可读——边界写进 UI 与隐私说明。
2. 分享 token 不进日志、不进 query、不进 Referer；链接用 fragment，API 用 header。
3. 只读链接必须服务端强制；前端隐藏不算。
4. 分享路由永远不能触达私有表/私有命名空间；独立库/连接。
5. 删除源笔记级联投影；有上传件先阻止。
6. 归档由客户端完成；服务端不静默删除过渡区数据。
7. 三把钥匙（MK / 静态密钥 / 备份密码）分开；恢复演练过才算备份可用。
8. 讲不清的代码不合并（本项目硬规则）。
9. 私有搜索只在客户端；共享搜索只查共享表；统一搜索不得上传私有索引/明文；共享查询需显式切换，q 不进 URL/日志。
10. 每个资源端点都有 A→B 403/404 行为测试；分享 token 不能越出本对象。

---

## 6. 状态机

```text
workspace: active → expired → quarantined → archived | purged
           quarantined → active（restore，生成新链接）
link:      active → expired | revoked | exhausted
upload:    pending → complete | failed | aborted
device:    active → revoked
```

- `expired` 与 `quarantined` 都可 410；区别是 `quarantined` 已从 active 命名空间移出，只在所有者 API 可见。
- link 过期只影响该链接；workspace 过期才进入 `quarantined`。
- 只读投影且无上传件：可在 `expired` 直接 `purged`；有上传件则按上传件归档，不复制正文。
- 归档是客户端任务；没有客户端在线时停留在 `quarantined`。

---

## 7. 数据模型（轮廓）

```text
-- 设备与同步日志
devices(id, name, pubkey, revoked_at, last_seen, last_acked_seq, created_at)
oplog(seq, entity, entity_id, op, payload, created_at)
-- payload 已加密；只做私有增量，不做复制；不建 audit 表

-- 私有
blobs(hash, scope, size, refcount, created_at)          -- scope: private | shared
notes(id, rev, ciphertext, wrapped_ck, deleted_at)
note_attachments(id, note_id, blob_hash, name_cipher, mime, size, created_at)
-- v1 不做私有网盘；无 drive_nodes（本地索引预留 drive_node）

-- 共享
workspaces(id, title, origin, source_note_id, state,
           expires_at, quarantined_at, archived_at, created_at)
           -- origin: blank | fork | projection
workspace_notes(workspace_id, rev, yjs_state, state_vector, updated_at)
workspace_files(id, workspace_id, parent_id NULL, name, blob_hash, size, rev, deleted_at)
doc_updates(seq, workspace_id, update, created_at)  -- 当前快照之后的尾部；不保留历史版本
share_links(id, token_hash, kind, object_id,
            note_scopes, file_scopes,
            expires_at, max_uses, used_count,
            passphrase_hash, revoked_at, created_at)
            -- note_scopes/file_scopes 是 bitmask；UI 预设只是映射
uploads(id, link_id, workspace_id, blob_hash, name, size,
        wrapped_fk, status, created_at)

-- 服务端 SQLite 索引
idx_notes_rev, idx_workspace_files(workspace_id), idx_updates(workspace_id, seq),
idx_links_token(token_hash), idx_oplog_seq
```

客户端本地（非服务端，桌面/手机 SQLite；Web 内存）：

```text
file_index(source_type, source_id, note_id NULL, name, note_title, updated_at)
-- source_type: note_attachment | drive_node；v1 只写 note_attachment
-- note_id/note_title 对 drive_node 为空
-- 只存文件名/标题/ID，不存正文；不同步、不上传
```

- 单租户：不加 `account_id`；实例（进程/容器）就是租户边界。
- 私有 blob 与共享 blob 物理分离；`scope` 防串读。
- 私有文件名/标题可加密；共享文件名/标题明文（服务端可读）。
- 删除用 `deleted_at`/tombstone；GC 按 refcount 与保留期。
- `oplog` 按活跃设备最小 `last_acked_seq` 清理，最长 90 天；`doc_updates` 快照后清；不建审计表。

---

## 8. 路由与协议（轮廓）

```text
# 所有者 API（公网 + B+）
POST   /api/devices/enroll            首设备 bootstrap / 新设备签名入网
POST   /api/devices/{id}/revoke
GET    /api/notes?since=seq
PUT    /api/notes/{id}                If-Match: rev
POST   /api/files                     blob 上传（密文）
GET    /api/share                     列出链接
POST   /api/share                     创建 file/workspace 链接
DELETE /api/share/{id}
POST   /api/search/shared              body {q}；只搜 active 共享工作区/文件；B+
GET    /api/quarantine
GET    /api/workspaces/{id}            所有者读取工作区（归档/预览）
PUT    /api/workspaces/{id}/note       投影推送/所有者编辑（B+）
POST   /api/workspaces/{id}/archive-complete
POST   /api/workspaces/{id}/restore

# 公网分享
GET    /public/files/{token}/meta
POST   /public/files/{token}/confirm  签发一次性票据（不消费次数）
GET    /public/files/{token}/blob     凭票据流式下载
GET    /public/workspaces/{token}/meta
GET    /public/workspaces/{token}/files
POST   /public/workspaces/{token}/files/upload
PATCH  /public/workspaces/{token}/files/{id}
DELETE /public/workspaces/{token}/files/{id}
GET    /public/workspaces/{token}/events   SSE：只发 rev

# 配对
POST   /api/devices/enroll                   B+ 签名 + bootstrap token
POST   /api/devices/{id}/revoke             设备吊销


# 实时
WS     /ws                            首条消息：token + mode
```

以上 `/api/*` 是所有者产品操作，不是运维管理接口；运维只走 SSH。
私有文件搜索没有服务端端点：客户端本地；`/api/search/shared` 只查共享表，不触达私有表；q 放 body，不进 URL/访问日志，日志不记 q。
WS 支持两种认证：分享 token（访客）或设备签名（所有者）；只读连接不能发 update。
大文件上传的 B+ 签名覆盖 `ciphertext hash + size + idempotency key`；分块上传签 manifest 或每块签名。

- 签名请求头：`X-Device-Id`、`X-Nonce`、`X-Timestamp`、`X-Signature`。
- 分享：`Authorization: Share <token>`；口令用单独 header/表单。
- 错误码：400/401/403/404/409/410/413/429。
- 幂等：上传与确认用 `Idempotency-Key`；票据一次性、短 TTL。
- 分页/限速：所有 list/搜索/文件端点默认 50、最大 200；`since` 单次最多 500 条；按 IP + 设备/链接限速，429 + `Retry-After`。

---

## 9. 配额与限制

```text
instance_quota   = 0.80 × 卷容量
workspace_quota  = 0.25 × instance_quota
link_quota       = 0.10 × instance_quota
max_file_bytes   = 100 MiB
disk_watermark   = 0.90 × 卷容量
```

- 私有附件与共享文件都计入实例总量；工作区/链接上限只约束共享上传。
- 原子预留 + 流式硬截断；不信任 `Content-Length`。
- 磁盘水位触发时拒绝新上传，保留读取。
- Yjs：单文档 5 MiB、单 update 1 MiB、每连接限速。

---

## 10. 备份与恢复

**本地（手动）**

```bash
sqlite3 app.db "VACUUM INTO '/backup/db.sqlite'"
restic backup /srv/cista/blob /backup/db.sqlite /srv/cista/config
```

**远端**：第二台只跑 restic/SFTP/rest-server，仓库加密、只追加；不跑应用。

**三把钥匙**

| 钥匙 | 丢了会怎样 | 放哪 |
|---|---|---|
| MK | 私有备份无法恢复 | 恢复码离线 |
| 静态加密密钥 | 共享 blob 无法解密 | 离线托管 |
| 备份仓库密码 | 所有备份打不开 | 离线托管 |

**恢复演练**：全新目录 → 恢复 DB + blob → 设置静态密钥 → 跑迁移 → 解一条私有笔记 → 创建分享链接、上传、下载、编辑 Yjs → 记录耗时。

**失败模式**：`cp` 活 SQLite、备份与源同盘、密钥与备份同处、共享明文未加密、从不演练。
**迁移**：前向迁移 + 独立回滚路径；代码回滚 ≠ 数据回滚，迁移在副本上先演练。

---

## 11. 阶段与验收

| 阶段 | 内容 | 验收 |
|---|---|---|
| 0 | 仓库、CI、`verify.sh`、Tauri Android spike | 干净 clone 能跑绿；spike 有结论；uptime 告警到达；备份 timer + 恢复 |
| 1 | `crates/core` 加密 + B+ 认证 + 笔记 CRUD | 设备吊销即无效；A 读 B 404；磁盘无明文 |
| 2 | 共享工作区 + 文件区 + 单文件链接 | 只读链接不能写；上传配额并发正确；过期 410；文件/链接 A→B 403/404 |
| 3 | Yjs 实时 + 崩溃恢复 | 双客户端收敛；离线并发编辑收敛；只读连接不能发 update；重连补齐；重启不丢已确认 update |
| 4 | 客户端搜索 + Web 临时端 + Android | 搜索附件→定位所属笔记；Web 索引不落盘；配对→内存密钥；Android 真机 + 旧版本升级；Keystore 可用 |
| 5 | 过渡区 + 归档 + 备份/恢复 | 归档新建笔记不覆盖源；恢复演练通过；迁移回滚独立 |
| 6 | 加固与发布 | 每端点 A→B；恢复演练；外部评审 auth/E2EE；发布检查 |

---

## 12. 默认值与未定项

**已定默认**

- 部署：单实例；默认单二进制 + systemd；Docker 可选；同源默认，前后端分域可选。不强制容器/双域名。
- 共享文件区权限三 bit + UI 预设；文件区扁平、保留 `parent_id`。
- 客户端本地文件索引（桌面/手机 SQLite `LIKE`；Web 内存）；私有搜索不出端；共享搜索只查共享表；`source_type` 取 `note_attachment`（`drive_node` 为保留值）。
- 结构化日志，轮转约 30 天（journald 或 Docker）；不建审计表；过渡区不自动删除，人工清理。
- 单文件 100 MiB；上传白名单；配额百分比；list/搜索分页，按 IP + 设备/链接限速。
- `/api`、`/public`、`/ws`；`/healthz` 公网必需（外部 uptime）。
- 备份：每日 systemd timer/cron；restic 7 daily / 4 weekly / 6 monthly；季度恢复演练；不建常驻备份服务。
- 告警：外部 uptime → 所有者邮箱/手机；不建 metrics；关键账号留恢复码，可选第二所有者。
- 预算：VPS/域名/备份/告警约 $20–30/月；不承诺 SLA。
- B+ 保留；公开分享前外部评审 auth/E2EE；评审失败才评估切 bearer，不在 v1 双栈。
- 关键账号恢复码离线；可选第二所有者，不共享密码。
- 冷备副本：v1 不做。

**未定**

- 域名具体名称；服务器地区（大陆需备案）。
- 外部评审人/时间（auth/E2EE；未定前不开放公网分享）。
- 是否设置第二所有者（可选；恢复码必须离线）。
- UI 组件库、Markdown 编辑器（CodeMirror 6 是默认候选）。
- 具体模型 provider 与角色分配。

---

## 13. 威胁模型与残余风险

| 威胁 | 私有面 | 共享面 |
|---|---|---|
| 公网抓包 | TLS | TLS |
| 服务端磁盘/备份被拿走 | 密文 | 静态加密 |
| 主机被完全控制 | 内容仍 E2E；可用性丢失 | 明文可读；链接可被伪造服务 |
| 设备丢失 | 公钥吊销；keychain | 不适用 |
| 链接泄漏 | 不适用 | 有效期内可访问；过期收不回已下载 |
| XSS | CSP、无第三方脚本 | CSP、不内联；可选独立子域 |
| 密钥丢失 | 无恢复码 = 数据永久丢失 | 静态密钥丢失 = 共享 blob 无法解密 |
| 元数据 | 标题/文件名可加密；大小/时间泄漏 | 标题/文件名明文 |
| 搜索索引/查询 | 只存文件名/标题/ID，不上传；本地 DB 被拿走会泄漏这些元数据 | 只查共享表；共享 q 显式切换、不进日志 |
| 服务端篡改/回滚 | B+ 不防；签名操作链属 v2 方案 | 不防；备份缓解 |

- **自研 B+/E2EE 是最高风险项**：公开分享前建议做安全评审；若评审结论否定，才评估切设备 bearer token（§2.2），v1 不同时支持两种方案。
- **Yjs 是证据缺口**：必须用双客户端、离线并发、重连、服务重启和随机收敛测试证明；没有测试不上线。
- **单所有者关键账号**：域名/DNS/VPS/备份仓库丢失 = 停摆；恢复码离线保存，可选第二所有者，不共享密码。
