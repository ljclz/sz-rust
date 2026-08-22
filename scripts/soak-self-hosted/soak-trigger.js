const { Client } = require('ssh2');
const fs = require('fs');
const path = require('path');

const SERVER = '122.51.216.76';
const USERNAME = 'root';

function getArg(name, defaultValue) {
    const idx = process.argv.indexOf(name);
    if (idx !== -1 && process.argv[idx + 1]) {
        return process.argv[idx + 1];
    }
    return defaultValue;
}

const TOOLKIT_DIR = '/www/rust/soak-toolkit';
const KEY_PATH = getArg('--key-path', process.env.SOAK_SSH_KEY_PATH || path.join(__dirname, '..', '..', 'deploy_key'));

if (!fs.existsSync(KEY_PATH)) {
    console.error(`❌ SSH 密钥不存在: ${KEY_PATH}`);
    console.error('   请通过 --key-path 参数或 SOAK_SSH_KEY_PATH 环境变量指定');
    process.exit(1);
}

const privateKey = fs.readFileSync(KEY_PATH, 'utf-8');

const duration = getArg('--duration', '10s');
const trigger = getArg('--trigger', 'manual');
const project = getArg('--project', 'sz-rust');
const port = getArg('--port', '8300');
const workDir = getArg('--work-dir', '/www/rust/sz-rust-soak');
const reportDir = getArg('--report-dir', '/www/rust/soak-reports');
const soakPorts = getArg('--soak-ports', '8401-8405');
const protectedProcess = getArg('--protected-process', 'sz-rust-sz300');
const restartScript = getArg('--restart-script', '/www/rust/sz-rust-soak/restart-sz300.sh');
const cronMarker = getArg('--cron-marker', '# sz-rust-soak');

function runCommand(conn, cmd, timeout = 600000) {
    return new Promise((resolve, reject) => {
        let stdout = '';
        let stderr = '';
        console.log(`> ${cmd}`);
        conn.exec(cmd, (err, stream) => {
            if (err) return reject(err);
            stream.on('close', (code) => {
                resolve({ code, stdout, stderr });
            });
            stream.on('data', (data) => {
                stdout += data.toString();
                process.stdout.write(data);
            });
            stream.stderr.on('data', (data) => {
                stderr += data.toString();
                process.stderr.write(data);
            });
        });
    });
}

function uploadFile(conn, localPath, remotePath) {
    return new Promise((resolve, reject) => {
        conn.sftp((err, sftp) => {
            if (err) return reject(err);
            sftp.fastPut(localPath, remotePath, (err) => {
                if (err) return reject(err);
                sftp.end();
                resolve();
            });
        });
    });
}

async function main() {
    console.log(`=== Soak Trigger ===`);
    console.log(`Project: ${project}, Duration: ${duration}, Trigger: ${trigger}`);
    console.log(`Port: ${port}, WorkDir: ${workDir}, ReportDir: ${reportDir}`);
    console.log(`SoakPorts: ${soakPorts}, ProtectedProcess: ${protectedProcess}`);
    console.log(`ToolkitDir: ${TOOLKIT_DIR}\n`);

    const scriptsToUpload = [
        'soak-runner.sh',
        'soak-archive.sh',
        'process-guard.sh',
        'soak-cron-setup.sh',
        'config-defaults.sh',
    ];

    for (let attempt = 1; attempt <= 3; attempt++) {
        try {
            const conn = new Client();
            await new Promise((resolve, reject) => {
                conn.on('ready', () => resolve());
                conn.on('error', reject);
                conn.connect({
                    host: SERVER, port: 22, username: USERNAME,
                    privateKey: privateKey, readyTimeout: 30000
                });
            });

            console.log('--- 上传脚本到通用工具目录 ---');
            await runCommand(conn, `mkdir -p ${TOOLKIT_DIR}`);
            for (const script of scriptsToUpload) {
                const localPath = path.join(__dirname, script);
                const remotePath = `${TOOLKIT_DIR}/${script}`;
                await uploadFile(conn, localPath, remotePath);
            }
            await runCommand(conn, `chmod +x ${TOOLKIT_DIR}/*.sh`);
            console.log('');

            console.log('--- 执行 Soak Test ---');
            const runnerCmd = `bash ${TOOLKIT_DIR}/soak-runner.sh` +
                ` --duration ${duration}` +
                ` --trigger ${trigger}` +
                ` --project ${project}` +
                ` --protected-port ${port}` +
                ` --work-dir ${workDir}` +
                ` --report-dir ${reportDir}` +
                ` --soak-ports ${soakPorts}` +
                ` --protected-process ${protectedProcess}` +
                ` --restart-script ${restartScript}` +
                ` --cron-marker '${cronMarker}'`;

            const result = await runCommand(conn,
                `source ~/.cargo/env 2>/dev/null; ${runnerCmd}`,
                600000
            );
            console.log(`\n退出码: ${result.code}\n`);

            console.log('--- 验证归档 ---');
            await runCommand(conn, `ls -la ${reportDir}/ && cat ${reportDir}/index.csv`);
            console.log('');

            console.log(`--- 验证 ${protectedProcess} 存活 ---`);
            await runCommand(conn, `fuser ${port}/tcp 2>/dev/null || echo "${port} 无进程"`);
            console.log('');

            conn.end();
            console.log('=== 完成 ===');
            process.exit(result.code || 0);
        } catch (err) {
            console.error(`尝试 ${attempt}/3 失败: ${err.message}`);
            if (attempt < 3) {
                console.log('等待 30 秒后重试...');
                await new Promise(r => setTimeout(r, 30000));
            } else {
                console.error('❌ 3 次重试均失败');
                process.exit(10);
            }
        }
    }
}

main();
