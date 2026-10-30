#!/usr/bin/env bash
# 本地与 CI 共用的门禁。
#
# 顺序说明：桌面壳在编译期嵌入前端产物（Tauri `frontendDist` = web/dist），
# 所以 web 必须先 install + build，否则 clippy/test 会在 `generate_context!` 处失败。
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -f web/package.json ]; then
  echo "== web build =="
  pnpm -C web install --frozen-lockfile
  pnpm -C web build
fi

echo "== rustfmt =="
cargo fmt --all -- --check

echo "== clippy =="
cargo clippy --workspace --all-targets -- -D warnings

echo "== test =="
cargo test --workspace

echo "== deny =="
cargo deny check

if [ -f web/package.json ]; then
  echo "== web tests =="
  pnpm -C web typecheck
  pnpm -C web test -- --run
fi

echo "verify: OK"
