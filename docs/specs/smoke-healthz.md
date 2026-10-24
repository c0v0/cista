# 规格：Phase 0 冒烟任务 —— `GET /healthz`

> 来源：`phase0.md` §8「第一个冒烟任务（验证流程，不是验证业务）」。
> 目标不是功能，而是验证「规格 → 计划 → 红绿 → verify → 审查 → commit」这条流水线能跑通。

## 目标 / 非目标

- 目标：公网可达的 `GET /healthz` 返回 `200`，响应体 `ok`，供 systemd/Docker healthcheck 与外部 uptime 使用。
- 非目标：不做 `/readyz`、不做 `/metrics`、不引入 Prometheus（`design.md` §4.4）。

## 验收标准（可执行）

1. `curl -i http://127.0.0.1:8787/healthz` → `HTTP/1.1 200`，body 为 `ok`，无 `Set-Cookie`。
2. 无鉴权：不带任何 `X-Device-Id`/`Authorization` 头也能 200；且不读取任何表（不依赖 DB 可用）。
3. 非 GET（`POST /healthz`）→ `405`。
4. 未定义路径（`GET /nope`）→ `404`，body 不含内部路径/版本细节。
5. 测试 `crates/server/tests/healthz.rs` 覆盖 1–4，且实现前必然是红的。
6. `./scripts/verify.sh` 全绿（含 `cargo clippy --workspace --all-targets -- -D warnings`）。

## 影响面

| 面 | 改/不改 |
|---|---|
| server | 改：新增 `crates/server/src/api/health.rs`，路由挂在 `/healthz` |
| db | 不改 |
| core | 不改 |
| desktop | 不改 |
| web | 不改（uptime 直接打服务端） |
| android | 不改 |

## 需要的 ADR

无（不碰 crypto/auth/schema）。

## 待确认

- 部署时 `/healthz` 是否暴露在反代公网段：是（`design.md` §2.3 要求公网部署必需）。
