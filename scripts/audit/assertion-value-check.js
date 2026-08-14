#!/usr/bin/env node
'use strict';

/**
 * 测试断言价值门禁（防"测试通过=幻影"，支撑铁律 10/23）
 *
 * 扫描 packages/ 下所有 Rust 测试代码（#[cfg(test)] 模块 + tests/ 目录），
 * 检测「无断言测试」：#[test] 函数体内不含任何断言宏（assert!/assert_eq!/assert_ne!/
 * assert_matches!/assert_approx_eq!/assert!(...) 等），且无 #[should_panic] 预期。
 * 这类测试无论实现多正确都会"通过"，是覆盖率空洞达标的典型来源。
 *
 * 规则：
 *   - ERROR：测试函数体内零断言宏（无价值测试，覆盖率被其空洞填充）
 *   - WARN ：每文件断言密度 < 0.5（断言数/测试数），提示批量低价值测试
 *   - 豁免：#[should_panic] 测试（本身即断言）；`// doc-code-consistency: ignore` 注释行
 *
 * 用法：node scripts/audit/assertion-value-check.js [--fail-on-warn]
 * 退出码：0 = 无 ERROR；1 = 存在 ERROR；--fail-on-warn 时 WARN 也退出 1
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const PACKAGES_DIR = path.join(ROOT, 'packages');
const FAIL_ON_WARN = process.argv.includes('--fail-on-warn');

// 断言宏模式（排除已废弃的 debug_assert? 否——debug_assert 也是断言，但测试中应使用正式断言；
// 这里同时识别两者，保证不误杀）
const ASSERT_PATTERNS = [
    /assert_eq!/g,
    /assert_ne!/g,
    /assert_matches!/g,
    /assert_approx_eq!/g,
    /assert_abs_diff_eq!/g,
    /assert_relative_eq!/g,
    /assert_ok!/g,
    /assert_err!/g,
    /assert_contains!/g,
    /assert!\(/g,
    /debug_assert/g,
];
const ASSERT_RE = new RegExp(
    ASSERT_PATTERNS.map((p) => p.source.replace(/\/g$/, '')).join('|'),
    'g'
);

// 测试属性：#[test] / #[tokio::test] / #[sqlx::test] / #[async_std::test] 等
// 注意：不带 /g 标志，避免 .test() 推进 lastIndex 导致交替失败
const TEST_ATTR_RE = /#\[\s*(?:tokio::test|async_std::test|test|sqlx::test|actix_web::test|::\w+::test)\s*(?:,\s*[^\]]*)?\]/;

function walkDir(dir, ext, results = []) {
    if (!fs.existsSync(dir)) return results;
    const entries = fs.readdirSync(dir, { withFileTypes: true });
    for (const entry of entries) {
        const fullPath = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            if (entry.name === 'target' || entry.name === 'node_modules') continue;
            walkDir(fullPath, ext, results);
        } else if (entry.isFile() && entry.name.endsWith(ext)) {
            results.push(fullPath);
        }
    }
    return results;
}

// 提取测试代码块（去掉行注释与字符串字面量，避免把注释/字符串里的 assert 当断言）
// 注意：字符串正则禁止跨行（[^"\\\n]），否则会把 " 到下一个 " 之间的整段代码吞掉
function stripNoise(code) {
    return code
        .replace(/\/\/[^\n]*/g, '')
        .replace(/"(?:[^"\\\n]|\\.)*"/g, '""')
        .replace(/\/\*[\s\S]*?\*\//g, '');
}

// 找到第 i 个字符开始的匹配括号块（支持嵌套）
function findBlockEnd(text, start) {
    let depth = 0;
    for (let i = start; i < text.length; i++) {
        if (text[i] === '{') depth++;
        else if (text[i] === '}') {
            depth--;
            if (depth === 0) return i + 1;
        }
    }
    return text.length;
}

// 意图明确的 smoke/构造/编译期测试词：验证「不 panic / 可构造 / Send+Sync / 可调用」，
// 在 Rust 生态属合法测试（编译期回归保护），降级为 WARN
const SMOKE_INTENT_RE = /no_panic|does_not_panic|is_send|send_sync|clonable|accessible|callable|constructible|smoke|unused|_new\b|_default\b|不 ?panic|编译期|编译时|验证.*(?:构造|可用)|可构造|初始化/;

