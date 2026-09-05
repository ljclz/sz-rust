import fs from 'fs';
import path from 'path';
import { SSHOperator } from './lib/ssh-operator.js';
import { LocalBuilder } from './lib/local-builder.js';
import { EvidenceCollector } from './lib/evidence-collector.js';
import { validateMySQL } from './validators/mysql-validator.js';
import { validatePostgreSQL } from './validators/postgresql-validator.js';
import { validateRedis } from './validators/redis-validator.js';
import { validateMQTT } from './validators/mqtt-validator.js';
import { validateDeploy } from './validators/deploy-validator.js';
import { validateE2E } from './validators/e2e-validator.js';
import { cleanAll } from './lib/cleaner.js';
import { generateReport } from './lib/report-generator.js';

const projectRoot = path.resolve(import.meta.dirname, '..', '..', '..', '..');

async function runValidation(configPath) {
    const config = JSON.parse(fs.readFileSync(configPath, 'utf-8'));
    const startedAt = Date.now();

    console.log('=== P0-1 服务器真实数据全链路验证 ===');
    console.log(`服务器: ${config.server.host}, sz-rust v${config.szRustVersion}`);
    console.log('');

    const ssh = new SSHOperator({
        host: config.server.host,
        port: config.server.port,
        username: config.server.username,
        privateKeyPath: config.server.privateKeyPath,
    });

    const moduleResults = [];
    let buildResults = null;
    let deployResult = null;

    try {
        console.log('[M1] 基础设施层 — 编译');
        const builder = new LocalBuilder(projectRoot);

        const skipBuild = process.env.SKIP_BUILD === '1';
        if (skipBuild) {
            console.log('[编译] 跳过编译 (SKIP_BUILD=1)');
            buildResults = config.applications.map(app => ({
                name: app.name,
                binaryPath: app.localBinaryPath,
                size: fs.existsSync(app.localBinaryPath) ? fs.statSync(app.localBinaryPath).size : 0,
            }));
        } else {
            buildResults = await builder.buildAll(config.applications);
        }

        const upstreamCheck = await builder.verifyNoUpstreamChanges();
        if (upstreamCheck.changed) {
            console.error('[警告] sz-orm 仓库有变更:', upstreamCheck.message);
        }

        console.log('');
        console.log('[M2] 验证模块层');

        console.log('[验证] MySQL...');
        const mysqlResult = await validateMySQL(ssh, config, projectRoot);
        moduleResults.push(mysqlResult);
        console.log(`  ${mysqlResult.passed ? '✅' : '❌'} MySQL ${mysqlResult.duration}ms`);

        console.log('[验证] PostgreSQL...');
        const pgResult = await validatePostgreSQL(ssh, config, projectRoot);
        moduleResults.push(pgResult);
        console.log(`  ${pgResult.passed ? '✅' : '❌'} PostgreSQL ${pgResult.duration}ms`);

        console.log('[验证] Redis...');
        const redisResult = await validateRedis(ssh, config, projectRoot);
        moduleResults.push(redisResult);
        console.log(`  ${redisResult.passed ? '✅' : '❌'} Redis ${redisResult.duration}ms`);

        console.log('[验证] MQTT...');
        const mqttResult = await validateMQTT(ssh, config, projectRoot);
        moduleResults.push(mqttResult);
        console.log(`  ${mqttResult.passed ? '✅' : '❌'} MQTT ${mqttResult.duration}ms`);

        console.log('[验证] 部署...');
        deployResult = await validateDeploy(ssh, config, buildResults, projectRoot);
        moduleResults.push(deployResult);
        console.log(`  ${deployResult.passed ? '✅' : '❌'} Deploy ${deployResult.duration}ms`);

        console.log('[验证] 全链路 E2E...');
        const e2eResult = await validateE2E(ssh, config, deployResult, projectRoot);
        moduleResults.push(e2eResult);
        console.log(`  ${e2eResult.passed ? '✅' : '❌'} E2E ${e2eResult.duration}ms`);

        console.log('');
        console.log('[M3] 编排与清理层');

        console.log('[清理] 验证产物...');
        const cleanResult = await cleanAll(ssh, config);
        console.log(`  ${cleanResult.passed ? '✅' : '❌'} Cleaner ${cleanResult.duration}ms`);

        console.log('[报告] 生成验证报告...');
        const reportPath = path.resolve(import.meta.dirname, '..', 'validation-report.md');
        const reportResult = await generateReport(moduleResults, cleanResult, config, projectRoot, reportPath);
        console.log(`  报告已生成: ${reportPath}`);
        console.log(`  整体结论: ${reportResult.overallPassed ? '✅ 可上生产' : '❌ 不可上生产'}`);

        console.log('');
        console.log(`=== 验证完成，总耗时: ${Date.now() - startedAt}ms ===`);

        return {
            overallPassed: reportResult.overallPassed,
            moduleResults,
            cleanResult,
            reportPath,
        };

    } catch (err) {
        console.error('[错误]', err.name, err.message);

        console.log('[清理] 异常路径清理...');
        try {
            const cleanResult = await cleanAll(ssh, config);
            console.log('  清理完成');
        } catch (cleanErr) {
            console.error('  清理失败:', cleanErr.message);
        }

        throw err;
    } finally {
        await ssh.close();
    }
}

const configPath = process.argv.find(arg => arg.startsWith('--config='))?.split('=')[1]
    || process.argv[process.argv.indexOf('--config') + 1]
    || path.resolve(import.meta.dirname, '..', 'validation-config.json');

runValidation(configPath).catch(err => {
    console.error('验证失败:', err.message);
    process.exit(1);
});