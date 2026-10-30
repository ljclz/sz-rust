import { SSHOperator } from '../../production-validation/scripts/lib/ssh-operator.js';
import { openTunnel } from './lib/ssh-tunnel.js';
import net from 'net';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, '..', '..', '..', '..');

async function testTunnel() {
    const config = JSON.parse(fs.readFileSync(path.join(__dirname, 'bench-config.json'), 'utf-8'));
    const keyPath = path.resolve(PROJECT_ROOT, config.server.privateKeyPath);

    const ssh = new SSHOperator({ host: config.server.host, port: config.server.port, username: config.server.username, privateKeyPath: keyPath });
    await ssh.connect();
    console.log('SSH connected');

    const tunnel = await openTunnel({ sshClient: ssh.client, localPort: 16379, remoteHost: '127.0.0.1', remotePort: 6379 });
    console.log('Tunnel opened');

    // 用 net 模块测试 Redis PING
    return new Promise((resolve) => {
        const socket = net.connect(16379, '127.0.0.1', () => {
            console.log('Connected to local 16379');
            socket.write('PING\r\n');
        });
        socket.on('data', (data) => {
            console.log('Redis response:', data.toString().trim());
            socket.destroy();
        });
        socket.on('error', (err) => {
            console.log('Socket error:', err.message);
        });
        socket.on('close', async () => {
            console.log('Socket closed');
            await tunnel.close();
            await ssh.close();
            resolve();
        });
    });
}

testTunnel().catch(e => { console.error(e); process.exit(1); });