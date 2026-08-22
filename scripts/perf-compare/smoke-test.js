import { Client } from 'ssh2';
import fs from 'fs';
import path from 'path';

const KEY_PATH = path.resolve(import.meta.dirname, '..', '..', 'deploy_key');
const RUST_PATH = '/root/.cargo/bin';
const REMOTE_BENCH = '/www/rust/perf-compare/benchmarks';

function createSSH() {
    return new Promise((resolve, reject) => {
        const client = new Client();
        client.on('ready', () => resolve(client));
        client.on('error', reject);
        client.connect({
            host: '122.51.216.76', port: 22, username: 'root',
            privateKey: fs.readFileSync(KEY_PATH, 'utf-8'),
            readyTimeout: 30000,
        });
    });
}

function exec(client, cmd, timeout = 30000) {
    return new Promise((resolve, reject) => {
        let stdout = '', stderr = '';
        const timer = setTimeout(() => reject(new Error('Timeout: ' + cmd)), timeout);
        client.exec(cmd, (err, stream) => {
            if (err) { clearTimeout(timer); reject(err); return; }
            stream.on('data', d => stdout += d);
            stream.stderr.on('data', d => stderr += d);
            stream.on('close', code => { clearTimeout(timer); resolve({ stdout, stderr, exitCode: code }); });
        });
    });
}

function execBackground(client, cmd) {
    return new Promise((resolve, reject) => {
        client.exec(cmd, (err, stream) => {
            if (err) { reject(err); return; }
            stream.on('close', () => resolve());
            setTimeout(() => resolve(), 1000);
        });
    });
}

async function main() {
    console.log('=== 冒烟测试: sz-rust x /simple x 32 ===');
    const client = await createSSH();

    console.log('[1] 编译 sz-rust...');
    const build = await exec(client, `cd ${REMOTE_BENCH}/sz-rust && export PATH="${RUST_PATH}:$PATH" && cargo build --release 2>&1 | tail -3`, 300000);
    console.log('    exitCode:', build.exitCode);

    console.log('[2] 清理端口 8401...');
    await exec(client, 'fuser -k 8401/tcp 2>/dev/null; sleep 1');

    console.log('[3] 启动 sz-rust...');
    await execBackground(client, `cd ${REMOTE_BENCH}/sz-rust && export PATH="${RUST_PATH}:$PATH" && setsid env PORT=8401 ./target/release/bench-sz-rust > /tmp/bench-sz-rust.log 2>&1 < /dev/null &`);
    await new Promise(r => setTimeout(r, 3000));

    console.log('[4] 健康检查 /simple...');
    const health = await exec(client, 'curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8401/simple');
    console.log('    HTTP', health.stdout.trim());

    console.log('[5] 健康检查 /db...');
    const dbHealth = await exec(client, 'curl -s -w "\\nHTTP %{http_code}" http://127.0.0.1:8401/db');
    console.log('    Response:', dbHealth.stdout.trim());

    console.log('[6] wrk 压测 /simple c=32 d=10s...');
    const wrk = await exec(client, 'wrk -t32 -c32 -d10s --latency http://127.0.0.1:8401/simple 2>&1', 30000);
    console.log(wrk.stdout);

    await exec(client, 'fuser -k 8401/tcp 2>/dev/null');
    console.log('[7] 清理完成');

    client.end();
    console.log('=== 冒烟测试完成 ===');
}

main().catch(err => { console.error('Error:', err.message); process.exit(1); });