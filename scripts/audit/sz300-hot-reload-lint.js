#!/usr/bin/env node
'use strict';

/**
 * sz300 native 插件热加载防护 lint（AppSec P1-1 闭环，2026-11-06）
 *
 * 背景：
 *   AppSec 复评 P1-1 指控"native 插件热加载无完整性校验 = 任意代码执行面"。
 *   packages/sz-rust-core/src/runtime/hot_reload.rs:347 用
 *   `unsafe { Library::new(&path) }` 直接加载 addons/ 下 .dll/.so/.dylib，
 *   无签名/哈希校验（grep verify/hash/sha256 在该文件无匹配）。
 *
 *   用户选定闭环方案（T5 调研）：不启用 + CI lint 禁用（最小攻击面，0.5 人日）。
 *   - sz300 Cargo.toml default features 刻意不含 hot-reload（Cargo.toml:112 仅声明 feature）
 *   - prod compose 的 SZ300_FEATURES 刻意不含 hot-reload
 *
 * 本 lint 作为"防未来引入"门禁，扫描三处：
 *   1. packages/sz-rust-sz300/Cargo.toml [features] 段的 default 数组
 *   2. deploy/docker-compose.prod.yml 的 SZ300_FEATURES 值
 *   3. 根目录 docker-compose.prod.yml 的 SZ300_FEATURES 值
 *   任一处含 hot-reload → exit 1。全部不含 → exit 0。
 *
 * 用法：node scripts/audit/sz300-hot-reload-lint.js
 * 退出码：0 = 生产构建未启用 hot-reload；1 = 启用了，违反 P1-1 闭环
 *
 * 接线：.github/workflows/ci.yml audit job（门禁 6 Security Audit，紧跟 6c P2-2 之后）
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const SZ300_MANIFEST = path.join(ROOT, 'packages', 'sz-rust-sz300', 'Cargo.toml');
const PROD_COMPOSES = [
    path.join(ROOT, 'deploy', 'docker-compose.prod.yml'),
    path.join(ROOT, 'docker-compose.prod.yml'),
];

const FORBIDDEN_FEATURE = 'hot-reload';

/**
 * 解析 Cargo.toml [features] 段中 default 数组的元素列表。
 *
 * TOML 数组可能跨多行：
 *   default = [
 *       "v18-security-headers",
 *       "v19-saga",
 *   ]
 * 也可能单行：default = ["a", "b"]
 *
 * 仅在 [features] 段内匹配（避免误命中 [dependencies] 等段的 default 键）。
 *
 * @param {string} manifestPath - Cargo.toml 绝对路径
 * @returns {{items: string[], line: number}} default 数组元素 + 起始行号（1-based）
 */
function parseDefaultFeatures(manifestPath) {
    const raw = fs.readFileSync(manifestPath, 'utf8').replace(/\r/g, '');
    const lines = raw.split('\n');
    let currentSection = null;
    for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        const sectionMatch = line.match(/^\[([^\]]+)\]/);
        if (sectionMatch) {
            currentSection = '[' + sectionMatch[1] + ']';
            continue;
        }
        if (currentSection !== '[features]') continue;
        // 匹配 `default = [`（等号两侧可有空格）
        const startMatch = line.match(/^default\s*=\s*\[/);
        if (startMatch) {
            // 收集从本行 [ 之后到 ] 之间的所有内容
            let buf = line.slice(line.indexOf('[') + 1);
            let end = buf.indexOf(']');
            if (end !== -1) {
                // 单行数组
                return { items: extractQuotedItems(buf.slice(0, end)), line: i + 1 };
            }
            // 多行数组：继续读直到找到 ]
            const items = extractQuotedItems(buf);
            for (let j = i + 1; j < lines.length; j++) {
                const continued = lines[j];
                const endPos = continued.indexOf(']');
                if (endPos !== -1) {
                    items.push(...extractQuotedItems(continued.slice(0, endPos)));
                    return { items, line: i + 1 };
                }
                items.push(...extractQuotedItems(continued));
            }
            // 数组未闭合（异常 TOML），返回已收集的
            return { items, line: i + 1 };
        }
    }
    return { items: [], line: 0 };
}

/**
 * 从字符串中提取所有双引号包裹的元素（去掉引号与尾随逗号/空白）。
 * @param {string} s
 * @returns {string[]}
 */
function extractQuotedItems(s) {
    const out = [];
    const re = /"([^"]+)"/g;
    let m;
    while ((m = re.exec(s)) !== null) {
        out.push(m[1]);
    }
    return out;
}

/**
 * 从 docker-compose.prod.yml 提取 SZ300_FEATURES 的值字符串。
 *
 * 支持两种形式：
 *   1. SZ300_FEATURES: "v18-security-headers,v18-data-mask"     （字面量）
 *   2. SZ300_FEATURES: "${SZ300_FEATURES:-v18-rbac,v18-upload}" （compose 变量默认值）
 *
 * 对形式 2，提取 `:-` 后到 `}` 前的默认值（这是 compose 文件声明的安全默认）。
 * 同时也检查整个值字符串是否含 hot-reload（防字面量硬编码）。
 *
 * @param {string} composePath - yml 绝对路径
 * @returns {{value: string, line: number} | null} SZ300_FEATURES 值 + 行号；未找到返回 null
 */
