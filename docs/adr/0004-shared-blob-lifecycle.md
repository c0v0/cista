# ADR 0004：共享 blob 的分层记账（refcount / 配额 / 过期缺口）

- 状态：接受（Phase 2）
- 日期：2026-10-24

## 背景

`design.md` §3.4/§5 要求：单文件链接把私有文件**复制**到共享域；共享 blob 用 refcount；
删链接只减引用、私有原件不动；过期只删共享副本，且没有归档任务。
同时 §9 要求私有与共享文件都计入实例总量。

实现中三个问题需要定论：

1. 单文件链接与工作区文件可能是**同一份内容**（同 sha256）。若每次创建都记一份实例配额，
   同一字节会被重复计数。
2. 一个人删除工作区文件后，引用该 blob 的单文件链接还必须能下载。
3. 只做「访问时校验过期」而不清理 blob，会留下永不释放的共享副本。

## 选择

- **记账按内容去重**：`blobs(hash, scope)` 是唯一事实源，`refcount` 表示引用数；
  仅当共享 blob **首次出现**时预留实例配额，`refcount` 归零时释放同额配额。
- **引用方**：工作区文件行与单文件链接各记 1 个引用；删除/撤销时减 1，归零才删本体。
- **文件本体**：`store_shared_blob` 内容寻址、幂等写入（已存在不重写）。
- **过期清理**：v1 **不做后台任务**。过期链接只在访问时 410（不消费次数）。
  共享 blob 的回收依赖显式撤销（`DELETE /api/share/{id}`）与工作区文件删除；
  过期未撤销的单文件链接的副本会在磁盘上停留。
- **一次性回收命令（Phase 6 落地）**：`cista-server gc`（SSH 手工或定时器执行，无公网入口）。
  在一个事务序列里：把 `expires_at <= now()` 或 `used_count >= max_uses` 且未撤销的链接
  标记 `revoked_at`；单文件链接按 `release_shared_copy` 减引用，归零时删元数据、删本体、
  释放实例配额；删除 `blob_tickets` 中过期票据。输出
  `expired_links=… released_blobs=… expired_tickets=…`，重复执行幂等。
  实现见 `crates/server/src/gc.rs`，测试 `crates/server/tests/gc.rs`。

## 后果

- 好处：配额不被重复计数；同一内容多引用安全；撤销路径可测试。
- 代价：过期副本可能滞留（有界：单文件链接的副本 ≤ 原文件大小）；
  只读链接的过期清理需要一次人工/定时脚本调用（无公网管理 API，符合「运维只走 SSH」）。
- 回滚：去掉引用计数会退回「删除即删本体」，会破坏其他链接——不做。

## 验证

- `tests/sharing.rs::single_file_link_is_two_step_and_capped_to_one_use`（创建副本 → 下载 → 410）。
- `tests/sharing.rs::revoked_and_expired_links_are_gone`（撤销即 410）。
- 配额：`tests/sharing.rs::upload_rejects_html_oversize_and_sanitizes_names`（超限不落盘）。
