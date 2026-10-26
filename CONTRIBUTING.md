# 贡献指南

## 环境

- Rust 1.99+、Node 22+、pnpm 9、Linux 上编桌面端还需要 GTK/WebKit 开发包
  （`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev pkg-config`）。
- 没有这些依赖时，`cargo test -p cista-core -p cista-server` 与 web 测试仍可跑。

## 提交前

```bash
./scripts/verify.sh      # web build → fmt → clippy → test → deny → web tests
```

要求：

1. 测试先红后绿；不要为了过 CI 改断言。
2. 改动 crypto / auth / schema / 同步协议前先写 `docs/specs` 或 ADR，并单独 PR。
3. 服务端不得出现私有明文；私有内容只在客户端解密。
4. 分享 token 不进 URL/query/日志；只走 `Authorization: Share` 头。
5. 一个新的 commit 只做一件事；影响 `docs/design.md` 的改动同提交更新。
6. 不引入未在 PR 说明里列出的新依赖。

## 代码结构

- `crates/core`：协议、加密、B+、同步状态机（无 IO，可 wasm）。
- `crates/server`：axum + 两份 SQLite（`private.db` / `shared.db`）。
- `web`：React SPA（访客分享页、配对与笔记编辑器）。
- `apps/desktop`：Tauri 2 壳，复用 `web/`。

## 报告问题

- 功能缺陷 → issue（模板已含版本、复现步骤、期望/实际）。
- 功能建议 → feature request 模板；先说明要解决的问题。
- 安全漏洞 → 按 `SECURITY.md` 走私密报告，**不要开公开 issue**。
- 参与讨论与提交 PR 前请遵守 `CODE_OF_CONDUCT.md`；PR 按
  `.github/PULL_REQUEST_TEMPLATE.md` 填写实际验证命令与结果。
