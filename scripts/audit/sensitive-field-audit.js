#!/usr/bin/env node
'use strict';

/**
 * 敏感字段脱敏审计脚本
 *
 * 扫描 packages/ 下所有 Rust 源文件，检测包含敏感字段（password, secret, token, api_key 等）
 * 的结构体是否已实现脱敏保护（自定义 Debug 输出 [REDACTED] 或 #[serde(skip_serializing)]）。
 *
 * 用法：node scripts/audit/sensitive-field-audit.js
 * 退出码：0 = 无 EXPOSED 项，1 = 存在 EXPOSED 项
 */

const fs = require('fs');
const path = require('path');

const PACKAGES_DIR = path.join(__dirname, '..', '..', 'packages');

const SENSITIVE_FIELD_PATTERNS = [
    /\bpassword\b/i,
    /\bsecret\b/i,
    /\bapi_key\b/i,
    /\bapikey\b/i,
    /\baccess_token\b/i,
    /\brefresh_token\b/i,
    /\bprivate_key\b/i,
    /\bpasswd\b/i,
    /\bcred(?:ential)?s?\b/i,
];

const REDACTION_MARKERS = [
    '[REDACTED]',
    '***',
    '<redacted>',
    'skip_serializing',
];

function walkDir(dir, ext, results = []) {
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

function findStructRange(lines, structLineIdx) {
    let braceDepth = 0;
    let foundOpen = false;
    let endIdx = structLineIdx;
    for (let i = structLineIdx; i < lines.length; i++) {
        const line = lines[i];
        for (const ch of line) {
            if (ch === '{') { braceDepth++; foundOpen = true; }
            else if (ch === '}') { braceDepth--; }
        }
        if (foundOpen && braceDepth === 0) {
            endIdx = i;
            break;
        }
    }
    return endIdx;
}

function hasCustomDebugInRange(lines, structName, startSearchIdx, endSearchIdx) {
    const debugImplPattern = new RegExp(`impl\\s+std::fmt::Debug\\s+for\\s+${structName}\\b`);
    const debugImplPattern2 = new RegExp(`impl\\s+fmt::Debug\\s+for\\s+${structName}\\b`);
    for (let i = startSearchIdx; i < Math.min(endSearchIdx + 200, lines.length); i++) {
        if (debugImplPattern.test(lines[i]) || debugImplPattern2.test(lines[i])) {
            for (let j = i; j < Math.min(i + 50, lines.length); j++) {
                if (REDACTION_MARKERS.some(m => lines[j].includes(m))) {
                    return true;
                }
            }
        }
    }
    return false;
}

function hasSkipSerializingInStruct(lines, structStartIdx, structEndIdx) {
    for (let i = structStartIdx; i <= structEndIdx; i++) {
        if (lines[i].includes('skip_serializing')) {
            return true;
        }
    }
    return false;
}

function hasDeriveDebug(lines, structStartIdx) {
    for (let i = Math.max(0, structStartIdx - 5); i <= structStartIdx; i++) {
        if (lines[i].includes('#[derive(') && lines[i].includes('Debug')) {
            return true;
        }
    }
    return false;
}

function auditFile(filePath) {
    const content = fs.readFileSync(filePath, 'utf-8');
    const lines = content.split('\n');
    const findings = [];

    for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        const structMatch = line.match(/^\s*pub\s+struct\s+(\w+)\s*\{/);
        if (!structMatch) continue;

        const structName = structMatch[1];
        const structEndIdx = findStructRange(lines, i);

        const hasDeriveDbg = hasDeriveDebug(lines, i);
        const hasSkipSer = hasSkipSerializingInStruct(lines, i, structEndIdx);
        const hasCustomDbg = hasCustomDebugInRange(lines, structName, structEndIdx + 1, lines.length);

        for (let j = i + 1; j <= structEndIdx; j++) {
            const fieldLine = lines[j];
            const fieldMatch = fieldLine.match(/^\s*pub\s+(\w+)\s*:/);
            if (!fieldMatch) continue;
            const fieldName = fieldMatch[1];

            if (!SENSITIVE_FIELD_PATTERNS.some(p => p.test(fieldName))) continue;

            let status;
            if (hasSkipSer) {
                status = 'SAFE_SKIP_SER';
            } else if (hasCustomDbg) {
                status = 'SAFE_CUSTOM_DEBUG';
            } else if (hasDeriveDbg) {
                status = 'EXPOSED';
            } else {
                status = 'NO_DEBUG_DERIVE';
            }

            findings.push({
                file: path.relative(path.join(__dirname, '..', '..'), filePath),
                line: j + 1,
                struct: structName,
                field: fieldName,
                status,
            });
        }
    }
    return findings;
}

function main() {
    const rustFiles = walkDir(PACKAGES_DIR, '.rs');
    const allFindings = [];

    for (const file of rustFiles) {
        allFindings.push(...auditFile(file));
    }

    const exposed = allFindings.filter(f => f.status === 'EXPOSED');
    const safe = allFindings.filter(f => f.status.startsWith('SAFE_'));
    const noDebug = allFindings.filter(f => f.status === 'NO_DEBUG_DERIVE');

    console.log('=== 敏感字段脱敏审计报告 ===');
    console.log(`扫描文件数：${rustFiles.length}`);
    console.log(`敏感字段总数：${allFindings.length}`);
    console.log(`  SAFE_CUSTOM_DEBUG（自定义 Debug 脱敏）：${safe.filter(f => f.status === 'SAFE_CUSTOM_DEBUG').length}`);
    console.log(`  SAFE_SKIP_SER（skip_serializing 脱敏）：${safe.filter(f => f.status === 'SAFE_SKIP_SER').length}`);
    console.log(`  NO_DEBUG_DERIVE（无 Debug，低风险）：${noDebug.length}`);
    console.log(`  EXPOSED（存在泄露风险）：${exposed.length}`);
    console.log('');

    if (exposed.length > 0) {
        console.log('=== EXPOSED 项（需要修复）===');
        for (const f of exposed) {
            console.log(`  ${f.file}:${f.line}  ${f.struct}.${f.field}`);
        }
        console.log('');
    }

    if (noDebug.length > 0) {
        console.log('=== NO_DEBUG_DERIVE 项（建议审查）===');
        for (const f of noDebug) {
            console.log(`  ${f.file}:${f.line}  ${f.struct}.${f.field}`);
        }
        console.log('');
    }

    if (exposed.length === 0) {
        console.log('✅ 审计通过：0 个 EXPOSED 项');
        process.exit(0);
    } else {
        console.log(`❌ 审计失败：${exposed.length} 个 EXPOSED 项`);
        process.exit(1);
    }
}

main();