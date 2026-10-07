#!/usr/bin/env node
/**
 * coverage-exemption.js — v1.9.0 覆盖率豁免审计脚本
 *
 * 对应 P0 债务 DB-2026-10-05-01 ⑦：扫描 workspace 内 `allow(dead_code)` 豁免项，
 * 分类输出到期清单，验证 test helper 模块的豁免符合 AGENTS.md 约束。
 *
 * 用法：node scripts/audit/coverage-exemption.js
 * 退出码：0 = 无到期未处理项；1 = 存在到期未处理项
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');
const PACKAGES_DIR = path.join(ROOT, 'packages');

// AGENTS.md 约束：测试 common 模块内 #![allow(dead_code)] 可接受
const TEST_HELPER_PATTERNS = [
    /tests[\\/]+common[\\/]+/,
    /tests[\\/]+common[\\/]+mod\.rs$/,
    /tests[\\/]+common[\\/]+assertions\.rs$/,
    /tests[\\/]+common[\\/]+fixtures\.rs$/,
];

// crate 级 #![allow(dead_code)] 禁止（AGENTS.md 铁律）
// 字段级 #[allow(dead_code)] 需附理由注释
function classifyFile(filePath) {
    const rel = path.relative(ROOT, filePath).replace(/\\/g, '/');

    // 测试 helper 模块 — 可接受
    if (TEST_HELPER_PATTERNS.some(p => p.test(rel))) {
        return { category: 'test_helper', acceptable: true, rel };
    }

    // benches — 基准测试辅助，可接受
    if (rel.includes('/benches/')) {
        return { category: 'bench_helper', acceptable: true, rel };
    }

    // src 内的字段级豁免 — 需人工审核
    if (rel.includes('/src/')) {
        return { category: 'src_field', acceptable: false, rel };
    }

    // tests 内其他豁免 — 需人工审核
    if (rel.includes('/tests/')) {
        return { category: 'test_other', acceptable: false, rel };
    }

    return { category: 'unknown', acceptable: false, rel };
}

function scanFile(filePath) {
    const content = fs.readFileSync(filePath, 'utf8');
    const lines = content.split('\n');
    const hits = [];

    lines.forEach((line, idx) => {
        const trimmed = line.trim();
        // crate 级 #![allow(dead_code)]
        if (trimmed.includes('#![allow(dead_code)]')) {
            hits.push({
                line: idx + 1,
                kind: 'crate_level',
                content: trimmed,
            });
        }
        // 字段级 #[allow(dead_code)]
        else if (trimmed.includes('#[allow(dead_code)]')) {
            const hasComment = trimmed.includes('//') || (lines[idx + 1] && lines[idx + 1].trim().startsWith('//'));
            hits.push({
                line: idx + 1,
                kind: 'field_level',
                hasReasonComment: hasComment,
                content: trimmed,
            });
        }
    });

    return hits;
}

function walkDir(dir, ext, acc) {
    if (!fs.existsSync(dir)) return acc;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            if (entry.name === 'target' || entry.name === 'node_modules') continue;
            walkDir(full, ext, acc);
        } else if (entry.name.endsWith(ext)) {
            acc.push(full);
        }
    }
    return acc;
}

// 主逻辑
const rustFiles = walkDir(PACKAGES_DIR, '.rs', []);
const results = [];
let unresolvedCount = 0;

for (const file of rustFiles) {
    const hits = scanFile(file);
    if (hits.length === 0) continue;

    const classification = classifyFile(file);

    for (const hit of hits) {
        const entry = {
            file: classification.rel,
            line: hit.line,
            kind: hit.kind,
            category: classification.category,
            acceptable: classification.acceptable,
            hasReasonComment: hit.hasReasonComment || false,
        };

        // 判定是否需要处理
        // 1. crate 级豁免在非 test_helper 模块 — 违规（AGENTS.md 铁律）
        // 2. 字段级豁免无理由注释 — 标记 INFO（建议补注释，不阻塞）
        if (hit.kind === 'crate_level' && !classification.acceptable) {
            entry.status = 'VIOLATION';
            unresolvedCount++;
        } else if (hit.kind === 'field_level' && !classification.acceptable && !hit.hasReasonComment) {
            entry.status = 'INFO';
            // 字段级豁免不阻塞，仅记录建议
        } else {
            entry.status = 'OK';
        }

        results.push(entry);
    }
}

// 输出报告
console.log('═'.repeat(78));
console.log('覆盖率豁免审计报告（v1.9.0 — DB-2026-10-05-01 ⑦）');
console.log('═'.repeat(78));
console.log(`扫描文件数：${rustFiles.length}`);
console.log(`豁免命中数：${results.length}`);
console.log(`通过：${results.length - unresolvedCount}　待处理：${unresolvedCount}`);
console.log('─'.repeat(78));

// 按类别分组
const byCategory = {};
for (const r of results) {
    if (!byCategory[r.category]) byCategory[r.category] = [];
    byCategory[r.category].push(r);
}

for (const [cat, items] of Object.entries(byCategory)) {
    console.log(`\n【${cat}】(${items.length} 项)`);
    for (const item of items) {
        const flag = item.status === 'OK' ? '✓' : item.status === 'VIOLATION' ? '✗' : '⚠';
        console.log(`  ${flag} ${item.file}:${item.line} [${item.kind}] ${item.status}`);
    }
}

console.log('\n' + '═'.repeat(78));
if (unresolvedCount === 0) {
    console.log('✅ 审计通过：0 到期未处理项');
    console.log('　test helper 模块豁免符合 AGENTS.md 约束（测试 common 模块内 #![allow(dead_code)] 可接受）');
    process.exit(0);
} else {
    console.log(`❌ 审计失败：${unresolvedCount} 到期未处理项`);
    process.exit(1);
}