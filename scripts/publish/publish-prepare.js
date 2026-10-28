#!/usr/bin/env node
/**
 * publish-prepare.js — sz-rust crates.io 发布准备脚本
 *
 * T1: MetadataChecker — 校验 31 crate 元数据完整性
 * T2: VersionBumper  — 升级 workspace 与内部依赖至目标版本
 * T3: BuildValidator — 全量编译与测试验证
 * T4: DependencyResolver — 依赖图构建与拓扑排序
 *
 * 用法：
 *   node publish-prepare.js --check-only       仅检查元数据，不修改文件
 *   node publish-prepare.js --dry-run          显示修改计划但不实际写入
 *   node publish-prepare.js                    执行全流程（检查+版本递增+编译+测试+拓扑排序）
 */

import { readFileSync, writeFileSync, existsSync, readdirSync, statSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { execSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const PROJECT_ROOT = resolve(__dirname, '..', '..');
const CARGO_TOML_PATH = join(PROJECT_ROOT, 'Cargo.toml');

const TARGET_VERSION = '0.7.0';
const REQUIRED_FIELDS = ['name', 'version', 'description', 'license', 'repository', 'homepage', 'keywords', 'categories'];
const EXPECTED_LICENSE = 'MIT';
const EXPECTED_REPO = 'https://github.com/ljclz/sz-rust';

const CRATES_IO_CATEGORIES = new Set([
    'web-programming', 'web-programming::http-server', 'web-programming::http-client',
    'web-programming::websocket', 'web-programming::api', 'command-line-utilities',
    'development-tools', 'development-tools::procedural-macro-helpers',
    'asynchronous', 'caching', 'authentication', 'cryptography', 'database',
    'database-implementations', 'data-structures', 'algorithms', 'config',
    'encoding', 'parser-implementations', 'rust-patterns', 'testing',
    'value-formatting', 'network-programming', 'filesystem', 'science',
    'science::geo', 'visualization', 'simulations', 'emulators',
    'games', 'graphics', 'images', 'multimedia', 'multimedia::images',
    'multimedia::video', 'multimedia::audio', 'text-processing',
    'internationalization', 'localization', 'finance', 'date-and-time',
    'mathematics', 'compression', 'no-std', 'no-std::no-alloc',
    'embedded', 'hardware-support', 'os', 'os::linux-apis',
    'api-bindings', 'external-ffi-bindings',
]);

// ── TOML 辅助解析 ──

function parseTomlString(content, section, key) {
    const sectionRegex = new RegExp(`^\\[${section.replace(/[.*]/g, '\\$&')}\\]`, 'm');
    const sectionMatch = content.match(sectionRegex);
    if (!sectionMatch) return undefined;

    const sectionStart = sectionMatch.index + sectionMatch[0].length;
    const nextSectionMatch = content.slice(sectionStart).match(/\n\[/);
    const sectionEnd = nextSectionMatch ? sectionStart + nextSectionMatch.index : content.length;
    const sectionContent = content.slice(sectionStart, sectionEnd);

    const lines = sectionContent.split('\n');
    for (const line of lines) {
        const trimmed = line.trim();
        if (trimmed.startsWith('#') || !trimmed) continue;

        if (key.includes('.')) {
            const [k1, k2] = key.split('.');
            const pattern = new RegExp(`^${k1}\\.${k2}\\s*=\\s*(.+)$`);
            const m = trimmed.match(pattern);
            if (m) return parseTomlValue(m[1]);
        }

        const pattern = new RegExp(`^${key}\\s*=\\s*(.+)$`);
        const m = trimmed.match(pattern);
        if (m) return parseTomlValue(m[1]);
    }
    return undefined;
}

function parseTomlValue(raw) {
    const trimmed = raw.trim();
    if (trimmed === 'true') return true;
    if (trimmed === 'false') return false;
    if (trimmed.startsWith('"') && trimmed.endsWith('"')) {
        return trimmed.slice(1, -1);
    }
    if (trimmed.startsWith('[') && trimmed.endsWith(']')) {
        const inner = trimmed.slice(1, -1);
        return inner.split(',').map(s => s.trim().replace(/^"|"$/g, '')).filter(s => s);
    }
    return trimmed;
}

function parseWorkspacePackage(content) {
    const fields = {};
    const sectionRegex = /^\[workspace\.package\]/m;
    const sectionMatch = content.match(sectionRegex);
    if (!sectionMatch) return fields;

    const sectionStart = sectionMatch.index + sectionMatch[0].length;
    const nextSectionMatch = content.slice(sectionStart).match(/\n\[/);
    const sectionEnd = nextSectionMatch ? sectionStart + nextSectionMatch.index : content.length;
    const sectionContent = content.slice(sectionStart, sectionEnd);

    for (const line of sectionContent.split('\n')) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith('#')) continue;
        const m = trimmed.match(/^(\w+)\s*=\s*(.+)$/);
        if (m) {
            fields[m[1]] = parseTomlValue(m[2]);
        }
    }
    return fields;
}

function parseWorkspaceMembers(content) {
    const members = [];
    const memberStart = content.indexOf('members = [');
    if (memberStart === -1) return members;

    let depth = 0;
    let i = memberStart + content.slice(memberStart).indexOf('[');
    let start = i + 1;
    for (; i < content.length; i++) {
        if (content[i] === '[') depth++;
        if (content[i] === ']') {
            depth--;
            if (depth === 0) break;
        }
    }
    const memberBlock = content.slice(start, i);
    for (const line of memberBlock.split('\n')) {
        const m = line.trim().match(/^"(.+)"[,]?$/);
        if (m) members.push(m[1]);
    }
    return members;
}

function parseCratePackage(crateTomlContent) {
    const pkg = {};
    const sectionRegex = /^\[package\]/m;
    const sectionMatch = crateTomlContent.match(sectionRegex);
    if (!sectionMatch) return pkg;

    const sectionStart = sectionMatch.index + sectionMatch[0].length;
    const nextSectionMatch = crateTomlContent.slice(sectionStart).match(/\n\[/);
    const sectionEnd = nextSectionMatch ? sectionStart + nextSectionMatch.index : crateTomlContent.length;
    const sectionContent = crateTomlContent.slice(sectionStart, sectionEnd);

    for (const line of sectionContent.split('\n')) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith('#')) continue;
        const m = trimmed.match(/^(\w+)\s*=\s*(.+)$/);
        if (m) {
            pkg[m[1]] = parseTomlValue(m[2]);
        }
        const wm = trimmed.match(/^(\w+)\.workspace\s*=\s*true$/);
        if (wm) {
            pkg[`${wm[1]}__workspace`] = true;
        }
    }
    return pkg;
}

function parseWorkspaceDependencies(content) {
    const deps = {};
    const sectionRegex = /^\[workspace\.dependencies\]/m;
    const sectionMatch = content.match(sectionRegex);
    if (!sectionMatch) return deps;

    const sectionStart = sectionMatch.index + sectionMatch[0].length;
    const nextSectionMatch = content.slice(sectionStart).match(/\n\[/);
    const sectionEnd = nextSectionMatch ? sectionStart + nextSectionMatch.index : content.length;
    const sectionContent = content.slice(sectionStart, sectionEnd);

    for (const line of sectionContent.split('\n')) {
        const trimmed = line.trim();
        if (!trimmed || trimmed.startsWith('#')) continue;
        const m = trimmed.match(/^([\w-]+)\s*=\s*(.+)$/);
        if (m) {
            const depName = m[1];
            const rawVal = m[2].trim();
            if (rawVal.startsWith('{')) {
                const versionMatch = rawVal.match(/version\s*=\s*"([^"]+)"/);
                const pathMatch = rawVal.match(/path\s*=\s*"([^"]+)"/);
                deps[depName] = {
                    version: versionMatch ? versionMatch[1] : null,
                    path: pathMatch ? pathMatch[1] : null,
                    raw: rawVal,
                };
            } else if (rawVal.startsWith('"')) {
                deps[depName] = { version: rawVal.slice(1, -1), path: null, raw: rawVal };
            }
        }
    }
    return deps;
}

