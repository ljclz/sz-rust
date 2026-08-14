import fs from 'fs';
import path from 'path';

/**
 * 5 步清理（幂等，失败不中断）
 */
export async function cleanBench({ ssh, tunnel, binaryPath, redisKeyPattern }) {
    const cleaned = [];
    const failed = [];

    // 步骤 1: 远程 Redis key 清理
    try {
        await ssh.execCommand(`redis-cli --scan --pattern '${redisKeyPattern}' | xargs -L 100 redis-cli DEL`);
        cleaned.push({ artifact: 'redis-keys', status: 'deleted' });
    } catch (e) {
        failed.push({ artifact: 'redis-keys', reason: e.message });
    }

    // 步骤 2: 终止 Rust 压测进程
    try {
        if (process.platform === 'win32') {
            try { require('child_process').execSync('taskkill /F /IM bench-runner.exe', { stdio: 'ignore' }); } catch { }
        } else {
            try { require('child_process').execSync('pkill -f bench-runner', { stdio: 'ignore' }); } catch { }
        }
        cleaned.push({ artifact: 'bench-process', status: 'killed' });
    } catch (e) {
        failed.push({ artifact: 'bench-process', reason: e.message });
    }

    // 步骤 3: 删除本地临时二进制
    try {
        if (fs.existsSync(binaryPath)) {
            fs.rmSync(binaryPath);
        }
        cleaned.push({ artifact: 'local-binary', status: 'deleted' });
    } catch (e) {
        failed.push({ artifact: 'local-binary', reason: e.message });
    }

    // 步骤 4: 关闭 SSH 隧道
    try {
        if (tunnel) {
            await tunnel.close();
        }
        cleaned.push({ artifact: 'ssh-tunnel', status: 'closed' });
    } catch (e) {
        failed.push({ artifact: 'ssh-tunnel', reason: e.message });
    }

    // 步骤 5: 删除服务器临时脚本
    try {
        await ssh.execCommand('rm -f /tmp/bench_*');
        cleaned.push({ artifact: 'remote-scripts', status: 'deleted' });
    } catch (e) {
        failed.push({ artifact: 'remote-scripts', reason: e.message });
    }

    return { cleaned, failed };
}