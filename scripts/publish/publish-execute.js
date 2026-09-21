#!/usr/bin/env node
/**
 * publish-execute.js — sz-rust crates.io 发布执行脚本
 *
 * T5: PublishExecutor — 登录与按拓扑序发布
 * T6: AuditLogger    — 审计日志与发布后验证
 *
 * 用法：
 *   CARGO_REGISTRY_TOKEN=xxx node publish-execute.js --dry-run   dry-run 验证打包
 *   CARGO_REGISTRY_TOKEN=xxx node publish-execute.js             正式发布
 *   CARGO_REGISTRY_TOKEN=xxx node publish-execute.js --skip a,b  跳过指定 crate
 */

import { readFileSync, writeFileSync, existsSync, appendFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { execSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const PROJECT_ROOT = resolve(__dirname, '..', '..');
const PUBLISH_ORDER_PATH = join(__dirname, 'publish-order.json');

const TARGET_VERSION = '0.7.0';
const CRATES_IO_BASE = 'https://crates.io/crates';

function sanitizeText(text) {
    const token = process.env.CARGO_REGISTRY_TOKEN;
    if (!token) return text;
    return text.split(token).join('***');
}

function timestamp() {
    return new Date().toISOString();
}

function timestampShort() {
    return new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
}

function loadPublishOrder() {
    if (!existsSync(PUBLISH_ORDER_PATH)) {
        console.error('❌ publish-order.json 不存在，请先运行 publish-prepare.js');
        process.exit(1);
    }
    return JSON.parse(readFileSync(PUBLISH_ORDER_PATH, 'utf-8'));
}

function cargoLogin(token) {
    console.log('📋 执行 cargo login ...');
    try {
        execSync(`cargo login ${token}`, {
            cwd: PROJECT_ROOT,
            timeout: 30000,
            stdio: 'pipe',
        });
        console.log('✅ cargo login 成功');
        return true;
    } catch (err) {
        const stderr = sanitizeText(err.stderr ? err.stderr.toString() : '');
        console.error('❌ cargo login 失败:', stderr);
        return false;
    }
}

function publishCrate(crateName, dryRun) {
    const cmd = dryRun
        ? `cargo publish --package ${crateName} --dry-run --allow-dirty`
        : `cargo publish --package ${crateName} --allow-dirty`;

    try {
        const stdout = execSync(cmd, {
            cwd: PROJECT_ROOT,
            timeout: 300000,
            stdio: 'pipe',
            encoding: 'utf-8',
        });

        if (stdout.includes('already exists') || stdout.includes('already been published')) {
            return { status: 'already_exists', stdout: sanitizeText(stdout) };
        }

        return { status: 'success', stdout: sanitizeText(stdout) };
    } catch (err) {
        const stderr = sanitizeText(err.stderr ? err.stderr.toString() : '');
        const stdout = sanitizeText(err.stdout ? err.stdout.toString() : '');
        const combined = stderr + stdout;

        if (combined.includes('already exists') || combined.includes('already been published')) {
            return { status: 'already_exists', stdout: combined };
        }

        return { status: 'failed', error: combined.slice(-2000) };
    }
}

function verifyCratePublished(crateName, version) {
    try {
        const stdout = execSync(`cargo search ${crateName}`, {
            cwd: PROJECT_ROOT,
            timeout: 30000,
            stdio: 'pipe',
            encoding: 'utf-8',
        });
        if (stdout.includes(`${crateName} = "${version}"`)) {
            return true;
        }
        return false;
    } catch (err) {
        return false;
    }
}

function writeAuditLog(record, auditLogPath) {
    const line = JSON.stringify(record) + '\n';
    appendFileSync(auditLogPath, line, 'utf-8');
}

function generatePublishSummary(records, summaryPath) {
    const summary = {
        total: records.length,
        success: records.filter(r => r.status === 'success' || r.status === 'verified').length,
        already_exists: records.filter(r => r.status === 'already_exists').length,
        failed: records.filter(r => r.status === 'failed').length,
        generated_at: timestamp(),
        target_version: TARGET_VERSION,
        records: records.map(r => ({
            crate: r.crate_name,
            status: r.status,
            crates_io_url: r.crates_io_url,
        })),
    };
    writeFileSync(summaryPath, JSON.stringify(summary, null, 2), 'utf-8');
    return summary;
}

function main() {
    const args = process.argv.slice(2);
    const dryRun = args.includes('--dry-run');
    const skipIdx = args.indexOf('--skip');
    const skipCrates = skipIdx >= 0 && args[skipIdx + 1]
        ? args[skipIdx + 1].split(',').map(s => s.trim())
        : [];

    const token = process.env.CARGO_REGISTRY_TOKEN;
    if (!token) {
        console.error('❌ 环境变量 CARGO_REGISTRY_TOKEN 未设置');
        console.error('   请设置: $env:CARGO_REGISTRY_TOKEN="your-token"; node publish-execute.js');
        process.exit(20);
    }

    console.log('═══════════════════════════════════════════════════════════════');
    console.log('  sz-rust crates.io 发布执行');
    console.log(`  模式: ${dryRun ? 'dry-run（验证打包）' : '正式发布'}`);
    console.log(`  目标版本: ${TARGET_VERSION}`);
    console.log('═══════════════════════════════════════════════════════════════\n');

    const publishOrder = loadPublishOrder();
    const toPublish = publishOrder.filter(e => !skipCrates.includes(e.crate));
    const skipped = publishOrder.filter(e => skipCrates.includes(e.crate));

    console.log(`  发布顺序: ${toPublish.length} 个 crate`);
    if (skipped.length > 0) {
        console.log(`  跳过: ${skipped.map(e => e.crate).join(', ')}`);
    }
    console.log('');

    if (!dryRun) {
        if (!cargoLogin(token)) {
            process.exit(20);
        }
    }

    const auditLogPath = join(__dirname, `audit-log-${timestampShort()}.jsonl`);
    const records = [];

    console.log('\n📋 开始按拓扑顺序发布...\n');

    for (const entry of toPublish) {
        const crateName = entry.crate;
        const layer = entry.layer;
        const order = entry.publish_order;

        console.log(`  [${order}/${toPublish.length}] L${layer} ${crateName} ...`);

        const result = publishCrate(crateName, dryRun);

        const record = {
            crate_name: crateName,
            version: TARGET_VERSION,
            status: result.status,
            crates_io_url: `${CRATES_IO_BASE}/${crateName}/${TARGET_VERSION}`,
            published_at: timestamp(),
            error_message: result.error || null,
        };

        writeAuditLog(record, auditLogPath);
        records.push(record);

        if (result.status === 'success') {
            console.log(`    ✅ 发布成功 → ${record.crates_io_url}`);
        } else if (result.status === 'already_exists') {
            console.log(`    ⏭️  已存在（跳过） → ${record.crates_io_url}`);
        } else {
            console.error(`    ❌ 发布失败: ${result.error}`);
            console.error(`\n═══ 发布中止：${crateName} 失败 ═══`);
            console.error(`    已成功: ${records.filter(r => r.status === 'success').length}`);
            console.error(`    已存在: ${records.filter(r => r.status === 'already_exists').length}`);
            console.error(`    失败: ${records.filter(r => r.status === 'failed').length}`);
            console.error(`    审计日志: ${auditLogPath}`);

            const summaryPath = join(__dirname, 'publish-summary.json');
            generatePublishSummary(records, summaryPath);
            console.error(`    摘要: ${summaryPath}`);
            process.exit(21);
        }
    }

    console.log('\n📋 发布后验证...');
    if (!dryRun) {
        for (const record of records) {
            if (record.status === 'success') {
                const verified = verifyCratePublished(record.crate_name, TARGET_VERSION);
                if (verified) {
                    record.status = 'verified';
                    console.log(`  ✅ ${record.crate_name} v${TARGET_VERSION} 已验证`);
                } else {
                    console.log(`  ⚠️  ${record.crate_name} v${TARGET_VERSION} 验证未通过（可能需要等待索引更新）`);
                }
            }
        }
    }

    const summaryPath = join(__dirname, 'publish-summary.json');
    const summary = generatePublishSummary(records, summaryPath);

    console.log('\n═══════════════════════════════════════════════════════════════');
    console.log('  📊 发布摘要');
    console.log('───────────────────────────────────────────────────────────────');
    console.log(`  总计: ${summary.total}`);
    console.log(`  成功: ${summary.success}`);
    console.log(`  已存在: ${summary.already_exists}`);
    console.log(`  失败: ${summary.failed}`);
    console.log(`  审计日志: ${auditLogPath}`);
    console.log(`  摘要: ${summaryPath}`);
    console.log('═══════════════════════════════════════════════════════════════');

    if (summary.failed > 0) {
        process.exit(21);
    }
}

main();