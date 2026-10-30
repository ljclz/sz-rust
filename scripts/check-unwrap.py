"""铁律 2 门禁：统计生产代码（非测试）中的裸 .unwrap()

2026-10-28 P1-11 修复扫描盲区：
- 旧实现把任意 `#[cfg(test)]` 行当作 `mod tests` 入口，`#[cfg(test)] use ...`
  这类单行属性会让后续整个文件被跳过（少报）。
- 旧实现只识别 `fn`/`pub fn`/`async fn` 声明，`pub(crate) fn`、`const fn`、
  `unsafe fn` 的块深度不跟踪，测试函数体内嵌套项会误算为生产代码（多报/漏报）。
- 旧实现用 `\\tests` 判断路径，在 Linux/CI 上永远不命中（测试目录被当成生产代码）。

新实现：按花括号深度维护「当前是否处于测试块」，属性行统一只做 pending 标记，
不再依赖函数声明正则。
"""
import os, re, sys, argparse


def parse_args():
    p = argparse.ArgumentParser(description='统计生产代码（非测试）中的 .unwrap() 数量')
    p.add_argument('--max', type=int, default=None,
                   help='门禁阈值：生产 unwrap 数超过该值则退出码为 1（缺省只统计不门禁）')
    p.add_argument('--root', default='packages', help='扫描根目录（默认 packages）')
    return p.parse_args()


def strip_source(src):
    """去除注释与字符串/字符字面量内容（保留换行以维持行号）

    必须正确处理生命周期（'static）与原始字符串（r#"..."#），否则其中的
    花括号会破坏深度计数——旧实现正是因此把测试块提前弹出而误报。
    """
    out = []
    i, n = 0, len(src)
    lifetime = re.compile(r"'[A-Za-z_][A-Za-z0-9_]*")
    raw_string = re.compile(r'(?:b?r)(#*)"')

    def keep_newlines(a, b):
        out.append('\n' * src.count('\n', a, b))

    while i < n:
        ch = src[i]
        nxt = src[i + 1] if i + 1 < n else ''

        if ch == '/' and nxt == '/':
            j = src.find('\n', i)
            i = n if j == -1 else j
            continue

        if ch == '/' and nxt == '*':
            depth, i = 1, i + 2
            start = i
            while i < n and depth:
                if src.startswith('/*', i):
                    depth += 1
                    i += 2
                elif src.startswith('*/', i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
            keep_newlines(start, i)
            continue

        m = raw_string.match(src, i)
        if m:
            body = m.end()
            term = '"' + m.group(1)
            j = src.find(term, body)
            end = n if j == -1 else j + len(term)
            keep_newlines(i, end)
            i = end
            continue

        if ch == 'b' and nxt == '"':
            start = i
            i = skip_quoted(src, i + 2, '"')
            keep_newlines(start, i)
            continue

        if ch == 'c' and nxt == '"':
            start = i
            i = skip_quoted(src, i + 2, '"')
            keep_newlines(start, i)
            continue

        if ch == '"':
            start = i
            i = skip_quoted(src, i + 1, '"')
            keep_newlines(start, i)
            continue

        if ch == "'":
            lm = lifetime.match(src, i)
            if lm and not (lm.end() < n and src[lm.end()] == "'"):
                out.append(lm.group(0))
                i = lm.end()
                continue
            start = i
            i = skip_quoted(src, i + 1, "'")
            keep_newlines(start, i)
            continue

        out.append(ch)
        i += 1
    return ''.join(out)


def skip_quoted(src, i, quote):
    n = len(src)
    while i < n:
        if src[i] == chr(92):
            i += 2
            continue
        if src[i] == quote:
            return i + 1
        i += 1
    return n


CFG_TEST = re.compile(r'^#\[cfg\(\s*(?:all\(\s*)?test')
TEST_ATTR = re.compile(r'^#\[(?:tokio::)?test\b')
TEST_MOD = re.compile(r'^(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+tests?\b')


def scan(path):
    with open(path, encoding='utf-8', errors='replace') as f:
        src = f.read()
    lines = strip_source(src).split('\n')
    hits = []
    depth = 0            # 当前花括号深度
    test_blocks = []     # 已打开的测试块深度栈
    pending_test = False  # 下一项（mod/fn/impl/...）属于测试

    for i, raw in enumerate(lines):
        s = raw.strip()
        if not s:
            continue  # 空行与纯注释（含 ///）不重置 pending_test

        if s.startswith('#['):
            if CFG_TEST.match(s) or TEST_ATTR.match(s):
                pending_test = True
            continue  # 其他属性不影响判定

        opens = s.count('{')
        closes = s.count('}')

        item_is_test = pending_test
        pending_test = False
        if TEST_MOD.match(s):
            item_is_test = True

        if item_is_test and opens > 0:
            # 该 item 的整个块视为测试块（深度目标 = 进入后的层数）
            test_blocks.append(depth + 1)

        if not test_blocks and '.unwrap()' in s:
            hits.append((i + 1, raw.strip()[:100]))
        depth += opens - closes
        while test_blocks and depth < test_blocks[-1]:
            test_blocks.pop()
    return hits


def iter_prod_sources(root):
    """遍历生产源码，跳过 tests/benches/examples 目录（跨平台）"""
    skip_dirs = {'target', 'tests', 'benches', 'examples'}
    for cur, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if d not in skip_dirs and not d.startswith('.')]
        for f in files:
            if f.endswith('.rs'):
                yield os.path.join(cur, f)


args = parse_args()

total = 0
per = {}
samples = []
for p in iter_prod_sources(args.root):
    hits = scan(p)
    if hits:
        rel = os.path.relpath(p, args.root).split(os.sep)
        crate = rel[0]
        per[crate] = per.get(crate, 0) + len(hits)
        total += len(hits)
        if len(samples) < 14:
            samples.extend('/'.join(rel[-2:]) + ':' + str(ln) + ' ' + txt for ln, txt in hits[:2])

print('AUTHORITATIVE_PROD_UNWRAP: %d' % total)
for crate in sorted(per, key=lambda c: -per[c]):
    print('  %s: %d' % (crate, per[crate]))
print('--- samples ---')
for s in samples[:12]:
    print(' ', s)

if args.max is not None:
    if total > args.max:
        print('FAIL: 生产 unwrap 数 %d 超过门禁阈值 %d' % (total, args.max))
        sys.exit(1)
    print('PASS: 生产 unwrap 数 %d <= 门禁阈值 %d' % (total, args.max))
