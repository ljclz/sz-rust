import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import { execSync } from 'child_process';
import { SSHOperator } from '../../production-validation/scripts/lib/ssh-operator.js';
import { openTunnel } from './lib/ssh-tunnel.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, '..', '..', '..', '..');

async function precheck() {
    const config = JSON.parse(fs.readFileSync(path.join(__dirname, 'bench-config.json'), 'utf-8'));
    let ok = true;

    // 1. 验证 deploy_key
    const keyPath = path.resolve(PROJECT_ROOT, config.server.privateKeyPath);
    try {
        fs.accessSync(keyPath, fs.constants.R_OK);
        console.log('SSH Key: OK');
    } catch {
        console.log('SSH Key: FAIL -', keyPath, 'not readable');
        ok = false;
    }

    // 2. SSH 连接
    let ssh = null;
    try {
        ssh = new SSHOperator({ host: config.server.host, port: config.server.port, username: config.server.username, privateKeyPath: keyPath });
        await ssh.connect();
        console.log('SSH: OK');
    } catch (e) {
        console.log('SSH: FAIL -', e.message);
        ok = false;
        if (ssh) await ssh.close();
        process.exit(1);
    }

    // 3. SSH 隧道
    let tunnel = null;
    try {
        tunnel = await openTunnel({ sshClient: ssh.client, localPort: 16379, remoteHost: '127.0.0.1', remotePort: 6379 });
        console.log('Tunnel: OK');
    } catch (e) {
        console.log('Tunnel: FAIL -', e.message);
        ok = false;
    }

    // 4. Redis PING (通过 SSH 在服务器上执行)
    try {
        const result = await ssh.execCommand('redis-cli PING');
        const pong = result.stdout.trim();
        console.log('Redis PING:', pong === 'PONG' ? 'OK' : 'FAIL');
        if (pong !== 'PONG') ok = false;
    } catch {
        console.log('Redis PING: FAIL');
        ok = false;
    }

    // 5. 编译
    try {
        execSync('cargo build --example bench-runner --features redis-store', {
            cwd: PROJECT_ROOT,
            env: { ...process.env, CARGO_INCREMENTAL: '0' },
            stdio: 'pipe',
            timeout: 300000,
        });
        console.log('Build: OK');
    } catch {
        console.log('Build: FAIL');
        ok = false;
    }

    // 清理
    if (tunnel) await tunnel.close();
    await ssh.close();

    console.log(ok ? '\n✅ 预检查全部通过' : '\n❌ 预检查存在失败项');
    process.exit(ok ? 0 : 1);
}

precheck().catch(e => { console.error(e); process.exit(1); });