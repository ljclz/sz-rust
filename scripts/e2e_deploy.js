#!/usr/bin/env node
/**
 * sz-rust E2E 部署脚本（W5/W6 E2E-004/005/006）
 *
 * 使用 ssh2 连接服务器，SFTP 上传产物，启动进程，健康检查，精确释放。
 * 禁止 sshpass，禁止 killall，使用 fuser -k <端口>/tcp。
 *
 * 用法：
 *   node scripts/e2e_deploy.js \
 *     --host 192.168.1.100 --port 22 --user deploy \
 *     --keyFile ~/.ssh/id_rsa \
 *     --localPath target/release/sz300-server.exe \
 *     --remotePath /www/rust/sz300/sz300-server \
 *     --appPort 8080
 */

const fs = require('fs');
const path = require('path');
const { Client } = require('ssh2');

function parseArgs(argv) {
    const args = {};
    for (let i = 2; i < argv.length; i += 2) {
        const key = argv[i].replace(/^--/, '');
        args[key] = argv[i + 1];
    }
    return args;
}

function log(msg) {
    console.log(`[e2e_deploy] ${new Date().toISOString()} ${msg}`);
}

async function deploy(args) {
    const {
        host = '127.0.0.1',
        port = '22',
        user = 'deploy',
        keyFile,
        localPath,
        remotePath = '/www/rust/sz300/sz300-server',
        appPort = '8080',
    } = args;

    if (!keyFile) throw new Error('--keyFile 必填');
    if (!localPath) throw new Error('--localPath 必填');

    const privateKey = fs.readFileSync(path.resolve(keyFile), 'utf-8');
    log(`读取私钥: ${keyFile}（内容不输出）`);

    return new Promise((resolve, reject) => {
        const conn = new Client();

        conn.on('ready', () => {
            log(`SSH 连接成功: ${user}@${host}:${port}`);

            conn.sftp((err, sftp) => {
                if (err) {
                    conn.end();
                    reject(new Error(`SFTP 失败: ${err.message}`));
                    return;
                }

                log(`SFTP 上传: ${localPath} -> ${remotePath}`);
                sftp.fastPut(localPath, remotePath, (putErr) => {
                    if (putErr) {
                        conn.end();
                        reject(new Error(`上传失败: ${putErr.message}`));
                        return;
                    }
                    log('SFTP 上传完成');

                    conn.exec(`chmod +x ${remotePath}`, (chmodErr) => {
                        if (chmodErr) log(`chmod 警告: ${chmodErr.message}`);

                        conn.exec(`fuser -k ${appPort}/tcp 2>/dev/null; sleep 1`, (killErr) => {
                            if (killErr) log(`fuser 警告: ${killErr.message}`);
                            log(`精确释放端口 ${appPort}（fuser -k ${appPort}/tcp）`);

                            conn.exec(`nohup ${remotePath} > /tmp/sz300.log 2>&1 &`, (startErr) => {
                                if (startErr) {
                                    conn.end();
                                    reject(new Error(`启动失败: ${startErr.message}`));
                                    return;
                                }
                                log(`sz300 启动命令已发送`);

                                setTimeout(() => {
                                    conn.exec(`curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:${appPort}/health`, (healthErr, stream) => {
                                        if (healthErr) {
                                            conn.end();
                                            reject(new Error(`健康检查失败: ${healthErr.message}`));
                                            return;
                                        }
                                        let healthCode = '';
                                        stream.on('data', (d) => { healthCode += d.toString(); });
                                        stream.on('close', () => {
                                            log(`健康检查 HTTP 状态码: ${healthCode}`);
                                            conn.end();
                                            resolve({
                                                uploaded: true,
                                                started: true,
                                                healthCheck: healthCode,
                                                port: appPort,
                                            });
                                        });
                                    });
                                }, 3000);
                            });
                        });
                    });
                });
            });
        });

        conn.on('error', (err) => {
            reject(new Error(`SSH 错误: ${err.message}`));
        });

        conn.connect({
            host,
            port: parseInt(port, 10),
            username: user,
            privateKey,
        });
    });
}

const args = parseArgs(process.argv);

if (require.main === module) {
    deploy(args)
        .then((result) => {
            log('部署成功');
            console.log(JSON.stringify(result, null, 2));
            process.exit(0);
        })
        .catch((err) => {
            log(`部署失败: ${err.message}`);
            process.exit(1);
        });
}

module.exports = { deploy, parseArgs };