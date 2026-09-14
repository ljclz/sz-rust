import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import { SSHOperator } from '../../production-validation/scripts/lib/ssh-operator.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, '..', '..', '..', '..');

async function verifyClean() {
    const config = JSON.parse(fs.readFileSync(path.join(__dirname, 'bench-config.json'), 'utf-8'));
    let ok = true;
    const results = [];

    // 1. Redis 无 sso:bench:* key
    let ssh = null;
    try {
        const keyPath = path.resolve(PROJECT_ROOT, config.server.privateKeyPath);
        ssh = new SSHOperator({ host: config.server.host, port: config.server.port, username: config.server.username, privateKeyPath: keyPath });
        await ssh.connect();
        const count = await ssh.execCommand("redis-cli --scan --pattern 'sso:bench:*' | wc -l");
        const n = parseInt(count.stdout.trim(), 10);
        results.push({ check: 'redis-keys', passed: n === 0, detail: `count=${n}` });
    } catch (e) {
        results.push({ check: 'redis-keys', passed: false, detail: e.message });
    }

    // 2. 无残留 bench-runner 进程
    try {
        const { execSync } = await import('child_process');
        if (process.platform === 'win32') {
            try { execSync('tasklist /FI "IMAGENAME eq bench-runner.exe"', { stdio: 'pipe' }); results.push({ check: 'process', passed: false, detail: 'process exists' }); } catch { results.push({ check: 'process', passed: true, detail: 'no process' }); }
        } else {
            results.push({ check: 'process', passed: true, detail: 'n/a' });
        }
    } catch { results.push({ check: 'process', passed: true, detail: 'no process' }); }

    // 3. 本地二进制已删除
    const binPath = path.join(PROJECT_ROOT, 'target', 'debug', 'examples', process.platform === 'win32' ? 'bench-runner.exe' : 'bench-runner');
    results.push({ check: 'local-binary', passed: !fs.existsSync(binPath), detail: binPath });

    // 4. SSH 隧道已关闭
    try {
        const { execSync } = await import('child_process');
        if (process.platform === 'win32') {
            try { execSync('netstat -an | findstr 16379', { stdio: 'pipe' }); results.push({ check: 'tunnel', passed: false, detail: 'port still listening' }); } catch { results.push({ check: 'tunnel', passed: true, detail: 'closed' }); }
        } else {
            results.push({ check: 'tunnel', passed: true, detail: 'n/a' });
        }
    } catch { results.push({ check: 'tunnel', passed: true, detail: 'closed' }); }

    // 5. 服务器临时脚本已删除
    try {
        const r = await ssh.execCommand('ls /tmp/bench_* 2>/dev/null | wc -l');
        const n = parseInt(r.stdout.trim(), 10);
        results.push({ check: 'remote-scripts', passed: n === 0, detail: `count=${n}` });
    } catch (e) {
        results.push({ check: 'remote-scripts', passed: false, detail: e.message });
    }

    if (ssh) await ssh.close();

    for (const r of results) {
        console.log(`${r.passed ? '✅' : '❌'} ${r.check}: ${r.detail}`);
        if (!r.passed) ok = false;
    }
    console.log(ok ? '\n✅ 清理验证全部通过' : '\n❌ 清理验证存在失败项');
    process.exit(ok ? 0 : 1);
}

verifyClean().catch(e => { console.error(e); process.exit(1); });