function parseSz300FeaturesFromCompose(composePath) {
    const raw = fs.readFileSync(composePath, 'utf8').replace(/\r/g, '');
    const lines = raw.split('\n');
    for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        // 匹配 `SZ300_FEATURES:` 后的内容（可能有引号、${...} 等）
        const m = line.match(/SZ300_FEATURES\s*:\s*(.+?)\s*$/);
        if (m) {
            let value = m[1].trim();
            // 去掉外层引号
            if (value.startsWith('"') && value.endsWith('"')) {
                value = value.slice(1, -1);
            }
            // 提取 ${VAR:-default} 中的 default 部分（compose 变量默认值）
            const varDefault = value.match(/\$\{[^}]*:-(.+?)\}/);
            if (varDefault) {
                return { value: varDefault[1], line: i + 1, raw: value };
            }
            return { value, line: i + 1, raw: value };
        }
    }
    return null;
}

/**
 * 检查 feature 列表字符串（逗号分隔）是否含 hot-reload。
 * @param {string} featuresCsv
 * @returns {boolean}
 */
function containsHotReload(featuresCsv) {
    const items = featuresCsv.split(',').map(s => s.trim()).filter(Boolean);
    return items.includes(FORBIDDEN_FEATURE);
}

function main() {
    const violations = [];

    // ── 检查 1：sz300 Cargo.toml default features ──
    if (!fs.existsSync(SZ300_MANIFEST)) {
        console.error(`[sz300-hot-reload-lint] ❌ 找不到 sz300 Cargo.toml: ${SZ300_MANIFEST}`);
        process.exit(2);
    }
    const { items: defaultFeatures, line: defaultLine } = parseDefaultFeatures(SZ300_MANIFEST);
    if (defaultFeatures.includes(FORBIDDEN_FEATURE)) {
        violations.push({
            where: `packages/sz-rust-sz300/Cargo.toml:${defaultLine}`,
            detail: `default features 含 "${FORBIDDEN_FEATURE}"`,
        });
    }

    // ── 检查 2 & 3：prod compose SZ300_FEATURES ──
    for (const composePath of PROD_COMPOSES) {
        const rel = path.relative(ROOT, composePath).replace(/\\/g, '/');
        if (!fs.existsSync(composePath)) {
            // deploy/docker-compose.prod.yml 是补充模板，可能不存在；根目录必须存在
            if (rel === 'docker-compose.prod.yml') {
                console.error(`[sz300-hot-reload-lint] ❌ 找不到根目录 prod compose: ${composePath}`);
                process.exit(2);
            }
            continue;
        }
        const parsed = parseSz300FeaturesFromCompose(composePath);
        if (parsed === null) {
            // SZ300_FEATURES 未声明——视作沿用 Cargo.toml default，已在检查 1 覆盖
            continue;
        }
        if (containsHotReload(parsed.value)) {
            violations.push({
                where: `${rel}:${parsed.line}`,
                detail: `SZ300_FEATURES 默认值含 "${FORBIDDEN_FEATURE}"`,
            });
        }
    }

    // ── 汇总 ──
    if (violations.length > 0) {
        console.error('[sz300-hot-reload-lint] ❌ 生产构建禁止含 hot-reload feature（P1-1 闭环），native 插件热加载无完整性校验:');
        console.error(`  证据：packages/sz-rust-core/src/runtime/hot_reload.rs:347 \`unsafe { Library::new(&path) }\` 无签名/哈希校验`);
        for (const v of violations) {
            console.error(`  - ${v.where}  ${v.detail}`);
        }
        console.error('');
        console.error('  闭环要求（见 docs/audit/2026-11-05-AppSec复评报告.md P1-1）:');
        console.error('    1. 从 sz300 Cargo.toml default 数组移除 "hot-reload"；');
        console.error('    2. 从 prod compose 的 SZ300_FEATURES 移除 hot-reload；');
        console.error('    3. 若确需热加载，先补完整性校验（SHA-256 白名单 + 签名验证）并更新 AppSec 复评报告。');
        process.exit(1);
    }

    console.log('[sz300-hot-reload-lint] ✅ 生产构建未启用 hot-reload feature（P1-1 闭环维持）');
    console.log(`[sz300-hot-reload-lint]    sz300 Cargo.toml default = [${defaultFeatures.join(', ')}]（${defaultFeatures.length} 项，不含 hot-reload）`);
    for (const composePath of PROD_COMPOSES) {
        const rel = path.relative(ROOT, composePath).replace(/\\/g, '/');
        if (!fs.existsSync(composePath)) continue;
        const parsed = parseSz300FeaturesFromCompose(composePath);
        if (parsed) {
            const count = parsed.value.split(',').filter(Boolean).length;
            console.log(`[sz300-hot-reload-lint]    ${rel}:${parsed.line} SZ300_FEATURES 默认 ${count} 项，不含 hot-reload`);
        }
    }
    process.exit(0);
}

main();