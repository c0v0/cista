# 规格：Phase 3 —— Yjs 实时与崩溃恢复

> 验收来源：`design.md` §11「Phase 3：Yjs 实时 + 崩溃恢复」→
> 「双客户端收敛；离线并发编辑收敛；只读连接不能发 update；重连补齐；重启不丢已确认 update」。

## 目标 / 非目标

- 目标：共享工作区正文用 `Y.Text`（Markdown 源码）实时协作；服务端持久化 + 快照 + 崩溃恢复；
  只读强制在服务端。
- 非目标：不做历史版本/恢复 UI；不做 awareness 持久化（只在内存转发）；不搜 Yjs 正文（共享搜索只搜标题/文件名）。

## 1. 存储（`design.md` §3.8）

```text
workspace_notes(workspace_id PK, rev, yjs_state BLOB, state_vector BLOB, updated_at)
doc_updates(seq INTEGER PK AUTOINCREMENT, workspace_id, update BLOB, created_at)
```

- 每个工作区一个内存 `Doc`（空闲 10 分钟卸载；卸载前落盘）。
- 每条 update **先同步落库再 ack**（`doc_updates` 追加），保证「重启不丢已确认 update」。
- 快照：每 100 条 update 或 5 分钟做一次「当前状态 + 状态向量」写入 `workspace_notes`；
  快照成功后删除已合并的 `doc_updates`（`seq <= 快照点`）。
- 崩溃恢复：`Doc::load(yjs_state)` → 重放 `doc_updates`（按 `seq`）→ 内存 Doc 与已 ack 状态一致。
- 限制：单文档 5 MiB、单 update 1 MiB、每连接限速（Phase 6 统一参数）；超限 → 关闭连接/拒绝 update。

## 2. 连接与认证（`design.md` §2.3、§3.8）

- `WS /ws`：首条消息必须是 JSON 握手（30 秒内，否则关闭）：

```json
{"v":1,"mode":"share","token":"<share token>","channel":"workspace","object":"<workspace id>"}
{"v":1,"mode":"owner","device":"<device id>","channel":"workspace","object":"<workspace id>",
 "sig":"<hex>","nonce":"<hex>","ts":1730000000}
```

- 握手后进入二进制 Yjs 帧（`y-sync` 协议：sync step1/step2/update、awareness）。
- 访客：token 决定 `note` 权限；`note=off` → 403；`note=ro` → 只读；`note=rw` → 可写。
- 所有者：设备签名（签名串覆盖握手 JSON 的 `sha256` 与 `object`），无条件可写。
- 只读连接：服务端仍发 sync/awareness，但**丢弃**收到的 `update`（并要求客户端侧只读 UI 禁编辑）。
- 工作区过期/撤销：`quarantined`/`revoked` → 握手 410/403；已建立连接在下次 update 时被断开。

## 3. 验收标准（可执行）

`cargo test -p cista-server`（`tests/yjs.rs`）：

1. 双客户端：A 插入 "hello"、B 插入 "world"，双向同步后两边 `Y.Text` 内容与 `state_vector` 一致（收敛）。
2. 离线并发：A、B 各自离线插入不同文本（同一位置），重连交换 update 后收敛到同一字符串。
3. 只读：`note=ro` 连接发 update，服务端不回发该 update 且库中状态不变；`note=rw` 同样操作会变化。
4. 重连补齐：断开 → 本地新增 → 重连（发 state vector）→ 只收到缺失部分，且最终收敛。
5. 重启不丢：写入 → 关闭服务 → 重新加载（新 `Doc` + 重放 `doc_updates`）→ 内容与关闭前一致。
6. 快照合并：超过阈值（测试用 3 条）触发快照 → `doc_updates` 被清理 → 仍能恢复出相同内容与状态向量。
7. 握手失败：无握手/超时/错 token/撤销 → 连接关闭（测试断言 close 且未创建 Doc）。
8. 单 update > 1 MiB → 拒绝（连接关闭或 4001 close code），库中无该 update。

## 影响面

| 面 | 改/不改 |
|---|---|
| server | 改：`ws.rs`（握手 + y-sync 桥接）、`yjs.rs`（Doc 注册表/快照/恢复）、迁移 0003 |
| db | 改：shared.db 增加 `workspace_notes`、`doc_updates` |
| core | 不改（Yjs 不进 core：`core` 禁 IO，且 yrs 是服务端/客户端各自依赖） |
| web/desktop/android | 不改（Phase 4 接 y-websocket 或自研客户端） |

## 需要的 ADR

- 0006：`doc_updates` 与快照的写入顺序（先落库再 ack）与快照点语义（`seq <= snapshot_seq` 删除）。

## 待确认

- 客户端用 `y-websocket`（需要服务端说标准 y-sync 二进制协议）还是自研 WS 封装：
  Phase 4 决定；服务端两种都兼容（`y-sync` 默认协议即标准二进制帧）。
- awareness 是否转发访客身份（v1 只转发匿名光标，不带昵称）。
