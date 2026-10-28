import { Client } from 'ssh2';
import fs from 'fs';
import path from 'path';

export const SERVER_HOST = '122.51.216.76';
export const SERVER_USER = 'root';
export const SERVER_PORT = 8300;
export const KEY_PATH = path.resolve(import.meta.dirname, '..', '..', 'deploy_key');
export const CARGO_BIN = '/root/.cargo/bin';

export function createSSHClient() {
    return new Promise((resolve, reject) => {
        const privateKey = fs.readFileSync(KEY_PATH, 'utf-8');
        const client = new Client();
        client.on('ready', () => resolve(client));
        client.on('error', (err) => reject(err));
        client.connect({ host: SERVER_HOST, port: 22, username: SERVER_USER, privateKey, readyTimeout: 30000 });
    });
}

export function execCommand(client, command, timeout = 30000) {
    return new Promise((resolve, reject) => {
        let stdout = '', stderr = '', settled = false;
        const timer = setTimeout(() => {
            if (!settled) { settled = true; reject(new Error(`Timeout ${timeout}ms: ${command}`)); }
        }, timeout);
        client.exec(command, (err, stream) => {
            if (err) { clearTimeout(timer); if (!settled) { settled = true; reject(err); } return; }
            stream.on('data', (d) => { stdout += d.toString(); });
            stream.stderr.on('data', (d) => { stderr += d.toString(); });
            stream.on('close', (code) => {
                clearTimeout(timer);
                if (settled) return;
                settled = true;
                resolve({ stdout, stderr, exitCode: code });
            });
        });
    });
}

export function closeClient(client) {
    return new Promise((resolve) => { client.end(); client.on('close', () => resolve()); setTimeout(resolve, 1000); });
}