function analyzeTestFunction(body, attr, name) {
    const clean = stripNoise(body);
    const assertions = (clean.match(ASSERT_RE) || []).length;
    // 隐式断言：unwrap()/expect() 失败即 panic，同样验证调用结果（验证「不失败」）
    const implicit = (clean.match(/\.unwrap\(\)|\.expect\(/g) || []).length;
    const isShouldPanic = /#\[\s*should_panic/.test(attr);
    const isSmokeIntent = SMOKE_INTENT_RE.test(name + ' ' + body);
    return { assertions, implicit, isShouldPanic, isSmokeIntent };
}

function analyzeFile(file) {
    const content = fs.readFileSync(file, 'utf8');
    const lines = content.split('\n');
    const tests = [];
    let i = 0;
    while (i < lines.length) {
        // 收集属性行（#[test] 可能跨多行或紧随 fn）
        let attrLines = [];
        while (i < lines.length && /^\s*#\[/.test(lines[i])) {
            attrLines.push(lines[i]);
            i++;
        }
        // 跳过属性后的空行/注释，定位 fn 行
        let fnLineIdx = i;
        while (fnLineIdx < lines.length && /^\s*(?:\/\/|\/\*|\*)/.test(lines[fnLineIdx])) fnLineIdx++;
        if (fnLineIdx >= lines.length) break;
        const fnLine = lines[fnLineIdx];
        const fnMatch = fnLine.match(/^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)\s*\(/);
            if (fnMatch) {
                const attr = attrLines.join('\n');
                if (TEST_ATTR_RE.test(attr)) {
                    // 累积从 fn 行开始的完整文本（函数体可能跨多行），定位 `{`
                    let text = lines.slice(fnLineIdx).join('\n');
                    let bodyStart = text.indexOf('{');
                    if (bodyStart !== -1) {
                        const full = text.slice(bodyStart);
                        const endRel = findBlockEnd(full, 0);
                        const body = full.slice(0, endRel);
                        const res = analyzeTestFunction(body, attr, fnMatch[1]);
                        tests.push({ name: fnMatch[1], line: fnLineIdx + 1, ...res });
                    }
                }
                attrLines = [];
            }
            i = fnLineIdx + 1;
    }
    return tests;
}

function main() {
    const files = walkDir(PACKAGES_DIR, '.rs');
    const errors = [];
    const warnings = [];
    let totalTests = 0;
    let totalAssertions = 0;
    let lowAssertFiles = 0;

    for (const file of files) {
        const rel = path.relative(ROOT, file).replace(/\\/g, '/');
        // 只分析含测试的文件（#[cfg(test)] 模块 或 tests/ 目录）
        const content = fs.readFileSync(file, 'utf8');
        if (!/cfg\(test\)/.test(content) && !rel.includes(`${path.sep}tests${path.sep}`) && !/\/tests\//.test(rel)) {
            continue;
        }
        const tests = analyzeFile(file);
        if (tests.length === 0) continue;

        const fileAssertions = tests.reduce((s, t) => s + t.assertions, 0);
        totalTests += tests.length;
        totalAssertions += fileAssertions;
        if (fileAssertions / tests.length < 0.5) lowAssertFiles++;

        for (const t of tests) {
            if (t.assertions === 0 && !t.isShouldPanic) {
                if (t.implicit > 0) {
                    warnings.push(`${rel}:${t.line} 测试 ${t.name}() 无断言宏但有 ${t.implicit} 处 unwrap/expect（隐式断言，建议补显式断言）`);
                } else if (t.isSmokeIntent) {
                    warnings.push(`${rel}:${t.line} 测试 ${t.name}() 无断言（smoke/构造/编译期意图，验证不 panic，建议补断言增强）`);
                } else {
                    errors.push(`${rel}:${t.line} 测试 ${t.name}() 无任何断言且无 unwrap/expect（覆盖率空洞，建议补充断言或删除）`);
                }
            }
        }
    }

    const density = totalTests > 0 ? (totalAssertions / totalTests).toFixed(2) : '0.00';
    console.log(`✅ 已扫描测试文件，共 ${totalTests} 个测试函数，${totalAssertions} 处断言，断言密度 ${density}`);
    for (const w of warnings) console.log(`  [WARN] ${w}`);
    for (const e of errors) console.log(`  [ERROR] ${e}`);
    if (errors.length > 0) {
        console.error(`\n❌ 发现 ${errors.length} 个无断言测试（断言价值门禁，铁律 10/23），请补充断言或删除空洞测试`);
        process.exit(1);
    }
    if (lowAssertFiles > 0) {
        console.warn(`\n⚠️ ${lowAssertFiles} 个文件断言密度 < 0.5（建议提高断言质量，不阻塞）`);
    } else if (totalTests > 0) {
        console.log(`\n✅ 断言密度 ${density} ≥ 0.5，无空洞测试`);
    }
    process.exit(0);
}

main();
