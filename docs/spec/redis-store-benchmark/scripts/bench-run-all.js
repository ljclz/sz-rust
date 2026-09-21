import { SSHOperator } from '../../production-validation/scripts/lib/ssh-operator.js';
import { openTunnel } from './lib/ssh-tunnel.js';
import { spawn } from 'child_process';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import { generateBenchReport } from './lib/bench-report-generator.js';
import { cleanBench } from './lib/bench-cleaner.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, '..', '..', '..', '..');
const BENCH_BIN = path.join(PROJECT_ROOT, 'target', 'debug', 'examples', 'bench-runner.exe');

function runBenchOp(op, conc, total, extra = {}) {
    return new Promise((resolve) => {
        const args = ['--op', op, '--concurrency', String(conc), '--total', String(total), '--redis-url', 'redis://127.0.0.1:16379', '--prefix', 'sso:bench'];
        for (const [k, v] of Object.entries(extra)) { args.push(k, String(v)); }
        const proc = spawn(BENCH_BIN, args, { cwd: PROJECT_ROOT, env: { ...process.env, CARGO_INCREMENTAL: '0', RUST_LOG: 'error' } });
        let stdout = '';
        let stderr = '';
        proc.stdout.on('data', (d) => { stdout += d; });
        proc.stderr.on('data', (d) => { stderr += d; });
        const timer = setTimeout(() => { proc.kill(); resolve({ error: 'timeout', stdout, stderr }); }, 120000);
        proc.on('close', (code) => { clearTimeout(timer); resolve({ code, stdout, stderr }); });
        proc.on('error', (e) => { clearTimeout(timer); resolve({ error: e.message, stdout, stderr }); });
    });
}

function parseResult(op, stdout) {
    const lines = stdout.toString().trim().split('\n').filter(l => l.trim());
    if (op === 'soak') {
        const snapshots = [];
        let summary = null;
        for (const line of lines) {
            try {
                const obj = JSON.parse(line);
                if (obj.minute_index !== undefined) { snapshots.push(obj); }
                else if (obj.qps_stable !== undefined) { summary = obj; }
            } catch { }
        }
        if (summary && snapshots.length > 0) { summary.snapshots = snapshots; }
        return summary;
    }
    for (const line of lines) {
        try {
            const obj = JSON.parse(line);
            if (obj.operation) { return obj; }
        } catch { }
    }
    return null;
}

async function main() {
    const config = JSON.parse(fs.readFileSync(path.join(__dirname, 'bench-config.json'), 'utf-8'));
    const keyPath = path.resolve(PROJECT_ROOT, config.server.privateKeyPath);
    const reportPath = path.resolve(__dirname, '..', 'benchmark-report.md');

    const ssh = new SSHOperator({ host: config.server.host, port: config.server.port, username: config.server.username, privateKeyPath: keyPath });
    await ssh.connect();
    const tunnel = await openTunnel({ sshClient: ssh.client, localPort: 16379, remoteHost: '127.0.0.1', remotePort: 6379 });
    console.log('隧道已建立');

    const roundResults = [];
    let soakResult = null;
    let poolResult = null;

    const rounds = [
        ['increment_version', 10, 1000],
        ['get_version', 10, 1000],
        ['is_revoked', 10, 1000],
        ['revoke', 10, 1000],
        ['register_session', 10, 1000],
        ['get_session', 10, 1000],
        ['get_sessions', 10, 1000, { '--devices-per-user': 10 }],
        ['revoke_session', 10, 1000],
        ['update_last_active', 10, 1000],
        ['ttl_validation', 1, 1],
        ['concurrent_increment', 100, 1000],
        ['mixed', 50, 5000, { '--mixed-ratio': '3:2:2:1:1:1' }],
        ['shared_pool', 50, 3000],
    ];

    for (let i = 0; i < rounds.length; i++) {
        const [op, conc, total, extra] = rounds[i];
        process.stdout.write(`[${i + 1}/${rounds.length}] ${op}...`);
        const result = await runBenchOp(op, conc, total, extra || {});
        if (result.error) {
            console.log(` FAIL (${result.error})`);
            roundResults.push({ operation: op, concurrency: conc, qps: 0, latency_p50_ms: 0, latency_p95_ms: 0, latency_p99_ms: 0, error_rate: 1, error_breakdown: {}, total_requests: total, duration_secs: 0, rss_peak_kb: 0, rss_start_kb: 0, evidence_file: '', evidence_line: '', verdict: 'fail', consistency_check: { passed: false, detail: result.error } });
        } else {
            const parsed = parseResult(op, result.stdout);
            if (parsed) {
                if (op === 'soak') { soakResult = parsed; }
                else { roundResults.push(parsed); if (parsed.service_unavailable_rate !== undefined) { poolResult = parsed; } }
                console.log(` ${parsed.verdict} (qps=${parsed.qps?.toFixed(0) || 0}, p99=${parsed.latency_p99_ms?.toFixed(2) || 0}ms, cc=${parsed.consistency_check?.passed})`);
            } else {
                console.log(` parse-fail`);
            }
        }
    }

    await tunnel.close();
    await ssh.close();

    const cleanResult = await cleanBench({ ssh: null, tunnel: null, binaryPath: BENCH_BIN, redisKeyPattern: 'sso:bench:*' });

    const { overallPassed, blockers } = await generateBenchReport({
        roundResults, soakResult, poolResult, cleanResult, config, projectRoot: PROJECT_ROOT, reportPath,
    });

    console.log(overallPassed ? '\n✅ 可上生产' : `\n❌ 阻断 — ${blockers.length} 项`);
    console.log(`报告已生成: ${reportPath}`);
}

main().catch(e => { console.error(e); process.exit(1); });