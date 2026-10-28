import { Client } from 'ssh2';
import fs from 'fs';
import path from 'path';

const SERVER_HOST = '122.51.216.76';
const SERVER_USER = 'root';
const KEY_PATH = path.resolve(import.meta.dirname, '..', '..', 'deploy_key');
const RUST_PATH = '/root/.cargo/bin';

function createSSHClient() {
    return new Promise((resolve, reject) => {
        const privateKey = fs.readFileSync(KEY_PATH, 'utf-8');
        const client = new Client();

        client.on('ready', () => resolve(client));
        client.on('error', (err) => reject(err));

        client.connect({
            host: SERVER_HOST,
            port: 22,
            username: SERVER_USER,
            privateKey,
            readyTimeout: 30000,
        });
    });
}

function execCommand(client, command, timeout = 15000) {
    return new Promise((resolve, reject) => {
        let stdout = '';
        let stderr = '';
        let settled = false;

        const timer = setTimeout(() => {
            if (!settled) { settled = true; reject(new Error(`Timeout: ${command}`)); }
        }, timeout);

        client.exec(command, (err, stream) => {
            if (err) { clearTimeout(timer); if (!settled) { settled = true; reject(err); } return; }
            stream.on('data', (data) => { stdout += data.toString(); });
            stream.stderr.on('data', (data) => { stderr += data.toString(); });
            stream.on('close', (code) => {
                clearTimeout(timer);
                if (settled) return;
                settled = true;
                resolve({ stdout, stderr, exitCode: code });
            });
        });
    });
}

async function main() {
    console.log('=== 压测前置检查 ===');
    console.log(`服务器: ${SERVER_HOST}`);
    console.log('');

    const checks = [];

    checks.push({
        name: 'deploy_key 存在',
        passed: fs.existsSync(KEY_PATH),
        detail: KEY_PATH,
    });

    if (fs.existsSync(KEY_PATH)) {
        const stat = fs.statSync(KEY_PATH);
        const mode = (stat.mode & 0o777).toString(8);
        checks.push({
            name: 'deploy_key 权限 600',
            passed: mode === '600',
            detail: `当前权限: ${mode}`,
        });
    }

    let client;
    try {
        console.log('[SSH] 连接服务器...');
        client = await createSSHClient();
        checks.push({ name: 'SSH 连接', passed: true, detail: '成功' });
    } catch (err) {
        checks.push({ name: 'SSH 连接', passed: false, detail: err.message });
        printResults(checks);
        process.exit(1);
    }

    const wrkCheck = await execCommand(client, 'wrk --version 2>&1 | head -1');
    checks.push({
        name: 'wrk 4.1.0',
        passed: wrkCheck.stdout.includes('4.1.0'),
        detail: wrkCheck.stdout.trim(),
    });

    const sarCheck = await execCommand(client, 'command -v sar && sar --version 2>&1 | head -1');
    checks.push({
        name: 'sysstat (sar)',
        passed: sarCheck.stdout.includes('sar'),
        detail: sarCheck.stdout.trim() || '未安装 (apt install sysstat)',
    });

    const dstatCheck = await execCommand(client, 'command -v dstat 2>&1');
    checks.push({
        name: 'dstat',
        passed: dstatCheck.exitCode === 0,
        detail: dstatCheck.stdout.trim() || '未安装 (apt install dstat)',
    });

    const rustCheck = await execCommand(client, `export PATH="${RUST_PATH}:$PATH" && rustc --version 2>&1`);
    checks.push({
        name: 'Rust 1.97.1',
        passed: rustCheck.stdout.includes('1.97.1'),
        detail: rustCheck.stdout.trim(),
    });

    const redisCheck = await execCommand(client, 'redis-cli -h 127.0.0.1 -p 6379 ping 2>&1');
    checks.push({
        name: 'Redis 127.0.0.1:6379',
        passed: redisCheck.stdout.trim() === 'PONG',
        detail: redisCheck.stdout.trim(),
    });

    const benchDirCheck = await execCommand(client, `ls -d ${'/www/rust/perf-compare/benchmarks'} 2>&1`);
    checks.push({
        name: '压测目录 /www/rust/perf-compare/benchmarks',
        passed: benchDirCheck.exitCode === 0,
        detail: benchDirCheck.stdout.trim() || '目录不存在',
    });

    client.end();
    printResults(checks);

    const allPassed = checks.every(c => c.passed);
    process.exit(allPassed ? 0 : 1);
}

function printResults(checks) {
    console.log('');
    for (const check of checks) {
        const icon = check.passed ? '✅' : '❌';
        console.log(`${icon} ${check.name}: ${check.detail}`);
    }
    console.log('');
    const passed = checks.filter(c => c.passed).length;
    console.log(`通过: ${passed}/${checks.length}`);
}

main().catch(err => {
    console.error('致命错误:', err.message);
    process.exit(1);
});