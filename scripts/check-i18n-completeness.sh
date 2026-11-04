#!/usr/bin/env bash
# i18n 完整性检查（v1.4.0 T12.1）
# 扫描翻译资源文件，检查中英文键一致性和缺失键
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PYTHON=""
for cand in python3 python; do
  command -v "$cand" >/dev/null 2>&1 && "$cand" -c 'import json' >/dev/null 2>&1 && PYTHON="$cand" && break
done
[ -z "$PYTHON" ] && echo "❌ 需要 Python 3" >&2 && exit 1
echo "▶ i18n 完整性检查…"
"$PYTHON" - "$ROOT_DIR" <<'PY'
import json, sys, os
root = sys.argv[1] if len(sys.argv)>1 else '.'
zh_path = os.path.join(root, 'i18n/zh-cn/errors.json')
en_path = os.path.join(root, 'i18n/en-us/errors.json')
if not os.path.exists(zh_path):
    print(f"❌ 中文翻译文件不存在: {zh_path}"); sys.exit(1)
if not os.path.exists(en_path):
    print(f"❌ 英文翻译文件不存在: {en_path}"); sys.exit(1)
zh = json.load(open(zh_path, encoding='utf-8'))
en = json.load(open(en_path, encoding='utf-8'))
zh_keys = set(zh.keys())
en_keys = set(en.keys())
missing_en = zh_keys - en_keys
missing_zh = en_keys - zh_keys
print(f"  中文键数: {len(zh_keys)}")
print(f"  英文键数: {len(en_keys)}")
if missing_en:
    print(f"❌ 英文缺失键 ({len(missing_en)}): {sorted(missing_en)}")
if missing_zh:
    print(f"❌ 中文缺失键 ({len(missing_zh)}): {sorted(missing_zh)}")
if not missing_en and not missing_zh:
    print("✅ 中英文键集合完全一致")
required = ['error.auth.not_login','error.validation.required','error.http.not_found','error.db.connection','error.system.busy']
missing_required = [k for k in required if k not in zh_keys]
if missing_required:
    print(f"❌ 缺失必需键: {missing_required}"); sys.exit(1)
print("✅ 必需键齐全")
sys.exit(0 if not missing_en and not missing_zh else 1)
PY
