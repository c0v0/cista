#!/usr/bin/env bash
# 产出桌面安装包（默认 deb；也可传 "deb,appimage"）。
#
# 前置：GTK/WebKit 开发包、pnpm、cargo（见 README「桌面端」）。
# 用法：scripts/package-desktop.sh [bundles]
#
# 说明：`tauri build` 的 `beforeBuildCommand` 会按 CLI 的 cwd 解析相对路径，
# 在不同调用方式下会指错目录；这里先自行构建 web，再用 --config 清空该命令。
set -euo pipefail
cd "$(dirname "$0")/.."
BUNDLES=${1:-deb}

pnpm -C web install --frozen-lockfile
pnpm -C web build

cd apps/desktop/src-tauri
pnpm dlx @tauri-apps/cli@2 build --bundles "$BUNDLES" \
  --config '{"build":{"beforeBuildCommand":""}}'

TARGET_ROOT=${CARGO_TARGET_DIR:-"$(cd "$(dirname "$0")/.." && pwd)/target"}
echo "== 产物 =="
find "$TARGET_ROOT/release/bundle" -maxdepth 2 -type f \
  \( -name '*.deb' -o -name '*.AppImage' -o -name '*.rpm' -o -name '*.msi' -o -name '*.exe' \) -print