// ── T1: MetadataChecker ──

function loadWorkspaceManifest() {
    const content = readFileSync(CARGO_TOML_PATH, 'utf-8');
    return {
        package: parseWorkspacePackage(content),
        members: parseWorkspaceMembers(content),
        dependencies: parseWorkspaceDependencies(content),
        rawContent: content,
    };
}

function loadCrateManifests(workspace) {
    const crates = [];
    for (const member of workspace.members) {
        const crateTomlPath = join(PROJECT_ROOT, member, 'Cargo.toml');
        if (!existsSync(crateTomlPath)) {
            crates.push({ member, path: crateTomlPath, exists: false, package: {} });
            continue;
        }
        const content = readFileSync(crateTomlPath, 'utf-8');
        const pkg = parseCratePackage(content);
        crates.push({
            member,
            path: crateTomlPath,
            exists: true,
            package: pkg,
            rawContent: content,
            publish: pkg.publish !== false,
        });
    }
    return crates;
}

function resolveFieldValue(pkg, key, workspacePackage) {
    if (pkg[`${key}__workspace`] === true) {
        return { value: workspacePackage[key], inherited: true };
    }
    if (pkg[key] !== undefined) {
        return { value: pkg[key], inherited: false };
    }
    return { value: undefined, inherited: false };
}

