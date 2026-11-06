#!/usr/bin/env python3
"""
批量改写 #[cfg(test)] 模块内的 panic! 当断言用 为 assert!(matches!) 形式。

处理的模式（match 块的最后一个 arm 是 other => panic!("msg: {other:?}")）：
  模式 A（空体 arm）:
    match <expr> {
        Pat => {}
        other => panic!("msg: {other:?}"),
    }
    →
    let __err = <expr>;
    assert!(matches!(__err, Pat), "msg: {__err:?}");

  模式 B（arm 内有断言）:
    match <expr> {
        Pat(field) => <assertion>,
        other => panic!("msg: {other:?}"),
    }
    →
    let __err = <expr>;
    assert!(matches!(&__err, &Pat(_)), "msg: {__err:?}");
    if let Pat(field) = __err {
        <assertion>;
    }

注意：仅处理 expr 是简单变量或 result.unwrap_err() 等的情况。
对于复杂情况（多 arm panic、should_panic、let-else else 块），跳过。
"""
import re
import sys
from pathlib import Path


def find_match_blocks(lines):
    """找到所有 `match <expr> {` 块的起始行号（0-indexed）和结束行号。"""
    blocks = []
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        # 匹配 `match <expr> {` 行（可能有前导空格）
        m = re.match(r'^(\s*)match\s+(.+?)\s*\{\s*$', line)
        if m:
            indent = m.group(1)
            expr = m.group(2)
            # 找到闭合的 `}`（考虑嵌套）
            depth = 1
            j = i + 1
            while j < n and depth > 0:
                for ch in lines[j]:
                    if ch == '{':
                        depth += 1
                    elif ch == '}':
                        depth -= 1
                if depth == 0:
                    break
                j += 1
            if j < n:
                blocks.append((i, j, indent, expr))
                i = j + 1
                continue
        i += 1
    return blocks


def parse_arm(lines, start, end, base_indent):
    """解析 match arm，返回 (pat_str, body_str, body_lines) 或 None。"""
    # arm 内容从 start 到 end
    arm_text = ''.join(lines[start:end+1])
    return arm_text


def try_rewrite_block(lines, block_start, block_end, indent, expr):
    """尝试改写一个 match 块。返回改写后的行列表或 None（如果不匹配模式）。"""
    inner_lines = lines[block_start+1:block_end]  # 不含 match 行和闭合 }
    # 去掉空行
    inner = [l for l in inner_lines if l.strip()]
    if len(inner) < 2:
        return None

    # 最后一个 arm 应该是 other => panic!("msg: {other:?}"), 或 _ => panic!(...),
    last_arm = inner[-1]
    # 匹配 other => panic!("msg: {other:?}"), 或 _ => panic!(...),
    panic_m = re.match(r'^\s*(other|_)\s*=>\s*panic!\((.+?)\)\s*,?\s*$', last_arm.rstrip())
    if not panic_m:
        return None
    panic_msg = panic_m.group(2)

    # 决定变量名：如果 expr 是简单标识符，直接用；否则引入 __err
    is_simple_var = bool(re.match(r'^[a-z_][a-z0-9_]*$', expr))
    if is_simple_var:
        var = expr
        let_line = None
    else:
        var = '__err'
        let_line = f'{indent}let __err = {expr};\n'

    # 把 panic_msg 中的 {other:?}/{_:?} 替换为 {var:?}
    msg = panic_msg.replace('{other:?}', '{' + var + ':?}').replace('{_:?}', '{' + var + ':?}')
    # 把引号外的 other/_ 标识符（格式参数）替换为 var
    # 简单方法：按引号分段，只替换偶数段（引号外）的 other/_
    parts = msg.split('"')
    for idx in range(1, len(parts), 2):  # 奇数索引 = 引号内，偶数索引 = 引号外
        pass
    for idx in range(0, len(parts), 2):  # 偶数索引 = 引号外
        parts[idx] = re.sub(r'\bother\b', var, parts[idx])
        parts[idx] = re.sub(r'\b_\b(?!:)', var, parts[idx])  # _ 但不是 :_ (即不是 {:?})
    msg = '"'.join(parts)

    # 如果只有一个前面的 arm
    if len(inner) == 2:
        first_arm = inner[0]
        # 模式 A: Pat => {}  或  Pat { .. } => {}  或  Pat(_) => {}
        arm_a = re.match(r'^\s*(.+?)\s*=>\s*\{\s*\}\s*,?\s*$', first_arm.rstrip())
        if arm_a:
            pat = arm_a.group(1).strip()
            new_lines = []
            if let_line:
                new_lines.append(let_line)
            new_lines.append(f'{indent}assert!(matches!({var}, {pat}), {msg});\n')
            return new_lines

        # 模式 B: Pat(field) => <assertion>,
        arm_b = re.match(r'^\s*(.+?)\s*=>\s*(.+?)\s*,?\s*$', first_arm.rstrip())
        if arm_b:
            lhs = arm_b.group(1).strip()
            body = arm_b.group(2).strip()

            # 构造用于 matches! 的模式（把绑定替换为 _）
            matches_pat = lhs
            if '(' in lhs:
                matches_pat = re.sub(r'\([^)]*\)', '(_)', lhs)
            if '{' in lhs:
                matches_pat = re.sub(r'\{[^}]*\}', '{ .. }', lhs)

            iflet_pat = lhs
            body_with_semi = body if body.endswith(';') or body.endswith('}') else body + ';'
            new_lines = []
            if let_line:
                new_lines.append(let_line)
            new_lines.append(f'{indent}assert!(matches!(&{var}, &{matches_pat}), {msg});\n')
            new_lines.append(f'{indent}if let {iflet_pat} = {var} {{\n')
            new_lines.append(f'{indent}    {body_with_semi}\n')
            new_lines.append(f'{indent}}}\n')
            return new_lines

    return None


def process_file(filepath):
    """处理单个文件，返回改写的块数。"""
    path = Path(filepath)
    text = path.read_text(encoding='utf-8')
    lines = text.splitlines(keepends=True)

    # 找到所有 match 块
    blocks = find_match_blocks(lines)

    # 从后往前改写（避免行号偏移）
    rewritten = 0
    skipped = 0
    for start, end, indent, expr in reversed(blocks):
        new_block = try_rewrite_block(lines, start, end, indent, expr)
        if new_block is not None:
            # 检查改写后的代码不包含 panic!
            if not any('panic!' in l for l in new_block):
                lines[start:end+1] = new_block
                rewritten += 1
            else:
                skipped += 1
        else:
            skipped += 1

    if rewritten > 0:
        path.write_text(''.join(lines), encoding='utf-8')

    return rewritten, skipped


def main():
    if len(sys.argv) < 2:
        print("Usage: rewrite_panic_assert.py <file1> [file2] ...")
        sys.exit(1)

    total_rewritten = 0
    total_skipped = 0
    for f in sys.argv[1:]:
        rewritten, skipped = process_file(f)
        if rewritten > 0:
            print(f"  {f}: 改写 {rewritten} 处, 跳过 {skipped} 处")
        total_rewritten += rewritten
        total_skipped += skipped

    print(f"\n总计: 改写 {total_rewritten} 处, 跳过 {total_skipped} 处")


if __name__ == '__main__':
    main()