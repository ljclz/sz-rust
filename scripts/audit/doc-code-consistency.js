#!/usr/bin/env node
'use strict';

/**
 * 文档-代码一致性审计脚本（防幻影交付，铁律 23）
 *
 * 扫描交付声称类文档（README / CHANGELOG / docs 根目录 / docs/audit 根目录），
 * 提取所有 `sz-rust-*` crate 名，与 `packages/` 目录 + Cargo.toml workspace members 交叉验证：
 *   - ERROR：文档声称的 crate 不存在于 packages/，且非「已定性虚构+标注回退」引用
 *             → 幻影交付，构建失败（含 KNOWN_FICTIONAL_CRATES 清单内未标注回退的声称）
 *   - WARN ：crate 存在但非 workspace member（如 sz-rust-wasm）
 *             或已定性虚构 crate 的审计回退标注引用（行内含 未完成/虚构/不存在）
 *
 * 排除规则：
 *   - 扫描范围仅限交付声称文档（docs/cases 愿景、docs/spec 规划、archive 历史文档不扫描）
 *   - `sz-rust-xxx.md` 文档路径引用不算 crate 声称
 *   - `sz-rust-addons-{cms,...}` 占位符写法不算声称
 *   - 文档中 `<!-- doc-code-consistency: ignore-begin --> ... ignore-end -->`
 *     注释块内不检查（用于审计报告引用"不存在"的 crate 时自我豁免）
 *
 * 用法：node scripts/audit/doc-code-consistency.js
 * 退出码：0 = 无 ERROR，1 = 存在 ERROR
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const PACKAGES_DIR = path.join(ROOT, 'packages');

// 扫描范围：交付声称类文档（不递归子目录，排除 archive/cases/spec 等非声称文档）
const SCAN_FILES = [
    'README.md',
    'README.en.md',
    'CHANGELOG.md',
    ...lsMd(path.join(ROOT, 'docs')),
    ...lsMd(path.join(ROOT, 'docs', 'audit')),
    // CI workflow 中的 `cargo test -p sz-rust-xxx` 也是交付声称（构建门禁引用）
    ...lsFile(path.join(ROOT, '.github', 'workflows'), '.yml'),
    ...lsFile(path.join(ROOT, '.github', 'workflows'), '.yaml'),
];

// 已定性虚构的 crate（2026-08-14 核验：开源版与企业版仓库 git 历史中均从未存在，
// 见 docs/audit/2026-08-13-文档已实现但生产零调用审计报告.md + doc-debt DB-2026-08-13-01）。
// 清单内 crate 的"声称"视为虚构（ERROR）；但行内已标注「未完成/虚构/不存在」的引用豁免（WARN）。
const KNOWN_FICTIONAL_CRATES = new Set([
    'sz-rust-marketplace',
    'sz-rust-visual',
    'sz-rust-sdd-agent',
    'sz-rust-migration',
]);

// 已移除的 crate（2026-08-15 移除：非框架核心、零测试、零依赖方）
// 历史文档/审计报告中的引用降级为 WARN，不阻塞 CI。
const REMOVED_CRATES = new Set([
    'sz-rust-operator',
    'sz-rust-wasm',
]);

// 非 crate 名：企业版仓库名 + .trae/skills/ 下的 Skill 目录名（文档常引用 Skill 名，非交付声称）
// + 已核验的部署目录/cron 标记（sz-rust-soak：soak-toolkit 工作目录名，见 scripts/soak-self-hosted/）
const NON_CRATE_NAMES = new Set([
    'sz-rust-enterprise',
    'sz-rust-soak',
    ...lsDir(path.join(ROOT, '.trae', 'skills')),
]);

// 文档整体标注企业版交付 → 该文档内"不存在"声称降级为 WARN
const EXTERNAL_REPO_MARKERS = /企业版|enterprise|商业版/i;

const IGNORE_BEGIN = '<!-- doc-code-consistency: ignore-begin -->';
const IGNORE_END = '<!-- doc-code-consistency: ignore-end -->';

// 声称分级制（铁律 20/23）：完成声称必须附证据。
// 「已完成 / 已实现 / 全部完成 / 已落地 / 测试通过」+ crate 名 = 完成声称，
// 行内必须附证据标记：来源 / cargo test / commit / 路径 :行号 / 测试数。
const COMPLETION_CLAIM_RE = /(?:已完成|全部完成|已实现|已落地|已交付|测试通过|tests? passed|全部通过)/;
const EVIDENCE_RE = /(?:来源|source|cargo (?:test|check|clippy)|\d+\s*[+]?\s*(?:个)?(?:测试|tests?|passed)|\d+\s*(?:个)?测试|:\d+|commit|SHA|PR\s*#|企业版|enterprise|✅|测试数|生产接入状态|未挂载|未接入|已实现但)/;

function lsDir(dir) {
    if (!fs.existsSync(dir)) return [];
    return fs.readdirSync(dir).filter((f) => f.startsWith('sz-rust-'));
}

function lsMd(dir) {
    if (!fs.existsSync(dir)) return [];
    return fs.readdirSync(dir)
        .filter((f) => f.endsWith('.md'))
        .map((f) => path.join(dir, f));
}

function lsFile(dir, ext) {
    if (!fs.existsSync(dir)) return [];
    return fs.readdirSync(dir)
        .filter((f) => f.endsWith(ext))
        .map((f) => path.join(dir, f));
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

function isIgnoredRegion(lines, i) {
    // 向前找最近的 ignore-begin/ignore-end，判断当前行是否在豁免块内
    for (let j = i; j >= 0; j--) {
        if (lines[j].includes(IGNORE_BEGIN)) return true;
        if (lines[j].includes(IGNORE_END)) return false;
    }
    return false;
}

// 声称分级制：扫描「完成声称」（crate 名 + 完成词但无证据标记）→ WARN 提示补证据
function ungradedClaims(file) {
    const content = fs.readFileSync(file, 'utf8');
    const lines = content.split('\n');
    const hits = [];
    lines.forEach((line, idx) => {
        if (isIgnoredRegion(lines, idx)) return;
        if (!COMPLETION_CLAIM_RE.test(line)) return;
        if (!/sz-rust-[a-z0-9-]+/.test(line)) return;
        if (EVIDENCE_RE.test(line)) return;
        hits.push({ line: idx + 1, text: line.trim().slice(0, 100) });
    });
    return hits;
}

function extractCrateNames(file) {
    const content = fs.readFileSync(file, 'utf8');
    const lines = content.split('\n');
    const names = new Map(); // crateName -> [{ line, text }]
    lines.forEach((line, idx) => {
        const lineno = idx + 1;
        if (isIgnoredRegion(lines, idx)) return;
        const matches = line.matchAll(/sz-rust-[a-z0-9-]+/g);
        for (const m of matches) {
            const name = m[0];
            // 占位符写法 `sz-rust-addons-{cms,...}` 或 `sz-rust-{xxx}` → 以 - 结尾
            if (name.endsWith('-')) continue;
            // 路径段引用 `/www/rust/sz-rust-soak`、`packages/sz-rust-xxx` 等 → 前一个字符是路径分隔符
            const prev = m.index > 0 ? line[m.index - 1] : '';
            if (prev === '/' || prev === '\\') continue;
            // 文档路径引用 `docs/sz-rust-xxx.md` → 下一个字符是 .md
            const rest = line.slice(m.index + name.length);
            if (rest.trimStart().startsWith('.md')) continue;
            // 仓库名 / Skill 目录名，非 crate
            if (NON_CRATE_NAMES.has(name)) continue;
            if (!names.has(name)) names.set(name, []);
            names.get(name).push({ line: lineno, text: line.trim() });
        }
    });
    return names;
}

function main() {
    const members = workspaceMembers();
    const errors = [];
    const warnings = [];
    let checked = 0;

    for (const file of SCAN_FILES) {
        if (!fs.existsSync(file)) continue;
        const rel = path.relative(ROOT, file).replace(/\\/g, '/');
        const content = fs.readFileSync(file, 'utf8');
        const docExternal = EXTERNAL_REPO_MARKERS.test(content);
        const claimed = extractCrateNames(file);

        // 声称分级制：完成声称未附证据 → WARN
        for (const hit of ungradedClaims(file)) {
            warnings.push(`${rel}:${hit.line} 完成声称未附证据（铁律 20/23，请标注来源/测试输出）: ${hit.text}`);
        }

        for (const [name, refs] of claimed) {
            const exists = fs.existsSync(path.join(PACKAGES_DIR, name));
            const isMember = members.has(name);
            if (exists && isMember) continue; // ✅ 存在且为 workspace member
            checked++;

            const firstRef = refs[0];
            if (!exists) {
                // 已定性虚构 crate：文档内标注「已定性虚构/纯虚构/未完成」= 审计回退后的文档，WARN 豁免；
                // 未标注 = 仍将其作为有效声称 → ERROR（防虚构交付再犯）
                const docFictional = /已定性虚构|纯虚构|虚构交付|未完成|fictional/.test(content);
                const fictionalAnnotated = docFictional || refs.some((r) => /未完成|虚构|不存在|从未存在|fictional/.test(r.text));
                // 已移除 crate：历史文档引用降级为 WARN
                if (REMOVED_CRATES.has(name)) {
                    warnings.push(`${rel}:${firstRef.line} 引用 ${name}（已移除 crate，历史文档引用）`);
                } else if (KNOWN_FICTIONAL_CRATES.has(name) && fictionalAnnotated) {
                    warnings.push(`${rel}:${firstRef.line} 引用 ${name}（已定性虚构，当前为审计回退标注引用）`);
                } else if (KNOWN_FICTIONAL_CRATES.has(name) || docExternal) {
                    errors.push(`${rel}:${firstRef.line} 声称 ${name}（已定性虚构交付：crate 在开源/企业版仓库均不存在，禁止作为有效声称）`);
                } else {
                    errors.push(`${rel}:${firstRef.line} 声称 ${name}（幻影交付：packages/${name} 不存在且未标注所属仓库）`);
                }
            } else if (!isMember) {
                warnings.push(`${rel}:${firstRef.line} 声称 ${name}（存在于 packages/ 但非 Cargo.toml workspace member）`);
            }
        }
    }

    console.log(`✅ 已扫描 ${SCAN_FILES.length} 个交付声称文档，发现 ${checked} 处需核验的 sz-rust-* 声称`);
    for (const w of warnings) console.log(`  [WARN] ${w}`);
    for (const e of errors) console.log(`  [ERROR] ${e}`);

    if (errors.length > 0) {
        console.error(`\n❌ 发现 ${errors.length} 处幻影交付声称（违反铁律 23），请核实：crate 是否应存在、是否需标注企业版仓库`);
        process.exit(1);
    }
    if (warnings.length > 0) {
        console.warn(`\n⚠️ 发现 ${warnings.length} 处需核验项（不阻塞，含已知企业版交付，跟踪见 docs/audit/doc-debt.md）`);
    } else {
        console.log('\n✅ 文档声称的 crate 与代码一致，无幻影交付');
    }
    process.exit(0);
}

main();