function checkMetadata(crates, workspacePackage) {
    const results = [];
    for (const crate of crates) {
        if (!crate.exists) {
            results.push({ crate: crate.member, errors: ['Cargo.toml 不存在'], warnings: [], publish: false });
            continue;
        }
        if (!crate.publish) {
            results.push({ crate: crate.member, errors: [], warnings: ['publish=false，跳过元数据校验'], publish: false });
            continue;
        }

        const errors = [];
        const warnings = [];
        const pkg = crate.package;
        const resolved = {};
        for (const field of REQUIRED_FIELDS) {
            const { value, inherited } = resolveFieldValue(pkg, field, workspacePackage);
            resolved[field] = { value, inherited };
            if (value === undefined || value === null || value === '') {
                errors.push(`缺失必填字段: ${field}`);
            } else if (Array.isArray(value) && value.length === 0) {
                errors.push(`缺失必填字段: ${field}（空数组）`);
            }
        }

        results.push({
            crate: pkg.name || crate.member,
            member: crate.member,
            errors,
            warnings,
            publish: true,
            resolved,
        });
    }
    return results;
}

function checkLicenseConsistency(metadataResults, workspacePackage) {
    for (const r of metadataResults) {
        if (!r.publish || r.errors.length > 0) continue;
        const licenseVal = r.resolved.license.value;
        if (licenseVal !== EXPECTED_LICENSE) {
            r.errors.push(`license 应为 "${EXPECTED_LICENSE}"，实际为 "${licenseVal}"`);
        }
    }
}

function checkRepoHomepageConsistency(metadataResults) {
    for (const r of metadataResults) {
        if (!r.publish || r.errors.length > 0) continue;
        const repo = r.resolved.repository.value;
        const homepage = r.resolved.homepage.value;
        if (repo !== EXPECTED_REPO) {
            r.errors.push(`repository 应为 "${EXPECTED_REPO}"，实际为 "${repo}"`);
        }
        if (homepage !== EXPECTED_REPO) {
            r.errors.push(`homepage 应为 "${EXPECTED_REPO}"，实际为 "${homepage}"`);
        }
    }
}

function checkKeywordsCompliance(metadataResults) {
    for (const r of metadataResults) {
        if (!r.publish || r.errors.length > 0) continue;
        const keywords = r.resolved.keywords.value;
        if (!Array.isArray(keywords)) {
            r.errors.push(`keywords 应为数组，实际为 ${typeof keywords}`);
            continue;
        }
        if (keywords.length < 1) {
            r.errors.push('keywords 至少 1 个');
        }
        if (keywords.length > 5) {
            r.errors.push(`keywords 最多 5 个，实际 ${keywords.length} 个`);
        }
        for (const kw of keywords) {
            if (kw.length > 20) {
                r.errors.push(`keyword "${kw}" 超过 20 字符`);
            }
            if (!/^[a-z0-9-]+$/.test(kw)) {
                r.errors.push(`keyword "${kw}" 含非法字符（仅允许小写字母/数字/连字符）`);
            }
        }
    }
}

function checkCategoriesCompliance(metadataResults) {
    for (const r of metadataResults) {
        if (!r.publish || r.errors.length > 0) continue;
        const categories = r.resolved.categories.value;
        if (!Array.isArray(categories)) {
            r.errors.push(`categories 应为数组，实际为 ${typeof categories}`);
            continue;
        }
        if (categories.length < 1) {
            r.errors.push('categories 至少 1 个');
        }
        for (const cat of categories) {
            if (!CRATES_IO_CATEGORIES.has(cat)) {
                r.warnings.push(`category "${cat}" 不在 crates.io 官方分类列表中（可能是新分类）`);
            }
        }
    }
}

