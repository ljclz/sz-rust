#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 SZ-Rust Team
#
# sz300 数据库备份 / binlog 增量 / 恢复演练
#
# 背景：审计发现只有"备份"口径，没有恢复验证 —— 未验证的备份等于没有备份。
# 本脚本把三件事固化成可重复命令：
#   full    逻辑全量备份（mysqldump --single-transaction，含 binlog 位点）
#   binlog  归档当前 binlog（增量恢复用）
#   drill   从最新全量备份恢复到临时库 <db>_restore_check，比对行数后清理
#
# 用法：
#   export SZ_BACKUP_HOST=127.0.0.1 SZ_BACKUP_PORT=3306
#   export SZ_BACKUP_USER=backup SZ_BACKUP_PASSWORD=***
#   export SZ_BACKUP_DB=sz300 BACKUP_DIR=/data/backup/sz300
#   ./scripts/db/backup-restore-drill.sh full
#   ./scripts/db/backup-restore-drill.sh binlog
#   ./scripts/db/backup-restore-drill.sh drill
#
# 安全约束：
#   - drill 只创建/删除 `<db>_restore_check`，绝不触碰源库
#   - 口令经 MYSQL_PWD 传递（不落 ps 参数），但仍建议用只读/备份专用账号

set -euo pipefail

: "${SZ_BACKUP_HOST:=127.0.0.1}"
: "${SZ_BACKUP_PORT:=3306}"
: "${SZ_BACKUP_USER:?需要设置 SZ_BACKUP_USER}"
: "${SZ_BACKUP_PASSWORD:?需要设置 SZ_BACKUP_PASSWORD}"
: "${SZ_BACKUP_DB:?需要设置 SZ_BACKUP_DB}"
: "${BACKUP_DIR:=./backups}"

export MYSQL_PWD="$SZ_BACKUP_PASSWORD"
MYSQL=(mysql -h "$SZ_BACKUP_HOST" -P "$SZ_BACKUP_PORT" -u "$SZ_BACKUP_USER")
DUMP=(mysqldump -h "$SZ_BACKUP_HOST" -P "$SZ_BACKUP_PORT" -u "$SZ_BACKUP_USER")

ts() { date -u +%Y%m%dT%H%M%SZ; }
die() { echo "ERROR: $*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "缺少命令 $1"; }

cmd_full() {
  need mysqldump
  mkdir -p "$BACKUP_DIR"
  local stamp out pos
  stamp="$(ts)"
  out="$BACKUP_DIR/${SZ_BACKUP_DB}_full_${stamp}.sql.gz"
  pos="$BACKUP_DIR/${SZ_BACKUP_DB}_full_${stamp}.pos"

  # SHOW MASTER STATUS 在 8.0 返回 5 列（含空的 Binlog_Do_DB/Executed_Gtid_Set），
  # -N 下会带一串空字段的制表符；只留 <file> <position>，便于直接喂给 --start-position
  "${MYSQL[@]}" -N -e "SHOW MASTER STATUS" | awk '{print $1" "$2}' > "$pos" || true

  # --single-transaction：InnoDB 一致性快照，不锁表
  # --source-data=2：把 binlog 位点写进 dump 注释（MySQL 8.0.26+）
  "${DUMP[@]}" \
    --single-transaction --quick --routines --triggers --events \
    --source-data=2 --hex-blob --default-character-set=utf8mb4 \
    --databases "$SZ_BACKUP_DB" | gzip -c > "$out"

  echo "全量备份完成: $out"
  echo "binlog 位点: $(cat "$pos" 2>/dev/null || echo '未知')"
}

cmd_binlog() {
  need mysqlbinlog
  mkdir -p "$BACKUP_DIR/binlog"
  local files
  files="$("${MYSQL[@]}" -N -e "SHOW BINARY LOGS" | awk '{print $1}')"
  [ -n "$files" ] || die "未开启 binlog（log_bin=ON 才能做增量恢复）"
  for f in $files; do
    # --raw 时 -r 指向目录（末尾带 /）：输出名 = 目录 + binlog 文件名。
    # 传完整路径会让 mysqlbinlog 再追加一次文件名（binlog.000001binlog.000001）。
    mysqlbinlog -h "$SZ_BACKUP_HOST" -P "$SZ_BACKUP_PORT" -u "$SZ_BACKUP_USER" \
      -R --raw -r "$BACKUP_DIR/binlog/" "$f"
  done
  echo "binlog 归档完成: $BACKUP_DIR/binlog（$(( $(echo "$files" | wc -l) )) 个文件）"
}

cmd_drill() {
  need mysqldump
  local dump scratch
  dump="$(ls -1t "$BACKUP_DIR"/${SZ_BACKUP_DB}_full_*.sql.gz 2>/dev/null | head -1 || true)"
  [ -n "$dump" ] || die "找不到全量备份，请先执行 full"
  scratch="${SZ_BACKUP_DB}_restore_check"

  echo "恢复演练：$dump → $scratch"
  "${MYSQL[@]}" -e "DROP DATABASE IF EXISTS \`$scratch\`; CREATE DATABASE \`$scratch\` CHARACTER SET utf8mb4"

  # dump 里带 CREATE DATABASE/USE 原库名，恢复时重定向到临时库
  gunzip -c "$dump" | sed -e "s/\`${SZ_BACKUP_DB}\`/\`${scratch}\`/g" \
    | "${MYSQL[@]}" "$scratch"

  local fail=0 table src dst
  for table in $("${MYSQL[@]}" -N -e \
      "SELECT table_name FROM information_schema.tables WHERE table_schema='${SZ_BACKUP_DB}' AND table_type='BASE TABLE'"); do
    src="$("${MYSQL[@]}" -N -e "SELECT COUNT(*) FROM \`${SZ_BACKUP_DB}\`.\`${table}\`")"
    dst="$("${MYSQL[@]}" -N -e "SELECT COUNT(*) FROM \`${scratch}\`.\`${table}\`")"
    if [ "$src" != "$dst" ]; then
      echo "  FAIL $table: 源=$src 恢复=$dst"
      fail=1
    else
      echo "  OK   $table: $src 行"
    fi
  done

  "${MYSQL[@]}" -e "DROP DATABASE \`$scratch\`"

  [ "$fail" -eq 0 ] && echo "恢复演练通过（临时库已清理）" || die "恢复演练失败"
}

case "${1:-}" in
  full)   cmd_full ;;
  binlog) cmd_binlog ;;
  drill)  cmd_drill ;;
  *) echo "用法: $0 {full|binlog|drill}" >&2; exit 2 ;;
esac
