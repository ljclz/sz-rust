import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import { execSync, spawn } from 'child_process';
import { SSHOperator } from '../../production-validation/scripts/lib/ssh-operator.js';
import { openTunnel } from './lib/ssh-tunnel.js';
import { generateBenchReport, judgeGoNoGo } from './lib/bench-report-generator.js';
import { cleanBench } from './lib/bench-cleaner.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, '..', '..', '..', '..');
const BENCH_BIN = path.join(PROJECT_ROOT, 'target', 'debug', 'examples', process.platform === 'win32' ? 'bench-runner.exe' : 'bench-runner');

/**
 * 主入口：运行全量压测
 */
export async function runBench(configPath) {
    const config = JSON.parse(fs.readFileSync(configPath, 'utf-8'));
    const reportPath = path.resolve(path.dirname(configPath), '..', 'benchmark-report.md');

    let ssh = null;
    let tunnel = null;
    let roundResults = [];
    let soakResult = null;
    let poolResult = null;
    let cleanResult = null;

    try {
        // 1. SSH 连接
        console.log('[1/4] 连接 SSH...');
        const keyPath = path.resolve(PROJECT_ROOT, config.server.privateKeyPath);
        ssh = new SSHOperator({ host: config.server.host, port: config.server.port, username: config.server.username, privateKeyPath: keyPath });
        await ssh.connect();

        // 2. 建立 SSH 隧道
        console.log('[2/4] 建立 SSH 隧道 (16379 → 6379)...');
        tunnel = await openTunnel({ sshClient: ssh.client, localPort: 16379, remoteHost: '127.0.0.1', remotePort: 6379 });

        // 3. 编译 bench-runner
        console.log('[3/4] 编译 bench-runner...');
        execSync('cargo build --example bench-runner --features redis-store', {
            cwd: PROJECT_ROOT,
            env: { ...process.env, CARGO_INCREMENTAL: '0' },
            stdio: 'pipe',
            timeout: 300000,
        });

        // 4. 编排 15 轮压测
        console.log('[4/4] 执行 15 轮压测...');
        const results = await runAllRounds(config, tunnel);
        roundResults = results.roundResults;
        soakResult = results.soakResult;
        poolResult = results.poolResult;

        // 生成报告
        console.log('生成报告...');
        cleanResult = { cleaned: [], failed: [] };
        const { overallPassed, blockers } = await generateBenchReport({
            roundResults, soakResult, poolResult, cleanResult, config, projectRoot: PROJECT_ROOT, reportPath,
        });

        console.log(overallPassed ? '\n✅ 可上生产 — 所有红线达标' : `\n❌ 阻断 — ${blockers.length} 个阻断项`);
        return { overallPassed, blockers };
    } catch (err) {
        console.error('压测失败:', err.message);
        return { overallPassed: false, blockers: [{ operation: 'orchestrator', concurrency: 0, redLineId: 'FATAL', actual: err.message, threshold: 'no error' }] };
    } finally {
        // 清理
        console.log('清理...');
        cleanResult = await cleanBench({ ssh, tunnel, binaryPath: BENCH_BIN, redisKeyPattern: 'sso:bench:*' });
        if (ssh) await ssh.close();
    }
}

/**
 * 编排 15 轮压测
 */
async function runAllRounds(config, tunnel) {
    const roundResults = [];
    let soakResult = null;
    let poolResult = null;
    let roundNum = 0;

    const runRound = async (op, concurrency, total, extra = {}) => {
        roundNum++;
        const args = ['--op', op, '--concurrency', String(concurrency), '--total', String(total), '--redis-url', config.redisUrl, '--prefix', config.keyPrefix];
        for (const [k, v] of Object.entries(extra)) { args.push(k, String(v)); }

        console.log(`  轮次 ${roundNum}/15: ${op} (并发=${concurrency}, 总数=${total})`);
        try {
            const stdout = await new Promise((resolve, reject) => {
                const proc = spawn(BENCH_BIN, args, { cwd: PROJECT_ROOT, env: { ...process.env, CARGO_INCREMENTAL: '0', RUST_LOG: 'error' } });
                let out = '';
                let err = '';
                proc.stdout.on('data', (d) => { out += d; });
                proc.stderr.on('data', (d) => { err += d; });
                const timer = setTimeout(() => { proc.kill(); reject(new Error('timeout')); }, 300000);
                proc.on('close', (code) => { clearTimeout(timer); if (code !== 0) { reject(new Error(`exit ${code}: ${err.slice(0, 200)}`)); } else { resolve(out); } });
                proc.on('error', (e) => { clearTimeout(timer); reject(e); });
            });
            const lines = stdout.toString().trim().split('\n').filter(l => l.trim());

            if (op === 'soak') {
                const snapshots = [];
                for (const line of lines) {
                    try {
                        const obj = JSON.parse(line);
                        if (obj.minute_index !== undefined) { snapshots.push(obj); }
                        else if (obj.qps_stable !== undefined) { soakResult = obj; }
                    } catch { }
                }
                if (soakResult && snapshots.length > 0) { soakResult.snapshots = snapshots; }
            } else {
                for (const line of lines) {
                    try {
                        const obj = JSON.parse(line);
                        if (obj.operation) { roundResults.push(obj); if (obj.service_unavailable_rate !== undefined) { poolResult = obj; } }
                    } catch { }
                }
            }
        } catch (e) {
            console.error(`  轮次 ${roundNum} 失败: ${e.message}`);
            roundResults.push({ operation: op, concurrency, qps: 0, latency_p50_ms: 0, latency_p95_ms: 0, latency_p99_ms: 0, error_rate: 1, error_breakdown: {}, total_requests: total, duration_secs: 0, rss_peak_kb: 0, rss_start_kb: 0, evidence_file: '', evidence_line: '', verdict: 'fail', consistency_check: { passed: false, detail: e.message } });
        }
    };

    // 11 轮单操作（PERF-1 ~ PERF-11）— SSH 隧道环境，缩小规模验证功能
    await runRound('increment_version', 10, 1000);
    await runRound('increment_version', 50, 5000);
    await runRound('increment_version', 100, 10000);
    await runRound('get_version', 100, 10000);
    await runRound('is_revoked', 100, 10000);
    await runRound('revoke', 50, 5000);
    await runRound('register_session', 50, 5000);
    await runRound('get_session', 100, 10000);
    await runRound('get_sessions', 50, 5000, { '--devices-per-user': 10 });
    await runRound('revoke_session', 50, 5000);
    await runRound('update_last_active', 50, 5000);

    // 混合负载
    await runRound('mixed', 50, 10000, { '--mixed-ratio': config.mixedRatio });

    // 连接池稳定性（1 分钟）
    await runRound('pool_stability', 100, 0, { '--soak-secs': 60 });

    // Soak（2 分钟）
    await runRound('soak', 50, 0, { '--soak-secs': 120, '--mixed-ratio': config.mixedRatio });

    // 无死锁
    await runRound('shared_pool', 100, 5000);

    return { roundResults, soakResult, poolResult };
}

// CLI 入口
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
    const configPath = process.argv[2] || path.join(__dirname, 'bench-config.json');
    runBench(configPath).then(result => {
        process.exit(result.overallPassed ? 0 : 1);
    }).catch(err => {
        console.error(err);
        process.exit(1);
    });
}