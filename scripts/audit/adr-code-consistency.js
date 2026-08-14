#!/usr/bin/env node
'use strict';

/**
 * ADR-代码对账审计脚本（ADR 引用代码存在性检查，支撑铁律 14）
 *
 * 解析 docs/adr/ 下所有 ADR 的「相关代码」段（及正文中的 packages/ 路径引用），
 * 验证引用的文件/目录是否真实存在于仓库：
 *   - ERROR：ADR 引用的代码路径不存在（ADR 与代码漂移）
 *   - WARN ：路径存在于 packages/ 但非 workspace member（crate 级引用漂移）
 *
 * 用法：node scripts/audit/adr-code-consistency.js
 * 退出码：0 = 无 ERROR，1 = 存在 ERROR
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const ADR_DIR = path.join(ROOT, 'docs', 'adr');

function walkMd(dir, results = []) {
    if (!fs.existsSync(dir)) return results;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
            walkMd(full, results);
        } else if (entry.isFile() && /^ADR-\d+.*\.md$/.test(entry.name)) {
            results.push(full);
        }
    }
    return results;
}

function workspaceMembers() {
    const manifest = fs.readFileSync(path.join(ROOT, 'Cargo.toml'), 'utf8');
    const members = new Set();
    for (const line of manifest.split('\n')) {
        const m = line.match(/"packages\/(sz-rust-[a-z0-9-]+)"/);
        if (m) members.add(m[1]);
    }
    return members;
}

// 已知企业版交付声明（2026-08-13 审计报告核验：代码在 sz-rust-enterprise 仓库，本仓库无源码）。
// ADR 引用这些 crate 属跨仓库错位，报 WARN 需人工核验；清单外的引用不存在一律 ERROR。
const KNOWN_EXTERNAL_CRATES = [
    'sz-rust-marketplace',
    'sz-rust-visual',
    'sz-rust-sdd-agent',
    'sz-rust-migration',
];

function isExternalRef(ref) {
    return KNOWN_EXTERNAL_CRATES.some((c) => ref.includes(`packages/${c}/`) || ref === `packages/${c}`);
}

// 提取 ADR 中引用的 packages/ 路径（相关代码段 + 正文）
function extractCodeRefs(content) {
    const refs = new Map(); // ref -> [{ line, context }]
    content.split('\n').forEach((line, idx) => {
        const matches = line.matchAll(/packages\/[A-Za-z0-9_\-./]+(?:\.rs)?/g);
        for (const m of matches) {
            // 去掉行尾标点
            let ref = m[0].replace(/[),;:」」]+$/, '');
            if (!refs.has(ref)) refs.set(ref, []);
            refs.get(ref).push({ line: idx + 1 });
        }
    });
    return refs;
}

function main() {
    const adrs = walkMd(ADR_DIR);
    const members = workspaceMembers();
    const errors = [];
    const warnings = [];
    let refCount = 0;

    for (const adr of adrs) {
        const content = fs.readFileSync(adr, 'utf8');
        const refs = extractCodeRefs(content);
        const rel = path.relative(ROOT, adr).replace(/\\/g, '/');
        for (const [ref, occurrences] of refs) {
            refCount++;
            const first = occurrences[0];
            // 归一化：去尾部标点/引号，支持 file:line 后缀
            const cleanRef = ref.replace(/[:：]\d+(-\d+)?$/, '');
            // 已知企业版交付（跨仓库错位）→ WARN
            if (isExternalRef(cleanRef)) {
                warnings.push(`${rel}:${first.line} 引用企业版交付 crate（需核验企业版仓库）: ${cleanRef}`);
                continue;
            }
            if (cleanRef.endsWith('.rs') || cleanRef.endsWith('/')) {
                if (!fs.existsSync(path.join(ROOT, cleanRef))) {
                    errors.push(`${rel}:${first.line} 引用代码不存在: ${cleanRef}`);
                }
            } else {
                // crate 目录级引用：存在性 + workspace member 校验
                if (!fs.existsSync(path.join(ROOT, cleanRef))) {
                    errors.push(`${rel}:${first.line} 引用目录不存在: ${cleanRef}`);
                } else {
                    const crateName = cleanRef.split('/').pop();
                    if (crateName.startsWith('sz-rust-') && !members.has(crateName)) {
                        warnings.push(`${rel}:${first.line} 引用 crate 非 workspace member: ${cleanRef}`);
                    }
                }
            }
        }
    }

    console.log(`✅ 已扫描 ${adrs.length} 个 ADR，${refCount} 处 packages/ 代码引用`);
    for (const w of warnings) console.log(`  [WARN] ${w}`);
    for (const e of errors) console.log(`  [ERROR] ${e}`);

    if (errors.length > 0) {
        console.error(`\n❌ 发现 ${errors.length} 处 ADR 引用代码不存在（ADR 漂移，铁律 14），请修正 ADR 或补齐代码`);
        process.exit(1);
    }
    if (warnings.length > 0) {
        console.warn(`\n⚠️ 发现 ${warnings.length} 处非 member 引用（不阻塞）`);
    } else {
        console.log('\n✅ ADR 引用的代码均存在，无漂移');
    }
    process.exit(0);
}

main();
