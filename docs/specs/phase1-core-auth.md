# 规格：Phase 1 —— 加密内核、B+ 认证、笔记 CRUD

> 验收来源：`design.md` §11「Phase 1：`crates/core` 加密 + B+ 认证 + 笔记 CRUD」→
> 「设备吊销即无效；A 读 B 404；磁盘无明文」。

## 目标 / 非目标

- 目标：私有笔记的密文落盘与增量同步（服务端只见密文、rev、时间），设备入网/吊销，B+ 逐请求签名。
- 非目标：共享面（Phase 2）、Yjs（Phase 3）、客户端 UI（Phase 4）；不做 bearer 双栈（ADR 0002）。

## 1. 密文格式（`cista-core::crypto`，v1 冻结）

| 对象 | 值 | 说明 |
|---|---|---|
| MK | 32B CSPRNG | 只在客户端内存/钥匙串；服务端永不持有 |
| CK（每篇笔记） | 32B CSPRNG | 用 MK 包裹后随笔记上行 |
| 恢复码 | MK 的 32B 原文（分组显示） | 离线保存；服务端不存 |

- AEAD：`XChaCha20-Poly1305`，24B 随机 nonce（每次加密新生成，不从计数器复用）。
- 包裹：`wrapped_ck = AEAD(MK, n1, aad = "cista.ck.v1" ‖ note_id, CK)`
- 正文：`ct = AEAD(CK, n2, aad = "cista.note.v1" ‖ note_id ‖ rev_le_u64, plaintext)`
- AAD 绑定对象 ID 与 rev：密文被挪到别的笔记、或旧 rev 重放解不开。
- 线格式（`SealedNote`，JSON，字段 base64）：
  `{"v":1,"rev":u64,"wrap":{"nonce":b64,"ct":b64},"body":{"nonce":b64,"ct":b64}}`
- 明文信封（客户端侧）：`{"kind":"note","title":str,"markdown":str}`——标题也在密文里，服务端不可读。

## 2. B+ 认证（v1 冻结）

- 设备：Ed25519（`ed25519-dalek`）；服务端只存公钥。`device_id = hex(sha256(pubkey))[..32]`。
- 首设备入网：`POST /api/devices/enroll`，`Authorization: Bootstrap <token>`（服务端环境变量，一次性）。
- 新设备入网：`POST /api/devices/enroll` 带 `X-Device-Id/-Nonce/-Timestamp/-Signature`（由已入网设备签名）+ body 里的新公钥。
- 签名串（UTF-8，`\n` 分隔，不含结尾换行）：
  `cista.b+1\n<METHOD>\n<PATH>\n<sha256(body).hex>\n<nonce.hex>\n<ts>`
  - `PATH` 是请求路径（含 query 由服务端拼接，v1 私有端点无 query 语义；`since` 参与签名）。
  - `body` 为原始字节；空 body 的 sha256 = `e3b0c442...b855`。
- 请求头：`X-Device-Id`、`X-Nonce`（16–32B CSPRNG hex）、`X-Timestamp`（unix 秒，十进制）。
- 服务端校验顺序：头齐备 → 设备存在且未吊销 → `|now - ts| ≤ 120s` → nonce 未用过（TTL 300s 内存缓存）→ 验签。
  失败一律 `401`（不回显原因细节），并丢弃已消费的 nonce。
- 吊销：`POST /api/devices/{id}/revoke`（需另一台已入网设备签名）→ 立即 401。

## 3. 端点与状态码

| 端点 | 语义 | 失败码 |
|---|---|---|
| `POST /api/devices/enroll` | bootstrap 或已入网设备签名入网 | 401（token 错/签名错）、409（device_id 已存在） |
| `POST /api/devices/{id}/revoke` | 吊销设备（只能吊销非自身） | 401、403（吊销自己）、404（设备不存在） |
| `GET /api/notes?since=<seq>` | 增量拉取（`seq` 默认 0，上限 500 条） | 401、400（since 非法） |
| `PUT /api/notes/{id}` | 保存密文；`If-Match: <rev>` 乐观并发 | 401、409（rev 冲突，body 返回服务端当前 rev）、413（超限） |
| `GET /api/notes/{id}` | 取单篇 | 401、404 |
| `POST /api/files` | 密文 blob 上传（`Idempotency-Key`） | 401、413 |

- 笔记归属：单租户，无 `account_id`；A→B 的语义由「设备 ID 是路径一部分」体现：
  `/api/notes/{id}` 未知 id 一律 `404`（不区分「存在但无权」与「不存在」）。
- `rev` 单调自增；`base_rev` 冲突返回 409 + 服务端当前 rev，由客户端生成冲突副本（`cista-core::sync`）。

## 4. 验收标准（可执行）

1. `cargo test -p cista-core` 全绿，覆盖：
   - 包裹/解包往返；AAD 改动（note_id 或 rev 变化）解包失败；
   - 密文篡改 → 解包失败；nonce 不复用（同明文两次加密的 nonce 与密文均不同）；
   - 签名往返；改了 method/path/body/ts 任一 → 验签失败；错误公钥 → 失败；
   - 重放窗口：同一 nonce 第二次被拒；ts 超出 ±120s 被拒；非单调 rev 的同步状态机返回冲突。
2. `cargo test -p cista-server` 全绿，覆盖：
   - `/healthz` 200 无鉴权；`POST /healthz` 405；未知路径 404。
   - 无签名请求 → 401；时间戳超窗 → 401；重复 nonce → 401；吊销后 → 401。
   - 设备 A 签名读设备 B 的笔记 id：一律 404（不存在与无权同形）。
   - `PUT` 用旧 rev → 409 且返回当前 rev；`PUT` 新对象 → 201/200 且 rev=1。
   - `GET /api/notes?since=` 半开区间、顺序、上限 500。
   - 磁盘无明文：测试写入的明文标记串不出现在 `private.db` 文件字节中。
3. `./scripts/verify.sh` 全绿。

## 影响面

| 面 | 改/不改 |
|---|---|
| server | 改：`state`、`db`、`auth`（B+ extractor）、`api/{health,devices,notes,files}`、迁移 0001 |
| db | 改：新增 `private.db` 迁移（devices/oplog/notes/note_attachments/blobs） |
| core | 改：新增 `crypto`、`auth`、`sync`、`protocol` |
| desktop/web/android | 不改（Phase 4 接入） |

## 相关 ADR

- 0003：`PUT /api/notes/{id}` 的并发模型（`If-Match` + rev 单调）与服务端冲突副本策略。
- 0004：blob 存储布局与 `refcount`（Phase 2 一起落）。

## 决定

- 恢复码 = MK（32 字节）的 base64url，43 字符、不分组；客户端原样复制/粘贴即可，服务端不参与。
