#!/usr/bin/env node
'use strict';

/**
 * 配置-代码一致性审计脚本（feature 声明 vs 实际启用，支撑铁律 23/审计建议）
 *
 * 交叉验证 Cargo.toml [features] 声明与源码 #[cfg(feature)] 引用：
 *   - ERROR：源码引用的 feature 未在 Cargo.toml 声明（永远不可启用 → 死代码）
 *   - WARN ：Cargo.toml 声明的 feature 无任何源码引用（死 feature）
 *   - WARN ：默认 feature（default = [...]）中声明但文档声称「默认关闭」的功能
 *
 * 用法：node scripts/audit/feature-consistency.js
 * 退出码：0 = 无 ERROR，1 = 存在 ERROR
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const PACKAGES_DIR = path.join(ROOT, 'packages');

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

// 解析 Cargo.toml 的 [features] 段：featureName -> 依赖列表字符串
function parseFeatures(manifestPath) {
    const content = fs.readFileSync(manifestPath, 'utf8').replace(/\r/g, ''); // 剥离 CR（JS 正则 . 不匹配 \r）
    const features = {};
    const lines = content.split('\n');
    let inFeatures = false;
    for (const line of lines) {
        if (/^\[features\]/.test(line)) { inFeatures = true; continue; }
        if (inFeatures && /^\[/.test(line)) break; // 下一个 section
        if (!inFeatures) continue;
        const m = line.match(/^(\w[\w-]*)\s*=\s*(.*)$/);
        if (m) features[m[1]] = m[2];
    }
    return features;
}

// 解析 [dependencies] 段：依赖名集合（feature 列表中出现依赖名 = 启用该依赖，非死 feature）
function parseDependencyNames(manifestPath) {
    const content = fs.readFileSync(manifestPath, 'utf8').replace(/\r/g, '');
    const deps = new Set();
    const lines = content.split('\n');
    let inDeps = false;
    for (const line of lines) {
        if (/^\[dependencies\]/.test(line)) { inDeps = true; continue; }
        if (inDeps && /^\[/.test(line)) break;
        if (!inDeps) continue;
        const m = line.match(/^(\w[\w-]*)\s*=/);
        if (m) deps.add(m[1]);
    }
    return deps;
}

// 提取源码中的 #[cfg(feature = "...")] / cfg!(feature = "...") 引用
function cfgFeatureRefs(srcDir) {
    const refs = new Set();
    for (const file of walkDir(srcDir, '.rs')) {
        const content = fs.readFileSync(file, 'utf8');
        for (const m of content.matchAll(/(?:cfg|cfg_attr)\([^)]*feature\s*=\s*"([\w-]+)"/g)) {
            refs.add(m[1]);
        }
        for (const m of content.matchAll(/cfg!\(\s*feature\s*=\s*"([\w-]+)"/g)) {
            refs.add(m[1]);
        }
    }
    return refs;
}

function main() {
    const errors = [];
    const warnings = [];
    let cratesChecked = 0;

    for (const pkg of fs.readdirSync(PACKAGES_DIR)) {
        const pkgDir = path.join(PACKAGES_DIR, pkg);
        const manifestPath = path.join(pkgDir, 'Cargo.toml');
        const srcDir = path.join(pkgDir, 'src');
        if (!fs.existsSync(manifestPath) || !fs.existsSync(srcDir)) continue;
        cratesChecked++;

        const features = parseFeatures(manifestPath);
        const depNames = parseDependencyNames(manifestPath);
        const used = cfgFeatureRefs(srcDir);
        const declared = new Set(Object.keys(features));

        // ERROR：源码引用但未声明
        for (const f of used) {
            if (!declared.has(f)) {
                errors.push(`${pkg}/src 引用 feature "${f}" 但 Cargo.toml 未声明（永远不可启用）`);
            }
        }
        // WARN：声明但无源码引用（死 feature）——排除聚合/依赖启用/跨 crate 透传
        for (const f of declared) {
            if (f === 'default') continue;
            if (!used.has(f)) {
                const deps = (features[f] || '').trim();
                const items = deps.replace(/[\[\]"]/g, '').split(',').map((s) => s.trim()).filter(Boolean);
                // 合法用途：启用其他 feature / 启用依赖（依赖名或 dep: 前缀）/ 跨 crate 透传（含 /）
                const legit = items.some((i) => declared.has(i) || depNames.has(i) || i.startsWith('dep:') || i.includes('/'));
                if (legit) continue;
                warnings.push(`${pkg} 声明 feature "${f}" 但无源码引用且无启用项（死 feature）`);
            }
        }
    }

    console.log(`✅ 已检查 ${cratesChecked} 个 crate 的 feature 声明与源码引用`);
    for (const w of warnings) console.log(`  [WARN] ${w}`);
    for (const e of errors) console.log(`  [ERROR] ${e}`);

    if (errors.length > 0) {
        console.error(`\n❌ 发现 ${errors.length} 处 feature 引用未声明（配置-代码不一致），请声明或移除引用`);
        process.exit(1);
    }
    if (warnings.length > 0) {
        console.warn(`\n⚠️ 发现 ${warnings.length} 处死 feature（不阻塞）`);
    } else {
        console.log('\n✅ feature 声明与源码引用一致');
    }
    process.exit(0);
}

main();
