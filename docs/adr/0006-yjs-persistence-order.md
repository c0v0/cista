# ADR 0006：Yjs 持久化顺序与只读强制

- 状态：接受（Phase 3 设计定稿；实现待编译环境恢复后落盘）
- 日期：2026-10-24

## 背景

`design.md` §3.8 要求：Rust `yrs` + `y-sync`；每工作区一个 `Doc`；只读连接丢弃 `update`；
每次 update 同步落库；快照 + 尾部 update 支撑崩溃恢复；单文档 5 MiB / 单 update 1 MiB。

关键抉择是**落库与 ack 的顺序**，以及「只读」在协议层的落点。

## 选择

1. **先落库，后转发/ack**：收到客户端 `update` → 校验大小与权限 → 在 SQLite 事务里
   `INSERT INTO doc_updates(workspace_id, update)` 并提交 → 再 apply 到内存 `Doc` → 再广播给其他连接。
   这样任何「已被服务端接受」的 update 一定能从库里重放出来。
2. **快照点语义**：每 100 条 update 或 5 分钟，计算当前 `yjs_state` 与 `state_vector`，
   写入 `workspace_notes`，提交后删除 `doc_updates.seq <= snapshot_seq` 的行。
   删除与快照必须在**同一事务**里，避免「快照没写却删了尾部」。
3. **只读强制在服务端**：`note=ro` 的连接照常收到 sync/awareness，但服务端在 apply/落库之前
   直接丢弃其 `update`（不回报错误，避免把只读状态泄漏成可探测信号）。
   `note=off` 的连接不建立 Doc。
4. **握手**：WebSocket 首帧 JSON（30 秒内），成功后进入 `y-sync` 标准二进制帧：

```json
{"v":1,"mode":"share","token":"…","object":"<workspace id>"}
{"v":1,"mode":"owner","device":"…","object":"…","nonce":"…","ts":1730000000,"sig":"…","handshake":"<sha256(canonical JSON)>"}
```

5. **恢复**：`Doc::new()` → `apply_update(yjs_state)`（若有）→ 按 `seq` 重放 `doc_updates` →
   与重启前的内容和 `state_vector` 一致（测试断言）。
6. **卸载**：空闲 10 分钟落盘并卸载 `Doc`；卸载前若快照不足阈值也要做一次快照。

## 后果

- 好处：已确认的 update 不丢；快照与尾部清理原子；只读不可被客户端绕过。
- 代价：每条 update 一次 SQLite 写（共享库单写者），需要限速；广播在提交之后，延迟略增。
- 风险：`yrs`/`y-sync` 的 API 版本耦合；Phase 3 测试必须覆盖双客户端、离线并发、重连、重启与只读。

## 验证（Phase 3 验收）

- 双客户端收敛（同一 `Y.Text` 最终一致）。
- 离线并发：两侧离线编辑后重连交换 update 收敛。
- `note=ro` 发 update：库中状态不变、其他连接收不到。
- 重连：发 `state_vector` 只补缺失部分。
- 重启：关闭服务 → 重载 → 内容与 `state_vector` 一致。
- 快照：阈值触发后 `doc_updates` 被清理，仍可恢复。
- 超限：单 update > 1 MiB 被拒，库中无该行。
