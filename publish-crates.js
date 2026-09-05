#!/usr/bin/env node
// publish-crates.js — sz-rust crates.io 发布脚本
//
// 功能：
//   1. 按依赖拓扑顺序发布 sz-rust workspace 中的所有 crate
//   2. 检查阻塞条件（sz-orm-* 是否已在 crates.io 上发布所需版本）
//   3. 支持 dry-run 模式（仅验证不实际发布）
//   4. 验证每个 crate 发布成功
//
// 用法：
//   node publish-crates.js                    # dry-run 模式，仅检查
//   node publish-crates.js --publish          # 实际发布
//   node publish-crates.js --check-only       # 仅检查阻塞条件
//
// 环境变量：
//   CARGO_REGISTRY_TOKEN  crates.io API token（或用 cargo login 预设）

const { execSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const WORKSPACE_ROOT = path.resolve(__dirname);
const DRY_RUN = !process.argv.includes('--publish');
const CHECK_ONLY = process.argv.includes('--check-only');
const SKIP_CHECK = process.argv.includes('--skip-check');

// sz-rust crate 发布拓扑顺序（依赖在前）
const PUBLISH_ORDER = [
    // 第一层：无内部依赖
    'sz-rust-macros',
    'sz-rust-tracing',
    'sz-rust-observability',
    'sz-rust-pdf',
    'sz-rust-operator',
    'sz-rust-addons-loader',
    // 第二层：仅依赖 macros（无 facade 依赖）
    'sz-rust-router-facade',
    'sz-rust-http-facade',
    'sz-rust-auth-facade',
    'sz-rust-orm-facade',
    'sz-rust-state-facade',
    'sz-rust-pay-facade',
    // 第三层：依赖第二层 facade
    'sz-rust-cache-facade',
    'sz-rust-infra-facade',
    'sz-rust-orm-ext-facade',
    // 第四层：依赖第三层 facade
    'sz-rust-middleware-facade',
    'sz-rust-mvc-facade',
    // 第五层：依赖第四层 facade
    'sz-rust-mcp',
    'sz-rust-core',
    // 第六层：依赖 core
    'sz-rust-addons-operate',
    'sz-rust-addons-erp',
    'sz-rust-addons-ecommerce',
    'sz-rust-addons-crm',
    'sz-rust-sz300',
    // 第七层：依赖业务包
    'sz-rust-cli',
    'sz-rust-examples',
];

// sz-orm 依赖检查列表（crates.io 上必须存在的版本）
const SZ_ORM_DEPS = [
    'sz-orm-core',
    'sz-orm-auth',
    'sz-orm-storage',
    'sz-orm-queue',
    'sz-orm-mqtt',
    'sz-orm-websocket',
    'sz-orm-scheduler',
    'sz-orm-tracing',
    'sz-orm-logger',
    'sz-orm-limit',
    'sz-orm-config',
    'sz-orm-macros',
    'sz-orm-sql-validator',
    'sz-orm-sqlx',
    'sz-orm-query-builder',
    'sz-orm-graphql',
    'sz-orm-grpc',
];

const REQUIRED_SZ_ORM_VERSION = '2.1.0';

function log(msg) { console.log(`[publish] ${msg}`); }
function warn(msg) { console.warn(`[publish] WARN: ${msg}`); }
function error(msg) { console.error(`[publish] ERROR: ${msg}`); }

// ── 检查 sz-orm 在 crates.io 上的版本 ──
function checkSzOrmOnCratesIo() {
    log(`检查 sz-orm-* 在 crates.io 上的版本（需要 ${REQUIRED_SZ_ORM_VERSION}）...`);
    let blocked = [];

    for (const pkg of SZ_ORM_DEPS) {
        try {
            const output = execSync(`cargo search ${pkg} --limit 5 2>&1`, { encoding: 'utf-8' });
            const match = output.match(new RegExp(`^${pkg}\\s*=\\s*"([\\d.]+)"`));
            if (match) {
                const version = match[1];
                if (version === REQUIRED_SZ_ORM_VERSION) {
                    log(`  ✅ ${pkg} = ${version}`);
                } else {
                    warn(`  ⚠ ${pkg} = ${version}（需要 ${REQUIRED_SZ_ORM_VERSION}）`);
                    blocked.push({ pkg, found: version, required: REQUIRED_SZ_ORM_VERSION });
                }
            } else {
                warn(`  ⚠ ${pkg} 未在 crates.io 上找到`);
                blocked.push({ pkg, found: null, required: REQUIRED_SZ_ORM_VERSION });
            }
        } catch (e) {
            warn(`  ⚠ ${pkg} 查询失败: ${e.message}`);
            blocked.push({ pkg, found: null, required: REQUIRED_SZ_ORM_VERSION });
        }
    }

    return blocked;
}

// ── 获取 crate 版本 ──
function getCrateVersion(crateName) {
    const cargoTomlPath = path.join(WORKSPACE_ROOT, 'packages', crateName, 'Cargo.toml');
    if (!fs.existsSync(cargoTomlPath)) {
        error(`Cargo.toml 不存在: ${cargoTomlPath}`);
        return null;
    }
    const content = fs.readFileSync(cargoTomlPath, 'utf-8');
    const match = content.match(/^version\s*=\s*"([\d.]+)"/m);
    if (match) return match[1];

    // 检查 workspace.version
    const wsMatch = content.match(/^version\.workspace\s*=\s*true/m);
    if (wsMatch) {
        const wsContent = fs.readFileSync(path.join(WORKSPACE_ROOT, 'Cargo.toml'), 'utf-8');
        const wsVersionMatch = wsContent.match(/^version\s*=\s*"([\d.]+)"/m);
        if (wsVersionMatch) return wsVersionMatch[1];
    }
    return null;
}

// ── 检查 crate 是否已在 crates.io 上 ──
function isOnCratesIo(crateName, version) {
    try {
        const output = execSync(`cargo search ${crateName} --limit 5 2>&1`, { encoding: 'utf-8' });
        const match = output.match(new RegExp(`^${crateName}\\s*=\\s*"([\\d.]+)"`));
        if (match) {
            return { exists: true, version: match[1] };
        }
    } catch (e) { }
    return { exists: false, version: null };
}

// ── 发布单个 crate ──
function publishCrate(crateName) {
    const cratePath = path.join(WORKSPACE_ROOT, 'packages', crateName);
    const version = getCrateVersion(crateName);
    if (!version) {
        error(`无法获取 ${crateName} 版本`);
        return false;
    }

    const onIo = isOnCratesIo(crateName, version);
    if (onIo.exists && onIo.version === version) {
        log(`  ⏭ ${crateName} ${version} 已在 crates.io 上，跳过`);
        return true;
    }

    log(`  📦 发布 ${crateName} ${version}...`);

    if (DRY_RUN) {
        log(`    [dry-run] cargo publish --manifest-path ${cratePath}/Cargo.toml`);
        return true;
    }

    const MAX_RETRIES = 6;
    const RETRY_DELAY_MS = 620 * 1000; // 10 分 20 秒

    for (let attempt = 1; attempt <= MAX_RETRIES; attempt++) {
        try {
            const output = execSync(`cargo publish --allow-dirty --manifest-path ${cratePath}/Cargo.toml`, {
                stdio: 'pipe',
                env: { ...process.env, CARGO_INCREMENTAL: '0' },
            });
            if (output) process.stdout.write(output);
            log(`  ✅ ${crateName} ${version} 发布成功`);
            return true;
        } catch (e) {
            if (e.stdout) process.stdout.write(e.stdout);
            if (e.stderr) process.stderr.write(e.stderr);
            const errMsg = (e.message || '') + (e.stderr ? e.stderr.toString() : '');
            if (errMsg.includes('already exists')) {
                log(`  ⏭ ${crateName} ${version} 已存在，视为成功`);
                return true;
            }
            if (errMsg.includes('429') || errMsg.includes('Too Many Requests')) {
                if (attempt < MAX_RETRIES) {
                    log(`  ⏳ ${crateName} 遇到速率限制，等待 10 分 20 秒后重试 (${attempt}/${MAX_RETRIES})...`);
                    execSync(`node -e "setTimeout(() => process.exit(0), ${RETRY_DELAY_MS})"`, { stdio: 'inherit' });
                    continue;
                }
            }
            error(`  ❌ ${crateName} ${version} 发布失败`);
            return false;
        }
    }
    return false;
}

// ── 主流程 ──
function main() {
    log('sz-rust crates.io 发布脚本');
    log(`模式: ${CHECK_ONLY ? '仅检查' : DRY_RUN ? 'dry-run' : '实际发布'}`);
    log('');

    // 1. 检查 sz-orm 阻塞条件
    if (SKIP_CHECK) {
        log('⏭ 跳过 sz-orm 依赖检查（--skip-check）');
    } else {
        const blocked = checkSzOrmOnCratesIo();
        if (blocked.length > 0) {
            log('');
            error(`sz-orm 依赖未就绪，${blocked.length} 个包版本不匹配：`);
            for (const b of blocked) {
                error(`  - ${b.pkg}: 需要 ${b.required}, 找到 ${b.found || '未发布'}`);
            }
            error('请先发布 sz-orm 2.0.0 到 crates.io，然后再发布 sz-rust');
            process.exit(1);
        }

        log('');
        log('✅ sz-orm 依赖检查通过');
    }

    if (CHECK_ONLY) {
        log('仅检查模式，退出');
        process.exit(0);
    }

    // 2. 按拓扑顺序发布
    log('');
    log(`按拓扑顺序发布 ${PUBLISH_ORDER.length} 个 crate...`);
    let success = 0;
    let failed = 0;

    for (const crateName of PUBLISH_ORDER) {
        if (publishCrate(crateName)) {
            success++;
        } else {
            failed++;
            error(`发布中断：${crateName} 失败`);
            break;
        }
    }

    // 3. 汇总
    log('');
    log(`发布完成：成功 ${success}，失败 ${failed}`);
    if (failed > 0) {
        process.exit(1);
    }
}

main();