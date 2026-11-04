import { SSHOperator } from '../../production-validation/scripts/lib/ssh-operator.js';
import { openTunnel } from './lib/ssh-tunnel.js';
import { spawn } from 'child_process';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, '..', '..', '..', '..');
const BENCH_BIN = path.join(PROJECT_ROOT, 'target', 'debug', 'examples', 'bench-runner.exe');

function runBenchOp(op, conc, total, extra = {}) {
    return new Promise((resolve, reject) => {
        const args = ['--op', op, '--concurrency', String(conc), '--total', String(total), '--redis-url', 'redis://127.0.0.1:16379', '--prefix', 'sso:bench'];
        for (const [k, v] of Object.entries(extra)) { args.push(k, String(v)); }
        const proc = spawn(BENCH_BIN, args, { cwd: PROJECT_ROOT, env: { ...process.env, CARGO_INCREMENTAL: '0', RUST_LOG: 'error' } });
        let stdout = '';
        let stderr = '';
        proc.stdout.on('data', (d) => { stdout += d; });
        proc.stderr.on('data', (d) => { stderr += d; });
        const timer = setTimeout(() => { proc.kill(); reject(new Error('timeout')); }, 120000);
        proc.on('close', (code) => {
            clearTimeout(timer);
            if (code !== 0) { reject(new Error(`exit ${code}: ${stderr}`)); return; }
            try { resolve(JSON.parse(stdout.trim())); } catch { reject(new Error(`parse: ${stdout.slice(0, 200)}`)); }
        });
        proc.on('error', (e) => { clearTimeout(timer); reject(e); });
    });
}

async function quickTest() {
    const config = JSON.parse(fs.readFileSync(path.join(__dirname, 'bench-config.json'), 'utf-8'));
    const keyPath = path.resolve(PROJECT_ROOT, config.server.privateKeyPath);

    const ssh = new SSHOperator({ host: config.server.host, port: config.server.port, username: config.server.username, privateKeyPath: keyPath });
    await ssh.connect();
    const tunnel = await openTunnel({ sshClient: ssh.client, localPort: 16379, remoteHost: '127.0.0.1', remotePort: 6379 });
    console.log('隧道已建立');

    const ops = [
        ['increment_version', 10, 100],
        ['get_version', 10, 100],
        ['is_revoked', 10, 100],
        ['register_session', 10, 100],
        ['get_session', 10, 100],
        ['ttl_validation', 1, 1],
    ];

    for (const [op, conc, total] of ops) {
        try {
            const obj = await runBenchOp(op, conc, total);
            console.log(`${op}: verdict=${obj.verdict}, qps=${obj.qps.toFixed(0)}, p99=${obj.latency_p99_ms.toFixed(2)}ms, consistency=${obj.consistency_check.passed}`);
        } catch (e) {
            console.log(`${op}: FAIL - ${e.message}`);
        }
    }

    await tunnel.close();
    await ssh.close();
    console.log('完成');
}

quickTest().catch(e => { console.error(e); process.exit(1); });
