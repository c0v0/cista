# ADR 0007：限速算法与 key 选择

- 状态：接受（Phase 6 第一项）
- 日期：2026-10-24

## 背景

`docs/specs/phase6-hardening.md` §1 要求按 IP / 设备 / 链接三个维度限速，429 + `Retry-After`，
且不依赖外部存储（单实例部署，`design.md` §2.3）。

约束：

- 服务端在 Caddy 之后，`ConnectInfo` 拿到的对端地址在测试里不存在（`oneshot` 无连接信息）；
  引入 `X-Forwarded-For` 又需要先确定可信代理，否则可被伪造绕过。
- 公网请求唯一的可信身份是分享 token 解析出的 `share_links.id`；所有者请求唯一的可信身份
  是验签后的 `devices.id`。
- 限速必须在**验签/查库之后**扣令牌，否则伪造 device id / 随机 token 的请求会在桶里建 key。

## 选择

1. **算法**：进程内令牌桶。每 key 懒创建，`tokens` 上限 = `burst`，按 `per_minute/60` 每秒补充；
   时间倒退（NTP 调整）按 0 处理，不补令牌。桶数量 > 4096 时清理 5 分钟未更新的条目。
2. **key**：
   - `ip:<peer>`：最外层中间件，覆盖未认证洪泛；拿不到对端地址时落 `ip:unknown`
     （单实例共用一个桶）；`/healthz` 不计数。
   - `/api/*`：`dev:<device_id>`，在 `require_signed` 验签 + 防重放成功后扣令牌。
   - `/public/*`：`share:<link_id>`，在 token 命中（`fetch_link` 成功）后扣令牌；
     失败的 token 不建桶。
3. **配置**：`CISTA_RATE_LIMIT_PER_MIN`（默认 240）、`CISTA_RATE_LIMIT_BURST`
   （默认 `per_minute/2`，至少 1）。三个维度共用同一组参数。
4. **认证失败退避**（`FailureTracker`）：key 用 `dev:<id>`（header 里没有合法 id 时落 `dev:anonymous`），
   5 分钟窗口内连续失败 5 次后按 `1/2/4/…`（上限 60s）锁定；成功验签清零。
   锁定在验签**之前**检查，避免失败请求反复做签名校验。
5. **响应**：429，体 `{"error":"rate_limited"}`，头 `Retry-After: <秒>`（`ceil(缺口/补充速率)`，至少 1）。
   桶状态（剩余令牌、key）不进响应也不进日志。

## 后果

- 好处：零依赖、可单测（注入时间）、不泄漏身份；伪造凭据的洪泛不消耗内存。
- 代价：进程重启丢失计数（单实例可接受）；`ip:unknown` 在没有连接信息时会把所有匿名请求
  合并成一个桶（对单实例是保守行为）；`drop` 链接暂未使用更严的额度。
- 未做：反代侧的全局限速（应用层已兜底），`drop` 更严额度。

## 验证

- `crates/server/src/rate_limit.rs`：突发耗尽、按 key 隔离、随时间补充、`Retry-After` 计算、
  时间倒退不补令牌；失败退避的阈值、增长与上限、成功清零、窗口重置（共 7 项单元测试）。
- `crates/server/tests/ratelimit.rs`：设备/链接维度、未认证洪泛按 IP 限速、
  连续伪造签名进入退避（4 项集成测试，不依赖墙钟顺序）。
