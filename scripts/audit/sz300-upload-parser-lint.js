#!/usr/bin/env node
'use strict';

/**
 * sz300 上传解析库防护 lint（AppSec P2-2 闭环，2026-11-06）
 *
 * 背景：
 *   AppSec 复评 P2-2 原指控"image/lopdf/calamine 等解析库处理用户上传，内部含 unsafe"。
 *   T5 调研确认 sz300 上传端点（controllers/file.rs:17/47）仅做文件保存
 *   （file_service::FileService::save，file_service.rs:26），不解析文件内容；
 *   image/lopdf/calamine 未被 sz300 业务代码调用（grep 无匹配），实际攻击面不存在。
 *   P2-2 风险是潜在的——未来若有人给 sz300 接入 PDF/Excel/图像解析端点而无资源限制
 *   （超时/内存上限/沙箱化），会重新引入 unsafe 解析不可信输入的攻击面。
 *
 * 本 lint 作为"防未来引入"门禁：
 *   扫描 packages/sz-rust-sz300/Cargo.toml 的 [dependencies] / [build-dependencies]，
 *   若出现下列解析库（含 facade crate sz-rust-pdf）则 exit 1，提示必须先登记资源限制
 *   并更新 AppSec 评测。dev-dependencies 不扫（测试用解析库可接受）。
 *
 * 用法：node scripts/audit/sz300-upload-parser-lint.js
 * 退出码：0 = sz300 未引入上传解析库；1 = 引入了，需登记资源限制
 *
 * 接线：.github/workflows/ci.yml audit job（门禁 6 Security Audit）
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const SZ300_MANIFEST = path.join(ROOT, 'packages', 'sz-rust-sz300', 'Cargo.toml');

// 禁用解析库清单（含 facade crate）。
// - image / lopdf / calamine：复评报告点名的图像/PDF/Excel 解析库，内部含 unsafe
// - pdf / xlsx / csv：通用解析库键名（防止未来以简名引入）
// - sz-rust-pdf：workspace 内 PDF/Excel 解析 facade（内部用 lopdf/calamine，见 sz-rust-pdf/src/pdf_form.rs:44 / excel_import.rs:23）
const FORBIDDEN_PARSER_LIBS = [
    'image',
    'lopdf',
    'calamine',
    'pdf',
    'xlsx',
    'csv',
    'sz-rust-pdf',
];

/**
 * 解析 Cargo.toml 指定 section 段内的依赖键名集合。
 * 仅匹配顶层 `key = ...` 形式（含 `key.workspace = true` / `key = { ... }` / `key = "version"`）。
 * 内嵌表行（如 `features = ["mysql"]`）被忽略——它们不是依赖键。
 *
 * @param {string} manifestPath - Cargo.toml 绝对路径
 * @param {string[]} sections  - 要收集的 section 头（如 ['[dependencies]', '[build-dependencies]']）
 * @returns {Map<string, number>} depName -> 行号（1-based）
 */
function parseDependencyKeys(manifestPath, sections) {
    const raw = fs.readFileSync(manifestPath, 'utf8').replace(/\r/g, ''); // 剥离 CR（JS 正则 . 不匹配 \r）
    const lines = raw.split('\n');
    const want = new Set(sections);
    const deps = new Map();
    let currentSection = null;
    for (let i = 0; i < lines.length; i++) {
        const line = lines[i];
        const sectionMatch = line.match(/^\[([^\]]+)\]/);
        if (sectionMatch) {
            currentSection = '[' + sectionMatch[1] + ']';
            continue;
        }
        if (currentSection === null || !want.has(currentSection)) continue;
        // 依赖键行：`name = ...`（name 可含连字符）。跳过空行/注释。
        if (/^\s*#/.test(line) || /^\s*$/.test(line)) continue;
        const m = line.match(/^([A-Za-z][A-Za-z0-9_-]*)\s*=/);
        if (m) {
            if (!deps.has(m[1])) deps.set(m[1], i + 1);
        }
    }
    return deps;
}

function main() {
    if (!fs.existsSync(SZ300_MANIFEST)) {
        console.error(`[sz300-upload-parser-lint] ❌ 找不到 sz300 Cargo.toml: ${SZ300_MANIFEST}`);
        process.exit(2);
    }

    const deps = parseDependencyKeys(SZ300_MANIFEST, ['[dependencies]', '[build-dependencies]']);

    const violations = [];
    for (const lib of FORBIDDEN_PARSER_LIBS) {
        if (deps.has(lib)) {
            violations.push({ lib, line: deps.get(lib) });
        }
    }

    if (violations.length > 0) {
        console.error('[sz300-upload-parser-lint] ❌ sz300 引入上传解析库，必须先登记资源限制（超时/内存上限/沙箱化）并更新 AppSec 评测:');
        for (const v of violations) {
            console.error(`  - ${v.lib}  (packages/sz-rust-sz300/Cargo.toml:${v.line})`);
        }
        console.error('');
        console.error('  闭环要求（见 docs/audit/2026-11-05-AppSec开放项修复方案调研.md P2-2 方案 B/C）:');
        console.error('    1. 解析调用必须用 tokio::time::timeout 包裹 + spawn_blocking 隔离（方案 B）；或');
        console.error('    2. 解析逻辑移入 WASM 沙箱（fuel + 内存上限 + 超时，复用 controllers/wasm_api.rs，方案 C）；');
        console.error('    3. 在 docs/audit/ 复评报告 P2-2 重新打开缺陷项，登记资源限制参数与测试证据。');
        process.exit(1);
    }

    console.log('[sz300-upload-parser-lint] ✅ sz300 未引入 image/lopdf/calamine/pdf/xlsx/csv/sz-rust-pdf 等上传解析库（P2-2 闭环维持）');
    console.log(`[sz300-upload-parser-lint]    扫描 [dependencies]+[build-dependencies] 共 ${deps.size} 个依赖键，0 命中禁用清单`);
    process.exit(0);
}

main();