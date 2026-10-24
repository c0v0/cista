#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== rustfmt =="
cargo fmt --all -- --check

echo "== clippy =="
cargo clippy --workspace --all-targets -- -D warnings

echo "== test =="
cargo test --workspace

echo "== deny =="
cargo deny check

if [ -f web/package.json ]; then
  echo "== web =="
  pnpm -C web install --frozen-lockfile
  pnpm -C web typecheck
  pnpm -C web test -- --run
fi

echo "verify: OK"
