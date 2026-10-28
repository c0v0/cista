#!/usr/bin/env bash
# 恢复演练（Phase 5）：从备份恢复两份 DB + blob → 完整性校验 → 起服务探活 → 记录耗时。
#
# 自动化的部分：restic/本地快照恢复、`PRAGMA integrity_check`、blob 清单对账、
# 启动服务并请求 /healthz、记录耗时。
# 人工的部分（发布检查 P10）：用 MK 解开一条私有笔记、打开一次分享链接。
#
# 用法：
#   RESTIC_REPOSITORY=… RESTIC_PASSWORD_FILE=… scripts/restore-drill.sh /tmp/drill
#   （无 restic 时：CISTA_BACKUP_DIR=/srv/cista/backup scripts/restore-drill.sh /tmp/drill）
set -euo pipefail

WORK=${1:?用法: scripts/restore-drill.sh <空目录>}
DATA_DIR=${CISTA_DATA_DIR:-/srv/cista/data}
BACKUP_DIR=${CISTA_BACKUP_DIR:-/srv/cista/backup}
SERVER_BIN=${CISTA_BINARY:-/usr/local/bin/cista-server}
PORT=${CISTA_DRILL_PORT:-18787}
START=$(date +%s)

log() { printf '%s %s\n' "$(date -Is)" "$*"; }
fail() {
  log "restore drill FAILED: $*"
  exit 1
}

[ -d "$WORK" ] || mkdir -p "$WORK"
[ -z "$(ls -A "$WORK")" ] || fail "$WORK 不是空目录（演练要全新目录）"
command -v sqlite3 >/dev/null 2>&1 || fail "缺少 sqlite3"

mkdir -p "$WORK/restore"
if [ -n "${RESTIC_REPOSITORY:-}" ]; then
  command -v restic >/dev/null 2>&1 || fail "缺少 restic"
  log "restic restore latest → $WORK/restore"
  restic restore latest --target "$WORK/restore" \
    --include "$BACKUP_DIR/snapshots" --include "$DATA_DIR/blobs" \
    || fail "restic restore"
else
  log "无 restic 仓库：使用本地快照目录 $BACKUP_DIR"
  SNAP=$(find "$BACKUP_DIR/snapshots" -mindepth 1 -maxdepth 1 -type d | sort | tail -1)
  [ -n "$SNAP" ] || fail "没有本地快照"
  mkdir -p "$WORK/restore$(dirname "$BACKUP_DIR")"
  cp -a "$SNAP" "$WORK/restore$(dirname "$BACKUP_DIR")/" || fail "复制本地快照"
  cp -a "$DATA_DIR/blobs" "$WORK/restore$(dirname "$DATA_DIR")/" 2>/dev/null || true
fi

SNAP=$(find "$WORK/restore" -type d -path '*snapshots/*' -mindepth 1 | sort | tail -1)
[ -n "$SNAP" ] || fail "恢复结果里找不到快照目录"
BLOBS=$(find "$WORK/restore" -type d -path '*blobs' | head -1)
[ -n "$BLOBS" ] || fail "恢复结果里找不到 blobs 目录"
log "snapshot: $SNAP"
log "blobs:    $BLOBS"

DRILL_DATA="$WORK/data"
mkdir -p "$DRILL_DATA"
cp "$SNAP/private.db" "$DRILL_DATA/private.db" || fail "复制 private.db"
cp "$SNAP/shared.db" "$DRILL_DATA/shared.db" || fail "复制 shared.db"
cp -a "$BLOBS" "$DRILL_DATA/blobs" || fail "复制 blobs"

# 完整性校验：两份库都必须 ok
for db in private shared; do
  CHECK=$(sqlite3 "$DRILL_DATA/$db.db" "PRAGMA integrity_check;")
  [ "$CHECK" = "ok" ] || fail "$db.db integrity_check: $CHECK"
  log "$db.db integrity_check: ok"
done

# blob 清单对账：快照里列出的每个 hash 都必须在恢复后的 blob 目录里，且大小一致
for scope in private shared; do
  manifest="$SNAP/blobs-$scope.txt"
  [ -f "$manifest" ] || continue
  missing=0
  while read -r hash size; do
    file="$DRILL_DATA/blobs/$scope/$hash"
    if [ ! -f "$file" ]; then
      log "MISSING blob $scope/$hash"
      missing=$((missing + 1))
      continue
    fi
    actual=$(stat -c %s "$file")
    [ "$actual" = "$size" ] || {
      log "SIZE MISMATCH $scope/$hash expected=$size actual=$actual"
      missing=$((missing + 1))
    }
  done <"$manifest"
  [ "$missing" -eq 0 ] || fail "$scope blobs: $missing 个不一致"
  log "$scope blobs: $(wc -l <"$manifest") 个全部命中"
done

# 起服务探活（恢复出来的库必须能被迁移与启动流程接受）
[ -x "$SERVER_BIN" ] || fail "找不到服务端二进制：$SERVER_BIN"
CISTA_DATA_DIR="$DRILL_DATA" CISTA_BIND="127.0.0.1:$PORT" CISTA_LOG_FORMAT=pretty \
  CISTA_STATIC_KEY="${CISTA_STATIC_KEY:?需要 CISTA_STATIC_KEY（恢复演练必须用真实密钥启动）}" \
  "$SERVER_BIN" >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT

ready=0
for _ in $(seq 1 40); do
  if curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.25
done
[ "$ready" -eq 1 ] || fail "服务未在 10s 内就绪（见 $WORK/server.log）"
log "/healthz: ok"

ELAPSED=$(( $(date +%s) - START ))
log "restore drill OK in ${ELAPSED}s"
printf '| %s | %s | %ss | %s |\n' \
  "$(date -u +%Y-%m-%dT%H:%MZ)" "$(basename "$SNAP")" "$ELAPSED" "${DRILL_NOTE:-手工部分待填}" \
  | tee "$WORK/drill-row.md"
log "把 $WORK/drill-row.md 追加到 docs/specs/restore-drill-log.md（含 MK 解密的人工结论）"
