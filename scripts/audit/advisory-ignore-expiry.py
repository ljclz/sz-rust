#!/usr/bin/env python3
"""deny.toml advisory ignore 复核期限门禁（2026-10-28 P1-6）

每条 `ignore` 条目必须：
1. 行尾带 `# review-by=YYYY-MM-DD`
2. 复核期限未过期

用途：RUSTSEC 公告被临时忽略时必须带到期日，避免"永久静音"。
到期后 CI 失败，强制升级上游依赖或重新评估暴露面。

退出码：0 = 合规；1 = 缺到期日 / 已过期 / 重复条目
"""
import datetime
import re
import sys

DENY_TOML = 'deny.toml'
ENTRY = re.compile(r'^\s*\{ id = "(RUSTSEC-\d{4}-\d{4})".*?\},?(?:\s*#\s*(.*))?$')
REVIEW = re.compile(r'review-by=(\d{4}-\d{2}-\d{2})')


def main(path=DENY_TOML):
    today = datetime.date.today()
    errors = []
    seen = {}
    annotated = 0

    with open(path, encoding='utf-8') as f:
        lines = f.readlines()

    in_ignore = False
    for lineno, line in enumerate(lines, 1):
        stripped = line.strip()
        if stripped.startswith('ignore = ['):
            in_ignore = True
            continue
        if in_ignore and stripped.startswith(']'):
            in_ignore = False
            continue
        if not in_ignore:
            continue
        m = ENTRY.match(line.rstrip('\n'))
        if not m:
            continue
        advisory, comment = m.group(1), m.group(2) or ''
        annotated += 1

        if advisory in seen:
            errors.append(
                '%s:%d 重复忽略 %s（首次出现在第 %d 行）' % (path, lineno, advisory, seen[advisory])
            )
        else:
            seen[advisory] = lineno

        date_m = REVIEW.search(comment)
        if not date_m:
            errors.append('%s:%d %s 缺少 `# review-by=YYYY-MM-DD`' % (path, lineno, advisory))
            continue
        try:
            due = datetime.date.fromisoformat(date_m.group(1))
        except ValueError:
            errors.append('%s:%d %s review-by 日期格式非法: %s'
                          % (path, lineno, advisory, date_m.group(1)))
            continue
        if due < today:
            errors.append('%s:%d %s 复核期限已过（%s < %s）'
                          % (path, lineno, advisory, due, today))

    print('advisory ignores: %d annotated' % annotated)
    if annotated == 0:
        errors.append('%s 中未找到任何 ignore 条目（解析失败或文件被清空）' % path)

    upcoming = []
    for advisory, lineno in sorted(seen.items(), key=lambda kv: kv[1]):
        line = lines[lineno - 1]
        date_m = REVIEW.search(line)
        if date_m:
            days = (datetime.date.fromisoformat(date_m.group(1)) - today).days
            if 0 <= days <= 45:
                upcoming.append('%s 将于 %d 天后到期' % (advisory, days))
    for note in upcoming:
        print('  WARN: %s' % note)

    if errors:
        for e in errors:
            print('FAIL: %s' % e)
        return 1
    print('PASS: %d 条 advisory ignore 均带有效复核期限' % annotated)
    return 0


if __name__ == '__main__':
    sys.exit(main(*sys.argv[1:]))
