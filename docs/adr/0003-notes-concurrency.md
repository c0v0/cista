# ADR 0003：笔记并发用 `If-Match: rev` + 单调 `rev`，冲突副本由客户端生成

- 状态：接受（Phase 1）
- 日期：2026-10-24

## 背景

`design.md` §2.4：服务端权威 `rev` + `base_rev`；冲突返回 `409`，客户端拉取并生成冲突副本。

- **A. last-write-wins**：实现最简，但离线编辑必然丢数据，与「数据不丢」目标冲突。
- **B. 服务端合并**：服务端只有密文，没有 MK，无法做内容合并（也不应持有明文）。
- **C. 乐观并发**：客户端带 `If-Match: <rev>`；服务端只在 `rev` 匹配时写入并 `rev += 1`，
  否则 `409` 返回当前服务端 rev；客户端拉取后按 `cista-core::sync` 决策：快进 / 生成冲突副本。

## 选择

**C。** 服务端不做内容决策，只保证「单调 rev + 拒绝陈旧写」；冲突解决完全在客户端（持有 MK 的一侧）。

## 后果

- 好处：不存在静默覆盖；服务端逻辑可被穷举测试（旧 rev / 等 rev / 超前 rev）。
- 代价：客户端必须实现 409 处理与冲突副本文案（Phase 4）；`oplog` 需要记录每次成功写入以支撑增量同步。
- 边界：`rev` 由服务端在成功写入时自增，客户端提交的 `rev` 仅作为前置条件，不参与写入值。

## 验证

- `crates/server/tests/notes.rs`：陈旧 `If-Match` → 409 且响应含当前 rev；无 `If-Match` 建新 → rev=1；
  同 rev 重放 → 409；成功写入后 `GET /api/notes?since=` 能看到新 rev。
- `crates/core`：`sync::decide` 在 `base_rev` 落后且本地有改动时返回 `Conflict`，在远端更新且本地干净时返回 `FastForward`。
