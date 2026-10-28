#!/usr/bin/env bash
# 迁移演练：副本上前向迁移 + “回滚 = 恢复迁移前快照”的数据路径验证。
#
# 用法：
#   CISTA_DATA_DIR=/srv/cista/data CISTA_BINARY=/usr/local/bin/cista-server \
#     scripts/migration-drill.sh <空目录>
#
# 说明：本仓库只保留前向迁移（无 down 脚本）。二进制回滚的正确做法是
# 「换回上一版二进制 + 恢复升级前备份」；本脚本验证的是备份可被当前二进制重新接受。
set -euo pipefail

WORK=${1:?用法: scripts/migration-drill.sh <空目录>}
DATA_DIR=${CISTA_DATA_DIR:?需要 CISTA_DATA_DIR}
SERVER_BIN=${CISTA_BINARY:?需要 CISTA_BINARY}
PORT=${CISTA_DRILL_PORT:-18789}

command -v sqlite3 >/dev/null 2>&1 || {
  echo "缺少 sqlite3"
  exit 1
}
[ -x "$SERVER_BIN" ] || {
  echo "找不到二进制：$SERVER_BIN"
  exit 1
}
mkdir -p "$WORK"
[ -z "$(ls -A "$WORK")" ] || {
  echo "$WORK 不是空目录"
  exit 1
}
for db in private shared; do
  [ -f "$DATA_DIR/$db.db" ] || {
    echo "缺少 $DATA_DIR/$db.db"
    exit 1
  }
done

mkdir -p "$WORK/pre" "$WORK/forward" "$WORK/rollback"
echo "== 迁移前快照 =="
for db in private shared; do
  sqlite3 "$DATA_DIR/$db.db" "VACUUM INTO '$WORK/pre/$db.db'"
done
if [ -d "$DATA_DIR/blobs" ]; then
  cp -a "$DATA_DIR/blobs" "$WORK/pre/blobs"
fi

start_server() {
  local data=$1
  CISTA_DATA_DIR="$data" CISTA_BIND="127.0.0.1:$PORT" CISTA_LOG_FORMAT=pretty \
    CISTA_STATIC_KEY="${CISTA_STATIC_KEY:?需要 CISTA_STATIC_KEY（迁移演练必须用真实密钥启动）}" \
    "$SERVER_BIN" >"$data/server.log" 2>&1 &
  local pid=$!
  for _ in $(seq 1 40); do
    if curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then
      echo "$pid"
      return 0
    fi
    sleep 0.25
  done
  kill "$pid" 2>/dev/null || true
  return 1
}

echo "== 前向：在副本上应用当前迁移 =="
cp -a "$WORK/pre/." "$WORK/forward/"
PID=$(start_server "$WORK/forward") || {
  echo "前向启动失败"
  tail -5 "$WORK/forward/server.log" || true
  exit 1
}
kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true

echo "== 回滚路径：恢复迁移前快照并可再次启动 =="
cp -a "$WORK/pre/." "$WORK/rollback/"
PID=$(start_server "$WORK/rollback") || {
  echo "回滚启动失败"
  tail -5 "$WORK/rollback/server.log" || true
  exit 1
}
kill "$PID" 2>/dev/null || true
wait "$PID" 2>/dev/null || true

echo "== 完整性校验 =="
for db in private shared; do
  CHECK=$(sqlite3 "$WORK/forward/$db.db" "PRAGMA integrity_check;")
  [ "$CHECK" = "ok" ] || {
    echo "$db.db: $CHECK"
    exit 1
  }
done

echo "migration drill OK（前向迁移 + 迁移前快照可启动）"