function generatePrepareReport(metadataResults) {
    const total = metadataResults.length;
    const publishable = metadataResults.filter(r => r.publish).length;
    const passed = metadataResults.filter(r => r.publish && r.errors.length === 0).length;
    const failed = metadataResults.filter(r => r.publish && r.errors.length > 0).length;
    const skipped = metadataResults.filter(r => !r.publish).length;

    const lines = [];
    lines.push('═══════════════════════════════════════════════════════════════');
    lines.push('  sz-rust crates.io 发布准备 — 元数据校验报告');
    lines.push('═══════════════════════════════════════════════════════════════');
    lines.push(`  总计: ${total} ｜ 可发布: ${publishable} ｜ 通过: ${passed} ｜ 失败: ${failed} ｜ 跳过: ${skipped}`);
    lines.push('───────────────────────────────────────────────────────────────');

    if (failed > 0) {
        lines.push('');
        lines.push('❌ 失败的 crate：');
        for (const r of metadataResults) {
            if (r.publish && r.errors.length > 0) {
                lines.push(`  • ${r.crate} (${r.member})`);
                for (const err of r.errors) {
                    lines.push(`      └─ ${err}`);
                }
            }
        }
    }

    if (skipped > 0) {
        lines.push('');
        lines.push('⏭️  跳过的 crate（publish=false）：');
        for (const r of metadataResults) {
            if (!r.publish) {
                lines.push(`  • ${r.crate || r.member} — ${r.warnings[0] || 'publish=false'}`);
            }
        }
    }

    if (passed > 0) {
        lines.push('');
        lines.push(`✅ 通过的 crate（${passed} 个）：`);
        const passedList = metadataResults.filter(r => r.publish && r.errors.length === 0).map(r => r.crate);
        lines.push(`  ${passedList.join(', ')}`);
    }

    const warningsList = metadataResults.filter(r => r.warnings.length > 0 && r.publish);
    if (warningsList.length > 0) {
        lines.push('');
        lines.push('⚠️  警告：');
        for (const r of warningsList) {
            for (const w of r.warnings) {
                lines.push(`  • ${r.crate}: ${w}`);
            }
        }
    }

    lines.push('');
    lines.push('═══════════════════════════════════════════════════════════════');

    return { text: lines.join('\n'), total, publishable, passed, failed, skipped };
}

// ── T2: VersionBumper ──

function bumpWorkspaceVersion(targetVersion, dryRun) {
    const content = readFileSync(CARGO_TOML_PATH, 'utf-8');
    const lines = content.split('\n');
    const changes = [];

    for (let i = 0; i < lines.length; i++) {
        const trimmed = lines[i].trim();
        if (trimmed.startsWith('version = "') && trimmed.endsWith('"')) {
            const oldVersion = trimmed.match(/version = "([^"]+)"/)[1];
            if (oldVersion !== targetVersion) {
                changes.push({
                    file: 'Cargo.toml',
                    line: i + 1,
                    old: trimmed,
                    new: `version = "${targetVersion}"`,
                });
                if (!dryRun) {
                    lines[i] = lines[i].replace(`version = "${oldVersion}"`, `version = "${targetVersion}"`);
                }
            }
        }
    }

    let inWsDeps = false;
    for (let i = 0; i < lines.length; i++) {
        const trimmed = lines[i].trim();
        if (trimmed === '[workspace.dependencies]') {
            inWsDeps = true;
            continue;
        }
        if (inWsDeps && trimmed.startsWith('[') && trimmed !== '[workspace.dependencies]') {
            inWsDeps = false;
            continue;
        }
        if (!inWsDeps) continue;
        if (!trimmed || trimmed.startsWith('#')) continue;

        const szRustMatch = trimmed.match(/^(sz-rust-[\w-]+)\s*=\s*\{\s*version\s*=\s*"([^"]+)"(.*)\}/);
        if (szRustMatch) {
            const depName = szRustMatch[1];
            const oldVer = szRustMatch[2];
            const rest = szRustMatch[3];
            if (oldVer !== targetVersion) {
                changes.push({
                    file: 'Cargo.toml',
                    line: i + 1,
                    old: trimmed,
                    new: `${depName} = { version = "${targetVersion}"${rest} }`,
                });
                if (!dryRun) {
                    lines[i] = lines[i].replace(
                        `version = "${oldVer}"`,
                        `version = "${targetVersion}"`
                    );
                }
            }
        }
    }

    if (!dryRun && changes.length > 0) {
        writeFileSync(CARGO_TOML_PATH, lines.join('\n'), 'utf-8');
    }

    return changes;
}

