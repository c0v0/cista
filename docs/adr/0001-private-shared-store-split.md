# ADR 0001：私有面与共享面分库（两份 SQLite + 两套连接池）

- 状态：接受（Phase 0）
- 日期：2026-10-24

## 背景

不变量 4 要求「分享路由永远不能触达私有表/私有命名空间；独立库/连接」。
`design.md` §7 给出的是单实例一份 SQLite 的逻辑轮廓，没有规定物理库数量；
`design.md` §10 只写了 `sqlite3 app.db "VACUUM INTO ..."`。

可选实现：

- **A. 单库 + 代码纪律**：所有表在一个 `app.db`，靠评审和 grep 保证分享 handler 不查私有表。
  代价：隔离靠人；一次疏忽即越权，且测试只能证明「测到的那条路径」没查。
- **B. 单库 + `sqlite3_set_authorizer`**：给分享连接安装授权器，拒绝访问私有表。
  代价：需要 `libsqlite3-sys` 原生回调；sqlx 不直接暴露 authorizer；运行时开销与复杂度高。
- **C. 两个 SQLite 文件**：`private.db`（devices/oplog/notes/note_attachments）与
  `shared.db`（workspaces/workspace_notes/workspace_files/doc_updates/share_links/uploads），
  各自连接池；分享路由的 state 里只存在 shared 池。

## 选择

**C。** 结构性隔离：分享路由拿不到私有池，就不存在「忘记判断」的失败模式；
实现成本低（两个 `SqlitePool`），测试可断言路由 state 中没有私有池。

## 后果

- 好处：不变量 4 由类型结构保证，而不是评审纪律；私有表名不会出现在分享模块里。
- 代价：
  - 无法用 SQLite 外键跨库约束。`workspaces.source_note_id` 只是溯源字段（弱引用），
    `share_links.object_id` 指向同库对象；跨库一致性由应用层校验 + 测试覆盖。
  - 备份/恢复要覆盖两个文件（`scripts/backup.sh` 对两份都做 `VACUUM INTO`）。
  - blob 目录同样按 `scope` 物理分离：`blobs/private/` 与 `blobs/shared/`。
- 回滚：若将来合并回单库，需要写迁移把两库表合并，并重新建立「独立连接」的补偿控制；
  这是重构，不是配置变更。

## 验证

- `crates/server/src/state.rs`：`AppState` 中 `shared: SharedStore` 结构体只持有 `shared.db` 池，
  分享路由的 handler 参数只接受 `SharedStore`。
- `crates/server/tests/`：分享端点测试断言对私有表的查询（如 `notes`）在分享池上必然失败。
