import { SSHOperator } from '../../production-validation/scripts/lib/ssh-operator.js';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import { execSync } from 'child_process';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, '..', '..', '..', '..');

async function cleanup() {
    const config = JSON.parse(fs.readFileSync(path.join(__dirname, 'bench-config.json'), 'utf-8'));
    const keyPath = path.resolve(PROJECT_ROOT, config.server.privateKeyPath);

    // 1. SSH 连接清理 Redis key 和远程脚本
    const ssh = new SSHOperator({ host: config.server.host, port: config.server.port, username: config.server.username, privateKeyPath: keyPath });
    await ssh.connect();
    const r1 = await ssh.execCommand("redis-cli --scan --pattern 'sso:bench:*' | xargs -L 100 redis-cli DEL");
    console.log('Redis keys cleaned:', r1.stdout.trim());
    const r2 = await ssh.execCommand('rm -f /tmp/bench_*');
    console.log('Remote scripts cleaned');
    await ssh.close();

    // 2. 终止 bench-runner 进程
    try { execSync('taskkill /F /IM bench-runner.exe', { stdio: 'ignore' }); console.log('Process killed'); } catch { console.log('No process to kill'); }

    // 3. 删除本地二进制
    const binPath = path.join(PROJECT_ROOT, 'target', 'debug', 'examples', 'bench-runner.exe');
    if (fs.existsSync(binPath)) { fs.rmSync(binPath); console.log('Binary deleted'); } else { console.log('No binary to delete'); }

    console.log('清理完成');
}

cleanup().catch(e => { console.error(e); process.exit(1); });