function verifyNoSzOrmChanges(changes) {
    const szOrmChanges = changes.filter(c =>
        c.old.includes('sz-orm-') || c.new.includes('sz-orm-')
    );
    return szOrmChanges.length === 0;
}

// ── T3: BuildValidator ──

function runCargoBuildRelease() {
    console.log('\n🔧 执行 cargo build --release --workspace ...');
    try {
        execSync('cargo build --release --workspace', {
            cwd: PROJECT_ROOT,
            timeout: 600000,
            stdio: 'pipe',
        });
        console.log('✅ 编译验证通过');
        return { success: true };
    } catch (err) {
        const stderr = err.stderr ? err.stderr.toString() : '';
        console.error('❌ 编译失败');
        console.error(stderr.slice(-2000));
        return { success: false, error: stderr };
    }
}

function runCargoTest() {
    console.log('\n🧪 执行 cargo test --workspace ...');
    try {
        const stdout = execSync('cargo test --workspace', {
            cwd: PROJECT_ROOT,
            timeout: 1200000,
            stdio: 'pipe',
            encoding: 'utf-8',
        });
        const failedMatch = stdout.match(/(\d+) failed/);
        const passedMatch = stdout.match(/(\d+) passed/);
        const failed = failedMatch ? parseInt(failedMatch[1]) : 0;
        const passed = passedMatch ? parseInt(passedMatch[1]) : 0;
        if (failed > 0) {
            console.error(`❌ 测试失败: ${failed} failed`);
            return { success: false, failed, passed, error: stdout };
        }
        console.log(`✅ 测试验证通过: ${passed} passed, ${failed} failed`);
        return { success: true, passed, failed };
    } catch (err) {
        const stderr = err.stderr ? err.stderr.toString() : '';
        const stdout = err.stdout ? err.stdout.toString() : '';
        console.error('❌ 测试执行失败');
        console.error((stderr + stdout).slice(-2000));
        return { success: false, error: stderr + stdout };
    }
}

// ── T4: DependencyResolver ──

function parsePathDependencies(crate, workspace) {
    const deps = [];
    if (!crate.rawContent) return deps;

    const sections = ['[dependencies]', '[dev-dependencies]'];
    for (const section of sections) {
        const sectionRegex = new RegExp(`^\\${section}`, 'm');
        const sectionMatch = crate.rawContent.match(sectionRegex);
        if (!sectionMatch) continue;

        const sectionStart = sectionMatch.index + sectionMatch[0].length;
        const nextSectionMatch = crate.rawContent.slice(sectionStart).match(/\n\[/);
        const sectionEnd = nextSectionMatch ? sectionStart + nextSectionMatch.index : crate.rawContent.length;
        const sectionContent = crate.rawContent.slice(sectionStart, sectionEnd);

        for (const line of sectionContent.split('\n')) {
            const trimmed = line.trim();
            if (!trimmed || trimmed.startsWith('#')) continue;

            const wsDotMatch = trimmed.match(/^(sz-rust-[\w-]+)\.workspace\s*=\s*true$/);
            if (wsDotMatch) {
                deps.push({ name: wsDotMatch[1], type: 'workspace' });
                continue;
            }

            const m = trimmed.match(/^(sz-rust-[\w-]+)\s*=\s*(.+)$/);
            if (!m) continue;
            const depName = m[1];
            const rawVal = m[2].trim();

            if (rawVal.includes('workspace = true') || rawVal.includes('workspace=true')) {
                deps.push({ name: depName, type: 'workspace' });
            } else if (rawVal.includes('path')) {
                const pathMatch = rawVal.match(/path\s*=\s*"([^"]+)"/);
                deps.push({ name: depName, type: 'path', path: pathMatch ? pathMatch[1] : null });
            }
        }
    }

    return deps;
}

function buildDAG(crates, workspace) {
    const dag = new Map();
    const crateNames = new Set();

    for (const crate of crates) {
        if (!crate.exists || !crate.publish) continue;
        const name = crate.package.name || crate.member;
        crateNames.add(name);
        dag.set(name, []);
    }

    for (const crate of crates) {
        if (!crate.exists || !crate.publish) continue;
        const name = crate.package.name || crate.member;
        const deps = parsePathDependencies(crate, workspace);
        for (const dep of deps) {
            if (crateNames.has(dep.name)) {
                dag.get(name).push(dep.name);
            }
        }
    }

    return dag;
}

