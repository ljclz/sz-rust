import os, re

def strip_scl(line):
    """去字符串字面量 + // 注释"""
    out, i, n, in_str = [], 0, len(line), None
    while i < n:
        ch = line[i]
        if in_str:
            if ch == chr(92) and i+1 < n: i += 2; continue
            if ch == in_str: in_str = None
            i += 1; continue
        if ch in ('"', chr(39)): in_str = ch; i += 1; continue
        if ch == '/' and i+1 < n and line[i+1] == '/': break
        out.append(ch); i += 1
    return ''.join(out)

def scan(path):
    with open(path, encoding='utf-8', errors='replace') as f:
        lines = f.readlines()
    # 状态：in_test_mod（mod tests 块深度）、test_fn（#[test] 函数深度栈）
    prod = []
    mod_depth = 0
    fn_stack = []  # (is_test_fn, brace_depth)
    pending_test = False
    for i, raw in enumerate(lines):
        line = strip_scl(raw).strip()
        if not line: continue
        # 进入 mod tests
        if mod_depth == 0 and re.search(r'#\[cfg\(test\)\]|mod tests\b', line):
            mod_depth = max(1, line.count('{'))
            continue
        if mod_depth > 0:
            mod_depth += line.count('{') - line.count('}')
            if mod_depth <= 0: mod_depth = 0
            continue
        # #[test] / #[tokio::test] 属性 → 下一 fn 是测试函数
        if re.search(r'#\[(tokio::)?test', line):
            pending_test = True
            continue
        if re.search(r'^(pub\s+)?(async\s+)?fn\s|^fn\s', line):
            is_test = pending_test
            pending_test = False
            fn_stack.append([is_test, line.count('{') - line.count('}')])
            continue
        # 函数体深度维护
        if fn_stack:
            delta = line.count('{') - line.count('}')
            fn_stack[-1][1] += delta
            if fn_stack[-1][1] <= 0:
                fn_stack.pop()
                continue
            if fn_stack[-1][0]:  # 测试函数体内
                continue
        # 非测试上下文
        if '.unwrap()' in line:
            prod.append((i+1, raw.strip()[:100]))
    return prod

total = 0
per = {}
samples = []
for root, dirs, files in os.walk('packages'):
    if 'target' in root or r'\tests' in root or r'\benches' in root or r'\examples' in root: continue
    for f in files:
        if not f.endswith('.rs'): continue
        p = os.path.join(root, f)
        hits = scan(p)
        if hits:
            crate = p.split(os.sep)[1]
            per[crate] = per.get(crate, 0) + len(hits)
            total += len(hits)
            if len(samples) < 14:
                samples.extend(p.split(os.sep)[-2] + "/" + os.path.basename(p) + ":" + str(ln) + " " + txt for ln, txt in hits[:2])
print("AUTHORITATIVE_PROD_UNWRAP:", total)
for crate in sorted(per, key=lambda c: -per[c]):
    print("  %s: %d" % (crate, per[crate]))
print("--- 抽样 ---")
for s in samples[:12]: print(" ", s)
