import { EvidenceCollector } from '../lib/evidence-collector.js';
import fs from 'fs';

export async function validateDeploy(ssh, config, buildResults, projectRoot) {
    const startedAt = Date.now();
    const evidenceCollector = new EvidenceCollector(projectRoot);
    const evidences = [];
    const errors = [];
    const processes = [];
    let passed = true;

    for (let i = 0; i < config.applications.length; i++) {
        const app = config.applications[i];
        const buildResult = buildResults[i];

        if (!buildResult || !fs.existsSync(buildResult.binaryPath)) {
            errors.push({ app: app.name, error: 'BUILD_ARTIFACT_MISSING', detail: '编译产物不存在' });
            passed = false;
            continue;
        }

        const result = await deployOneApp(ssh, app, buildResult, evidenceCollector, evidences, errors);
        if (result) {
            processes.push(result);
        } else {
            passed = false;
        }
    }

    return {
        module: 'Deploy',
        passed,
        processes,
        evidences,
        errors,
        duration: Date.now() - startedAt,
    };
}

async function deployOneApp(ssh, app, buildResult, evidenceCollector, evidences, errors) {
    const remoteDir = app.remoteDir;
    const remoteBinary = `${remoteDir}/${app.remoteBinaryName}`;
    const backupDir = `${remoteDir}/backup`;
    const timestamp = new Date().toISOString().replace(/[:.]/g, '-');

    try {
        await ssh.execCommand(`mkdir -p ${remoteDir} ${backupDir}`, { timeout: 5000 });

        const { stdout: fuserOut } = await ssh.execCommand(`fuser ${app.port}/tcp 2>/dev/null || true`, { timeout: 5000, expectNonZero: true });
        const pids = fuserOut.trim().split(/\s+/).filter(p => p);
        if (pids.length > 0) {
            console.log(`[部署] ${app.name} 端口 ${app.port} 被 PID ${pids.join(',')} 占用，精准终止...`);
            await ssh.execCommand(`fuser -k ${app.port}/tcp || true`, { timeout: 5000, expectNonZero: true });
            await ssh.execCommand('sleep 2', { timeout: 5000 });
            const { stdout: recheck } = await ssh.execCommand(`fuser ${app.port}/tcp 2>/dev/null || true`, { timeout: 5000, expectNonZero: true });
            if (recheck.trim().length > 0) {
                errors.push({ app: app.name, error: 'PORT_OCCUPIED', detail: `端口 ${app.port} 无法释放` });
                return null;
            }
        }

        console.log(`[部署] 上传 ${app.name} 二进制...`);
        await ssh.uploadFile(buildResult.binaryPath, remoteBinary);

        await ssh.execCommand(`chmod +x ${remoteBinary}`, { timeout: 5000 });
        await ssh.execCommand(`cp ${remoteBinary} ${backupDir}/${app.remoteBinaryName}.bak.${timestamp}`, { timeout: 5000 });

        console.log(`[部署] 启动 ${app.name}...`);
        await ssh.execCommand(`cd ${remoteDir} && nohup ./${app.remoteBinaryName} > ${app.remoteBinaryName}.log 2>&1 &`, { timeout: 5000 });

        await ssh.execCommand('sleep 3', { timeout: 5000 });

        const { stdout: healthOut } = await ssh.execCommand(
            `curl -s http://127.0.0.1:${app.port}${app.healthEndpoint}`,
            { timeout: 10000 }
        );

        if (!healthOut || healthOut.length === 0) {
            errors.push({ app: app.name, error: 'HEALTH_CHECK_FAILED', detail: '健康检查无响应，自动回滚' });
            await ssh.execCommand(`fuser -k ${app.port}/tcp || true`, { timeout: 5000, expectNonZero: true });
            await ssh.execCommand('sleep 2', { timeout: 5000 });
            await ssh.execCommand(`cp ${backupDir}/${app.remoteBinaryName}.bak.${timestamp} ${remoteBinary}`, { timeout: 5000 });
            await ssh.execCommand(`chmod +x ${remoteBinary}`, { timeout: 5000 });
            await ssh.execCommand(`cd ${remoteDir} && nohup ./${app.remoteBinaryName} > ${app.remoteBinaryName}.log 2>&1 &`, { timeout: 5000 });
            return null;
        }

        evidences.push(evidenceCollector.createEvidence(
            `${app.name} 健康检查通过`,
            'packages/sz-rust-sz300/src/controllers/health.rs',
            '10-21'
        ));

        const { stdout: pidOut } = await ssh.execCommand(`fuser ${app.port}/tcp 2>/dev/null`, { timeout: 5000, expectNonZero: true });
        const pid = pidOut.trim().split(/\s+/)[0];

        const { stdout: startOut } = await ssh.execCommand(`ps -p ${pid} -o lstart= 2>/dev/null`, { timeout: 5000 });
        const { stdout: rssOut } = await ssh.execCommand(`ps -p ${pid} -o rss= 2>/dev/null`, { timeout: 5000 });
        const rssKB = parseInt(rssOut.trim(), 10);

        if (rssKB > 30 * 1024) {
            errors.push({ app: app.name, error: 'RSS_EXCEED_30MB', detail: `RSS = ${rssKB} KB` });
        }

        console.log(`[部署] ${app.name} 部署成功: PID=${pid}, RSS=${rssKB}KB, 启动时间=${startOut.trim()}`);

        return {
            name: app.name,
            pid: parseInt(pid, 10),
            port: app.port,
            startedAt: startOut.trim(),
            rssKB,
        };

    } catch (err) {
        errors.push({ app: app.name, error: err.name || 'DEPLOY_ERROR', detail: err.message });
        return null;
    }
}