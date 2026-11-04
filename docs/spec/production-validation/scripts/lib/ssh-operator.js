import { Client } from 'ssh2';
import fs from 'fs';
import path from 'path';

export class SSHOperator {
    constructor({ host, port, username, privateKeyPath }) {
        this.host = host;
        this.port = port || 22;
        this.username = username;
        this.privateKeyPath = privateKeyPath;
        this.client = null;
        this.sftp = null;
        this.connected = false;
    }

    async connect() {
        if (this.connected) return;

        const privateKey = fs.readFileSync(this.privateKeyPath, 'utf-8');

        return new Promise((resolve, reject) => {
            const client = new Client();

            client.on('ready', () => {
                this.client = client;
                this.connected = true;
                resolve();
            });

            client.on('error', (err) => {
                if (err.level === 'authentication') {
                    reject(new SSHAuthFailedError(this.privateKeyPath));
                } else if (err.level === 'connect-timeout') {
                    reject(new SSHConnectionTimeoutError(this.host, this.port));
                } else {
                    reject(err);
                }
            });

            client.connect({
                host: this.host,
                port: this.port,
                username: this.username,
                privateKey,
                readyTimeout: 30000,
            });
        });
    }

    async execCommand(command, options = {}) {
        await this.connect();

        const timeout = options.timeout || 30000;

        return new Promise((resolve, reject) => {
            let stdout = '';
            let stderr = '';
            let exitCode = null;
            let settled = false;

            const timer = setTimeout(() => {
                if (!settled) {
                    settled = true;
                    reject(new SSHConnectionTimeoutError(this.host, this.port));
                }
            }, timeout);

            this.client.exec(command, (err, stream) => {
                if (err) {
                    clearTimeout(timer);
                    if (!settled) {
                        settled = true;
                        reject(err);
                    }
                    return;
                }

                stream.on('data', (data) => { stdout += data.toString(); });
                stream.stderr.on('data', (data) => { stderr += data.toString(); });

                stream.on('close', (code) => {
                    clearTimeout(timer);
                    if (settled) return;
                    settled = true;
                    exitCode = code;

                    if (code !== 0 && options.expectNonZero !== true) {
                        reject(new ExecNonZeroExitError(command, stderr, code));
                    } else {
                        resolve({ stdout, stderr, exitCode: code });
                    }
                });
            });
        });
    }

    async uploadFile(localPath, remotePath) {
        await this.connect();

        return new Promise((resolve, reject) => {
            this.client.sftp((err, sftp) => {
                if (err) { reject(err); return; }
                this.sftp = sftp;

                const remoteDir = path.posix.dirname(remotePath);
                sftp.stat(remoteDir, (statErr) => {
                    if (statErr) {
                        reject(new Error(`远程目录不存在: ${remoteDir}`));
                        return;
                    }

                    sftp.fastPut(localPath, remotePath, (putErr) => {
                        if (putErr) { reject(putErr); return; }
                        resolve();
                    });
                });
            });
        });
    }

    async downloadFile(remotePath, localPath) {
        await this.connect();

        return new Promise((resolve, reject) => {
            this.client.sftp((err, sftp) => {
                if (err) { reject(err); return; }
                this.sftp = sftp;

                sftp.fastGet(remotePath, localPath, (getErr) => {
                    if (getErr) { reject(getErr); return; }
                    resolve();
                });
            });
        });
    }

    async close() {
        if (this.sftp) {
            this.sftp.end();
            this.sftp = null;
        }
        if (this.client) {
            this.client.end();
            this.client = null;
        }
        this.connected = false;
    }
}

export class SSHAuthFailedError extends Error {
    constructor(keyPath) {
        super(`SSH 认证失败，密钥路径: ${keyPath}`);
        this.name = 'SSH_AUTH_FAILED';
    }
}

export class SSHConnectionTimeoutError extends Error {
    constructor(host, port) {
        super(`SSH 连接超时: ${host}:${port}`);
        this.name = 'SSH_CONNECTION_TIMEOUT';
    }
}

export class ExecNonZeroExitError extends Error {
    constructor(command, stderr, exitCode) {
        super(`命令退出码非零 (${exitCode}): ${command}\nstderr: ${stderr}`);
        this.name = 'EXEC_NONZERO_EXIT';
        this.command = command;
        this.stderr = stderr;
        this.exitCode = exitCode;
    }
}