# 规格：Phase 5 —— 过渡区、归档、备份与恢复演练

> 验收来源：`design.md` §11「Phase 5：过渡区 + 归档 + 备份/恢复」→
> 「归档新建笔记不覆盖源；恢复演练通过；迁移回滚独立」。

## 目标 / 非目标

- 目标：工作区过期 → `quarantined` 过渡区（公网无路由，仅所有者 API）；客户端归档（服务端没有 MK）；
  每日备份 + 恢复演练；级联删除规则。
- 非目标：不做冷备副本；不做自动清理；不做常驻备份服务（`design.md` §2.6、§12）。

## 1. 状态与路由

```text
workspace: active → expired → quarantined → archived | purged
           quarantined → active（restore：生成新链接）
```

- `quarantined`：`/public/*` 对该工作区一律 404/410（无任何公网路由可达）；只在所有者 B+ API（`GET /api/quarantine`）可见。
- 过渡区照常计配额；**不静默删除**；不自动清理，人工在所有者会话里处理。

## 2. 归档（客户端执行）

```text
1) 客户端取回工作区明文（note + 文件）
2) 用 MK 加密 → 新建私有笔记/附件（fork/blank 一律新建，记 source_note_id 溯源；不覆盖源）
3) 校验（能解密、文件数量与哈希一致）
4) POST /api/workspaces/{id}/archive-complete  → 服务端 purge 共享数据
5) 失败可重试；未完成时停留在 quarantined
```

级联规则（`design.md` §3.5）：

| 情形 | 行为 |
|---|---|
| 源笔记删除 → 只读投影 | 全部级联删除，链接 410 |
| 源笔记删除 → 编辑分叉 | 不受影响 |
| 源笔记删除（有访客上传件） | 阻止删除：取消 / 移入共享收件箱 / 强制删除（记日志） |
| 只读投影归档（有上传件） | 为上传件新建私有笔记，文件拍平；不复制正文 |
| 只读投影归档（无上传件） | 直接删投影（可在 `expired` 直接 `purged`） |
| 分叉/空白归档 | 新建私有笔记 + 文件拍平 |

## 3. 备份与恢复（`design.md` §2.6、§10）

```bash
# scripts/backup.sh（timer/cron 每日；失败写 journal 并以非零退出）
sqlite3 private.db "VACUUM INTO '<backup>/private.db'"   # 不是 cp 活 DB
sqlite3 shared.db  "VACUUM INTO '<backup>/shared.db'"
# blob 清单（hash/size/scope；校验快照引用的 blob 都在清单里）
restic backup <data>/blobs <backup>/private.db <backup>/shared.db <data>/config
```

- 保留：restic 7 daily / 4 weekly / 6 monthly；远端第二台只跑 restic/SFTP/rest-server（只追加，不解密）。
- 三把钥匙分开：MK（恢复码离线）/ 静态加密密钥 / 备份仓库密码。
- 恢复演练：全新目录 → 恢复两份 DB + blob → 设置静态密钥 → 跑迁移 → 解一条私有笔记 → 建分享链接、上传、下载、编辑 Yjs → 记录耗时。
- 迁移回滚：前向迁移 + 独立回滚路径；代码回滚 ≠ 数据回滚；迁移先在副本上演练。

## 4. 验收标准（可执行）

1. `cargo test -p cista-server`（`tests/lifecycle.rs`）：
   - 过期工作区：`GET /public/workspaces/{t}/meta` → 410；`GET /api/quarantine` 里能看到；`restore` 后新链接可用。
   - `archive-complete` 前工作区仍在 `quarantined`；调用后共享数据被 purge（`workspace_files`/`doc_updates` 清空、blob refcount 归零）。
   - 源笔记删除（无上传件）：投影工作区级联删除、链接 410；有上传件时删除被阻止（返回 409 + 三种处理路径标记）。
   - 编辑分叉在源删除后仍可访问。
2. `scripts/backup.sh` 在一份测试数据上跑通：产出两份 DB 快照 + restic 快照；
   `scripts/restore-drill.sh` 在某目录恢复后能用 MK 解开一条笔记，并写出耗时记录到 `docs/specs/restore-drill-log.md`。
3. 迁移回滚演练：`scripts/migrate-drill.sh` 在副本上先向前迁移再回滚（独立路径），记录结果。
4. `./scripts/verify.sh` 全绿。

## 影响面

| 面 | 改/不改 |
|---|---|
| server | 改：`api/workspaces.rs`（quarantine/restore/archive-complete）、`api/notes.rs`（删除级联）、迁移 |
| db | 改：`workspaces.state/quarantined_at/archived_at`、`workspace_files.deleted_at`、`notes.deleted_at` |
| scripts | 新增：`backup.sh`、`restore-drill.sh`、`migrate-drill.sh` |
| deploy | 新增：`deploy/cista.service`、`deploy/cista-backup.{service,timer}`、`deploy/Caddyfile` |
| core | 改：归档信封格式（私有笔记 + 附件引用）与校验辅助 |

## 相关 ADR

- 0009：归档由客户端执行时的「校验 → archive-complete → purge」顺序与失败重试语义。

## 决定

- v1 不提供独立「共享收件箱」视图：上传结果通过工作区文件列表与配额计数呈现。
