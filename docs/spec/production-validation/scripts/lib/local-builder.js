import { spawn } from 'child_process';
import fs from 'fs';
import path from 'path';

export class LocalBuilder {
    constructor(projectRoot) {
        this.projectRoot = projectRoot;
    }

    async buildAll(applications) {
        const results = [];

        for (const app of applications) {
            console.log(`[编译] 开始编译 ${app.name}...`);
            const result = await this.buildOne(app);
            results.push(result);
        }

        return results;
    }

    async buildOne(app) {
        const localPath = app.localPath;
        const binaryName = app.remoteBinaryName;
        const expectedBinaryPath = app.localBinaryPath;

        if (!fs.existsSync(localPath)) {
            throw new BuildFailedError(app.name, `本地路径不存在: ${localPath}`);
        }

        return new Promise((resolve, reject) => {
            const env = {
                ...process.env,
                CARGO_INCREMENTAL: '0',
            };

            const child = spawn('cargo', [
                'build', '--release', '--target', 'x86_64-unknown-linux-musl'
            ], {
                cwd: localPath,
                env,
                stdio: ['ignore', 'pipe', 'pipe'],
            });

            let stdout = '';
            let stderr = '';

            child.stdout.on('data', (data) => {
                stdout += data.toString();
                process.stdout.write(data);
            });

            child.stderr.on('data', (data) => {
                stderr += data.toString();
                process.stderr.write(data);
            });

            const timer = setTimeout(() => {
                child.kill('SIGTERM');
                reject(new BuildFailedError(app.name, `编译超时 (600s)`));
            }, 600000);

            child.on('close', (code) => {
                clearTimeout(timer);

                if (code !== 0) {
                    reject(new BuildFailedError(
                        app.name,
                        `编译失败 (exit ${code})\nstderr: ${stderr}\n证据: Cargo.toml:23 (sz-rust-core 0.6.7 依赖行)`
                    ));
                    return;
                }

                if (!fs.existsSync(expectedBinaryPath)) {
                    reject(new BuildFailedError(
                        app.name,
                        `编译产物不存在: ${expectedBinaryPath}`
                    ));
                    return;
                }

                const stats = fs.statSync(expectedBinaryPath);
                if (stats.size === 0) {
                    reject(new BuildFailedError(app.name, `编译产物大小为 0: ${expectedBinaryPath}`));
                    return;
                }

                console.log(`[编译] ${app.name} 编译成功，产物: ${expectedBinaryPath} (${(stats.size / 1024 / 1024).toFixed(2)} MB)`);
                resolve({
                    name: app.name,
                    binaryPath: expectedBinaryPath,
                    size: stats.size,
                });
            });

            child.on('error', (err) => {
                clearTimeout(timer);
                reject(new BuildFailedError(app.name, err.message));
            });
        });
    }

    async verifyNoUpstreamChanges() {
        const szOrmPath = path.resolve(this.projectRoot, '..', 'sz-orm');

        if (!fs.existsSync(szOrmPath)) {
            return { changed: false, message: 'sz-orm 仓库不存在，跳过检查' };
        }

        return new Promise((resolve) => {
            const child = spawn('git', ['status', '--porcelain'], {
                cwd: szOrmPath,
                stdio: ['ignore', 'pipe', 'pipe'],
            });

            let stdout = '';
            child.stdout.on('data', (data) => { stdout += data.toString(); });
            child.on('close', () => {
                const changed = stdout.trim().length > 0;
                resolve({
                    changed,
                    message: changed ? `sz-orm 仓库有变更:\n${stdout}` : 'sz-orm 仓库无变更',
                });
            });
        });
    }
}

export class BuildFailedError extends Error {
    constructor(appName, detail) {
        super(`编译失败 [${appName}]: ${detail}`);
        this.name = 'BUILD_FAILED';
        this.appName = appName;
    }
}