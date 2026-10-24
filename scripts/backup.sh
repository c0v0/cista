#!/usr/bin/env bash
# 每日备份（参考实现，Phase 5）：两份 SQLite 一致性快照 + blob 清单 + restic。
#
# 设计要点（design.md §2.6、§10）：
# - VACUUM INTO，不是 cp 活库。
# - 快照引用的 blob 必须在清单里；清单随快照提交。
# - restic 仓库加密、只追加；保留 7 daily / 4 weekly / 6 monthly。
# - 失败写 journal（systemd 捕获 stderr）并以非零退出。
#
# 用法：
#   CISTA_DATA_DIR=/srv/cista/data \
#   CISTA_BACKUP_DIR=/srv/cista/backup \
#   RESTIC_REPOSITORY=sftp:backup@host:/srv/restic/cista \
#   RESTIC_PASSWORD_FILE=/etc/cista/restic.pass \
#   scripts/backup.sh
set -euo pipefail

DATA_DIR=${CISTA_DATA_DIR:-/srv/cista/data}
BACKUP_ROOT=${CISTA_BACKUP_DIR:-/srv/cista/backup}
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
SNAP="$BACKUP_ROOT/snapshots/$STAMP"

log() { printf '%s %s\n' "$(date -Is)" "$*"; }
fail() {
  log "backup FAILED: $*"
  exit 1
}

mkdir -p "$SNAP" || fail "创建快照目录 $SNAP"
[ -f "$DATA_DIR/private.db" ] || fail "缺少 $DATA_DIR/private.db"
[ -f "$DATA_DIR/shared.db" ] || fail "缺少 $DATA_DIR/shared.db"
command -v sqlite3 >/dev/null 2>&1 || fail "缺少 sqlite3"

# 1) 一致性快照（不是 cp 活库）
sqlite3 "$DATA_DIR/private.db" "VACUUM INTO '$SNAP/private.db'" || fail "私有库快照"
sqlite3 "$DATA_DIR/shared.db" "VACUUM INTO '$SNAP/shared.db'" || fail "共享库快照"
log "snapshots: $(du -sh "$SNAP" | cut -f1)"

# 2) blob 清单：hash 与大小（恢复时用来校验快照引用的 blob 都在）
for scope in private shared; do
  dir="$DATA_DIR/blobs/$scope"
  [ -d "$dir" ] || continue
  (cd "$dir" && find . -maxdepth 1 -type f -printf '%P %s\n' | sort) >"$SNAP/blobs-$scope.txt"
done

# 3) restic：加密、只追加。没有仓库配置时只保留本地快照（不满足三把钥匙要求）
if [ -n "${RESTIC_REPOSITORY:-}" ]; then
  command -v restic >/dev/null 2>&1 || fail "缺少 restic"
  restic backup --tag cista "$SNAP" "$DATA_DIR/blobs" || fail "restic backup"
  restic forget --tag cista \
    --keep-daily 7 --keep-weekly 4 --keep-monthly 6 --prune || fail "restic forget"
  log "restic: snapshot uploaded"
else
  log "WARN: RESTIC_REPOSITORY 未设置，只保留本地快照"
fi

# 4) 本地快照保留 14 天（长期保留交给 restic）
find "$BACKUP_ROOT/snapshots" -mindepth 1 -maxdepth 1 -type d -mtime +14 -exec rm -rf {} + 2>/dev/null || true

log "backup ok: $SNAP"