function detectCycle(dag) {
    const WHITE = 0, GRAY = 1, BLACK = 2;
    const color = new Map();
    for (const [node] of dag) color.set(node, WHITE);

    let cyclePath = null;

    function dfs(node, path) {
        color.set(node, GRAY);
        path.push(node);

        for (const dep of dag.get(node) || []) {
            if (color.get(dep) === GRAY) {
                const cycleStart = path.indexOf(dep);
                cyclePath = path.slice(cycleStart).concat(dep);
                return true;
            }
            if (color.get(dep) === WHITE) {
                if (dfs(dep, path)) return true;
            }
        }

        color.set(node, BLACK);
        path.pop();
        return false;
    }

    for (const [node] of dag) {
        if (color.get(node) === WHITE) {
            if (dfs(node, [])) return cyclePath;
        }
    }
    return null;
}

function topologicalSort(dag) {
    // dag.get(A) = [B, C] 表示 A 依赖 B 和 C
    // 发布顺序：B 和 C 必须在 A 之前（被依赖的先发布）
    // inDegree[A] = A 的依赖数量
    // reverseDag.get(B) = [A] 表示 A 依赖 B（反向边）

    const inDegree = new Map();
    const reverseDag = new Map();

    for (const [node, deps] of dag) {
        inDegree.set(node, deps.length);
        if (!reverseDag.has(node)) reverseDag.set(node, []);
    }

    for (const [node, deps] of dag) {
        for (const dep of deps) {
            if (!reverseDag.has(dep)) reverseDag.set(dep, []);
            reverseDag.get(dep).push(node);
        }
    }

    const queue = [];
    for (const [node, deg] of inDegree) {
        if (deg === 0) queue.push(node);
    }
    queue.sort();

    const sorted = [];
    while (queue.length > 0) {
        const node = queue.shift();
        sorted.push(node);

        const dependents = reverseDag.get(node) || [];
        for (const dependent of dependents) {
            inDegree.set(dependent, inDegree.get(dependent) - 1);
            if (inDegree.get(dependent) === 0) {
                queue.push(dependent);
                queue.sort();
            }
        }
    }

    return sorted;
}

function assignLayer(crateName, dag, memo = new Map()) {
    if (memo.has(crateName)) return memo.get(crateName);
    const deps = dag.get(crateName) || [];
    if (deps.length === 0) {
        memo.set(crateName, 0);
        return 0;
    }
    const maxDepLayer = Math.max(...deps.map(d => assignLayer(d, dag, memo)));
    const layer = maxDepLayer + 1;
    memo.set(crateName, layer);
    return layer;
}

function generatePublishOrder(sortedCrates, dag) {
    const layerMemo = new Map();
    const entries = sortedCrates.map((crateName, index) => {
        const layer = assignLayer(crateName, dag, layerMemo);
        const deps = dag.get(crateName) || [];
        return {
            crate: crateName,
            layer,
            dependencies: deps,
            publish_order: index + 1,
        };
    });

    const outputPath = join(__dirname, 'publish-order.json');
    writeFileSync(outputPath, JSON.stringify(entries, null, 2), 'utf-8');
    return { entries, outputPath };
}

// ── 主流程 ──

