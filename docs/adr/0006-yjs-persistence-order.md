# ADR 0006：Yjs 持久化顺序、只读强制与线协议实现

- 状态：接受（Phase 3）
- 日期：2026-10-24

## 背景

`design.md` §3.8 要求：Rust `yrs` + `y-sync`；每工作区一个 `Doc`；只读连接丢弃 `update`；
每次 update 同步落库；快照 + 尾部 update 支撑崩溃恢复；单文档 5 MiB / 单 update 1 MiB。

实现时发现一个硬约束：**`y-sync` 最新版本 0.4.0 固定依赖 `yrs ^0.17`**，而 yrs 0.17 的
`Doc`/`TextRef` 内部用 `Rc`/`NonNull`，**不是 `Send`/`Sync`**——无法放进 axum 的 `AppState`
（需要 `Send + Sync`）。要让 y-sync 0.4 可用，只能给每个工作区起一条专用线程做 actor，
或者换一个 `Send` 的 CRDT 版本。

## 选择

1. **CRDT 用 `yrs 0.28`（`sync` 特性）**：`Doc` 是 `Send + Sync`，可以直接放进进程状态。
2. **线协议自己实现（`wire.rs`）**，布局与 y-sync 0.4 / y-websocket 完全一致：
   `tag: varuint`；`Sync` 帧为 `subtype: varuint + buf`；`Awareness` 帧为 `buf`。
   状态向量与 update 载荷仍由 `yrs` 编解码（v1）。
   只实现服务端需要的四类消息：step1 / step2 / update / awareness 转发；
   awareness 载荷不解析（不落库、只转发）。
3. **持久化顺序（先落库，后应用/广播）**：收到客户端 `update` → 校验大小与权限 →
   `INSERT INTO doc_updates` 提交 → apply 到内存 `Doc` → 广播给其他连接。
   任何被接受的 update 都能从尾部重放。
4. **快照点语义**：每 100 条 update 或连接关闭时，计算 `yjs_state` + `state_vector` 写入
   `workspace_notes`，并在**同一事务**里删除 `doc_updates.seq <= max_seq`。
5. **只读强制在服务端**：`note=ro` 连接照常收到 sync/awareness，但 `apply_client_update`
   直接丢弃其 update（返回 `None`，不写库、不广播、不报错）。
6. **握手**：WebSocket 首帧 JSON（30 秒内），成功后进入上面的二进制帧。
   - 分享：`{"v":1,"mode":"share","token":"…","object":"<workspace id>"}`
   - 所有者：`{"v":1,"mode":"owner","device":"…","object":"…","nonce":"…","ts":…,"sig":"…"}`，
     签名素材 `method=WS, path=/ws, body=原始握手 JSON`。
7. **恢复**：快照 `yjs_state` → 重放 `doc_updates`（按 seq）→ 与关闭前内容/状态向量一致。

## 后果

- 好处：`Doc` 可直接共享；依赖树只有 `yrs`（不再引入一个固定旧版 `yrs` 的 `y-sync`）；
  线格式与 y-websocket 兼容，Web 客户端可继续用标准协议。
- 代价：`wire.rs` 是我们维护的协议代码（约 200 行，含 varint 与帧布局测试）；
  升级 y-protocol 时需要跟 `y-websocket` 对照。
- 风险：`yrs` 0.28 的 API 与 0.17 不同（`Text::insert` 等），Phase 3 测试覆盖了
  收敛/离线并发/重连/重启/只读，未覆盖 awareness 语义（v1 只转发）。

## 验证

- `crates/server/tests/yjs.rs`：落库后重启可恢复；双客户端经服务端收敛；离线并发收敛；
  只读 update 不落库不生效；快照后尾部被清理且状态不变；超 1 MiB update 被拒。
- `crates/server/src/wire.rs`：varint 往返/截断/溢出；四种帧往返；固定字节布局断言。
