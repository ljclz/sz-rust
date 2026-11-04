import { createSSHClient, execCommand, closeClient } from './_ssh.js';
import { fileURLToPath } from 'url';

// 通过 ssh2 连接远程服务器，使用已有的 ab (Apache Benchmark) 作为压测工具
const HEY_BIN = '/usr/bin/ab';

export async function installHey() {
    const client = await createSSHClient();
    try {
        const { stdout } = await execCommand(client, `${HEY_BIN} -V 2>&1 | head -1`, 10000);
        if (!stdout.trim()) throw new Error('ab 未安装');
        return { success: true, version: stdout.trim(), path: HEY_BIN };
    } finally {
        await closeClient(client);
    }
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isMain) {
    if (process.argv.includes('--dry-run')) {
        console.log(JSON.stringify({ success: true, version: 'dry-run', path: HEY_BIN }, null, 2));
        process.exit(0);
    }
    installHey().then(r => { console.log(JSON.stringify(r, null, 2)); process.exit(0); })
        .catch(e => { console.error(e.message); process.exit(1); });
}