function main() {
    const args = process.argv.slice(2);
    const checkOnly = args.includes('--check-only');
    const dryRun = args.includes('--dry-run');
    const skipBuild = args.includes('--skip-build');

    console.log('═══════════════════════════════════════════════════════════════');
    console.log('  sz-rust crates.io 发布准备');
    console.log(`  模式: ${checkOnly ? '仅检查' : dryRun ? 'dry-run' : '全流程'}`);
    console.log('═══════════════════════════════════════════════════════════════\n');

    // ── T1: 元数据校验 ──
    console.log('📋 T1: MetadataChecker — 元数据校验');
    const workspace = loadWorkspaceManifest();
    console.log(`  workspace.package.version = ${workspace.package.version}`);
    console.log(`  workspace members: ${workspace.members.length}`);

    const crates = loadCrateManifests(workspace);
    const metadataResults = checkMetadata(crates, workspace.package);
    checkLicenseConsistency(metadataResults, workspace.package);
    checkRepoHomepageConsistency(metadataResults);
    checkKeywordsCompliance(metadataResults);
    checkCategoriesCompliance(metadataResults);

    const report = generatePrepareReport(metadataResults);
    console.log(report.text);

    if (report.failed > 0) {
        console.error(`\n❌ 元数据校验失败: ${report.failed} 个 crate 有错误`);
        console.error('   请修复上述缺失字段后再继续');
        process.exit(10);
    }

    if (checkOnly) {
        console.log('\n✅ --check-only 模式，仅检查完成');
        process.exit(0);
    }

    // ── T2: 版本递增 ──
    console.log('\n📋 T2: VersionBumper — 版本递增');
    const currentVersion = workspace.package.version;
    console.log(`  当前版本: ${currentVersion} → 目标版本: ${TARGET_VERSION}`);
    const changes = bumpWorkspaceVersion(TARGET_VERSION, dryRun);
    if (changes.length === 0) {
        console.log('  无需修改（workspace.version 和内部依赖均已是目标版本）');
    } else {
        console.log(`  ${dryRun ? '[dry-run] 计划修改' : '已修改'} ${changes.length} 处:`);
        for (const c of changes) {
            console.log(`    ${c.file}:${c.line}: ${c.old} → ${c.new}`);
        }

        if (!verifyNoSzOrmChanges(changes)) {
            console.error('\n❌ 错误: sz-orm-* 依赖被修改（违反约束）');
            process.exit(1);
        }
        console.log('  ✅ sz-orm-* 依赖未被修改');
    }

    // ── T3: 编译与测试验证 ──
    if (skipBuild) {
        console.log('\n📋 T3: BuildValidator — 跳过（--skip-build）');
    } else {
        console.log('\n📋 T3: BuildValidator — 编译与测试验证');
        const buildResult = runCargoBuildRelease();
        if (!buildResult.success) {
            console.error('\n❌ 编译失败，中止发布准备');
            process.exit(11);
        }

        const testResult = runCargoTest();
        if (!testResult.success) {
            console.error('\n❌ 测试失败，中止发布准备');
            process.exit(12);
        }
    }

    // ── T4: 依赖图与拓扑排序 ──
    console.log('\n📋 T4: DependencyResolver — 依赖图与拓扑排序');
    const refreshedWorkspace = loadWorkspaceManifest();
    const refreshedCrates = loadCrateManifests(refreshedWorkspace);
    const publishableCrates = refreshedCrates.filter(c => c.exists && c.publish);

    const dag = buildDAG(refreshedCrates, refreshedWorkspace);
    console.log(`  依赖图节点数: ${dag.size}`);

    const cycle = detectCycle(dag);
    if (cycle) {
        console.error(`\n❌ 检测到依赖环: ${cycle.join(' → ')}`);
        process.exit(13);
    }
    console.log('  ✅ 无依赖环');

    const sorted = topologicalSort(dag);
    console.log(`  拓扑排序完成: ${sorted.length} 个 crate`);

    const { entries, outputPath } = generatePublishOrder(sorted, dag);

    const layerGroups = {};
    for (const e of entries) {
        if (!layerGroups[`L${e.layer}`]) layerGroups[`L${e.layer}`] = [];
        layerGroups[`L${e.layer}`].push(e.crate);
    }
    console.log('\  层级分组:');
    for (const [layer, crates] of Object.entries(layerGroups).sort()) {
        console.log(`    ${layer} (${crates.length}): ${crates.join(', ')}`);
    }

    console.log(`\n  发布顺序已写入: ${outputPath}`);

    // 验证拓扑序
    const orderMap = new Map();
    for (const e of entries) orderMap.set(e.crate, e.publish_order);
    let topoValid = true;
    for (const e of entries) {
        for (const dep of e.dependencies) {
            if (orderMap.get(dep) >= e.publish_order) {
                console.error(`  ❌ 拓扑序错误: ${dep} (order=${orderMap.get(dep)}) 应在 ${e.crate} (order=${e.publish_order}) 之前`);
                topoValid = false;
            }
        }
    }
    if (topoValid) {
        console.log('  ✅ 拓扑序验证通过');
    } else {
        process.exit(13);
    }

    console.log('\n═══════════════════════════════════════════════════════════════');
    console.log('  ✅ 发布准备完成');
    console.log(`  可发布 crate: ${report.publishable} 个`);
    console.log(`  目标版本: ${TARGET_VERSION}`);
    console.log(`  发布顺序: ${outputPath}`);
    console.log('═══════════════════════════════════════════════════════════════');
}

main();