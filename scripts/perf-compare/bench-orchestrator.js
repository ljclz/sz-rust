import { Client } from 'ssh2';
import fs from 'fs';
import path from 'path';
import { startMonitoring, stopAndCollect } from './resource-monitor.js';

const SERVER_HOST = '122.51.216.76';
const SERVER_USER = 'root';
const KEY_PATH = path.resolve(import.meta.dirname, '..', '..', 'deploy_key');
const REMOTE_BENCH_DIR = '/www/rust/perf-compare/benchmarks';
const REMOTE_RESULTS_DIR = '/www/rust/perf-compare';
const RUST_PATH = '/root/.cargo/bin';

const FRAMEWORKS = ['sz-rust', 'actix', 'axum', 'poem'];
const ROUTES = ['/simple', '/json', '/db'];
const CONCURRENCIES = [32, 128, 256];
const WRK_DURATION = 10;

const PORTS = { 'sz-rust': 8401, 'actix': 8402, 'axum': 8403, 'poem': 8405 };
const PKG_NAMES = { 'sz-rust': 'bench-sz-rust', 'actix': 'bench-actix', 'axum': 'bench-axum', 'poem': 'bench-poem' };

function createSSHClient() {
    return new Promise((resolve, reject) => {
        const privateKey = fs.readFileSync(KEY_PATH, 'utf-8');
        const client = new Client();

        client.on('ready', () => resolve(client));
        client.on('error', (err) => reject(err));

        client.connect({
            host: SERVER_HOST,
            port: 22,
            username: SERVER_USER,
            privateKey,
            readyTimeout: 30000,
        });
    });
}

function execCommand(client, command, timeout = 30000) {
    return new Promise((resolve, reject) => {
        let stdout = '';
        let stderr = '';
        let settled = false;

        const timer = setTimeout(() => {
            if (!settled) {
                settled = true;
                reject(new Error(`Command timeout after ${timeout}ms: ${command}`));
            }
        }, timeout);

        client.exec(command, (err, stream) => {
            if (err) {
                clearTimeout(timer);
                if (!settled) { settled = true; reject(err); }
                return;
            }

            stream.on('data', (data) => { stdout += data.toString(); });
            stream.stderr.on('data', (data) => { stderr += data.toString(); });

            stream.on('close', (code) => {
                clearTimeout(timer);
                if (settled) return;
                settled = true;
                resolve({ stdout, stderr, exitCode: code });
            });
        });
    });
}

function execBackground(client, command) {
    return new Promise((resolve, reject) => {
        client.exec(command, (err, stream) => {
            if (err) { reject(err); return; }
            stream.on('close', () => resolve());
            setTimeout(() => resolve(), 1000);
        });
    });
}


async function connectWithRetry(maxRetries = 3, intervalMs = 5000) {
    for (let i = 0; i < maxRetries; i++) {
        try {
            console.log(`[SSH] 连接 ${SERVER_HOST} (尝试 ${i + 1}/${maxRetries})...`);
            const client = await createSSHClient();
            console.log('[SSH] 连接成功');
            return client;
        } catch (err) {
            console.error(`[SSH] 连接失败: ${err.message}`);
            if (i < maxRetries - 1) {
                console.log(`[SSH] ${intervalMs / 1000}s 后重试...`);
                await new Promise(r => setTimeout(r, intervalMs));
            }
        }
    }
    throw new Error('BENCH_SSH_CONNECT_FAILED: 3 次连接均失败');
}

async function compileFramework(client, fw) {
    const pkg = PKG_NAMES[fw];
    const cmd = `cd ${REMOTE_BENCH_DIR}/${fw} && export PATH="${RUST_PATH}:$PATH" && cargo build --release 2>&1 | tail -5`;
    console.log(`[编译] ${fw} (${pkg})...`);
    const { stdout, exitCode } = await execCommand(client, cmd, 600000);
    console.log(`[编译] ${fw} exitCode=${exitCode}`);
    return exitCode === 0;
}

async function killByPort(client, port) {
    const checkCmd = `lsof -i:${port} -t 2>/dev/null`;
    const { stdout } = await execCommand(client, checkCmd);
    if (stdout.trim()) {
        await execCommand(client, `fuser -k ${port}/tcp 2>/dev/null; sleep 1`);
        console.log(`[清理] 端口 ${port} 已释放`);
    }
}

async function startFramework(client, fw, port) {
    const pkg = PKG_NAMES[fw];
    const cmd = `cd ${REMOTE_BENCH_DIR}/${fw} && export PATH="${RUST_PATH}:$PATH" && setsid env PORT=${port} ./target/release/${pkg} > /tmp/bench-${fw}.log 2>&1 < /dev/null &`;
    await execBackground(client, cmd);
    await new Promise(r => setTimeout(r, 3000));

    const { stdout, exitCode } = await execCommand(client, `curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:${port}/simple`);
    if (exitCode === 0 && stdout.trim() === '200') {
        console.log(`[启动] ${fw} 端口 ${port} ✅`);
        return true;
    }
    console.error(`[启动] ${fw} 端口 ${port} ❌ (HTTP ${stdout.trim()})`);
    return false;
}

async function runWrk(client, fw, route, concurrency, port) {
    const cmd = `wrk -t${concurrency} -c${concurrency} -d${WRK_DURATION}s --latency http://127.0.0.1:${port}${route} 2>&1`;
    console.log(`[wrk] ${fw} ${route} c=${concurrency}...`);
    const { stdout } = await execCommand(client, cmd, 30000);

    const result = parseWrkOutput(stdout);
    result.framework = fw;
    result.route = route;
    result.concurrency = concurrency;
    result.port = port;
    result.command = cmd;
    result.timestamp = new Date().toISOString();
    result.wrkDuration = WRK_DURATION;
    return result;
}

function parseWrkOutput(output) {
    const result = { rps: 0, p50: '0', p95: '0', p99: '0', errors: 0, transfer: '', raw: output };

    const rpsMatch = output.match(/Requests\/sec:\s+([\d.]+)/);
    if (rpsMatch) result.rps = parseFloat(rpsMatch[1]);

    const transferMatch = output.match(/Transfer\/sec:\s+([\d.]+\s*\w+)/);
    if (transferMatch) result.transfer = transferMatch[1];

    const latencyLines = output.split('\n');
    for (const line of latencyLines) {
        const p50Match = line.match(/^\s*50%\s+([\d.]+(\w+)?)/);
        if (p50Match) result.p50 = p50Match[1];

        const p95Match = line.match(/^\s*95%\s+([\d.]+(\w+)?)/);
        if (p95Match) result.p95 = p95Match[1];

        const p99Match = line.match(/^\s*99%\s+([\d.]+(\w+)?)/);
        if (p99Match) result.p99 = p99Match[1];
    }

    const errorsMatch = output.match(/Non-2xx or 3xx responses:\s+(\d+)/);
    if (errorsMatch) result.errors = parseInt(errorsMatch[1]);

    return result;
}

async function main() {
    console.log('=== v0.7 远程压测编排 ===');
    console.log(`服务器: ${SERVER_HOST}`);
    console.log(`框架: ${FRAMEWORKS.join(', ')}`);
    console.log(`路由: ${ROUTES.join(', ')}`);
    console.log(`并发: ${CONCURRENCIES.join(', ')}`);
    console.log(`组合数: ${FRAMEWORKS.length * ROUTES.length * CONCURRENCIES.length}`);
    console.log('');

    const client = await connectWithRetry();

    const timestamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    const rawFile = `${REMOTE_RESULTS_DIR}/raw-results-${timestamp}.jsonl`;
    const resultsFile = `${REMOTE_RESULTS_DIR}/results-${timestamp}.json`;

    await execCommand(client, `mkdir -p ${REMOTE_RESULTS_DIR}`);
    await execCommand(client, `echo "" > ${rawFile}`);

    const allResults = [];
    const summary = { timestamp: new Date().toISOString(), wrkDuration: WRK_DURATION, results: [] };

    for (const fw of FRAMEWORKS) {
        const port = PORTS[fw];

        const compiled = await compileFramework(client, fw);
        if (!compiled) {
            console.error(`[跳过] ${fw} 编译失败`);
            for (const route of ROUTES) {
                for (const c of CONCURRENCIES) {
                    const naResult = { framework: fw, route, concurrency: c, status: 'N/A', reason: 'compile_failed', timestamp: new Date().toISOString() };
                    allResults.push(naResult);
                    await execCommand(client, `echo '${JSON.stringify(naResult)}' >> ${rawFile}`);
                }
            }
            continue;
        }

        for (const route of ROUTES) {
            for (const c of CONCURRENCIES) {
                await killByPort(client, port);

                const started = await startFramework(client, fw, port);
                if (!started) {
                    const naResult = { framework: fw, route, concurrency: c, status: 'N/A', reason: 'start_failed', timestamp: new Date().toISOString() };
                    allResults.push(naResult);
                    await execCommand(client, `echo '${JSON.stringify(naResult)}' >> ${rawFile}`);
                    continue;
                }

                let monitorFiles = null;
                try {
                    monitorFiles = await startMonitoring(client, 20);
                } catch (e) {
                    console.log(`  ⚠️ 资源监控启动失败: ${e.message}`);
                }
                await new Promise(r => setTimeout(r, 5000));

                try {
                    const result = await runWrk(client, fw, route, c, port);
                    result.status = 'ok';

                    await new Promise(r => setTimeout(r, 5000));

                    if (monitorFiles) {
                        try {
                            result.resource = await stopAndCollect(client, monitorFiles);
                        } catch (resErr) {
                            result.resource = null;
                        }
                    } else {
                        result.resource = null;
                    }

                    allResults.push(result);
                    await execCommand(client, `echo '${JSON.stringify(result)}' >> ${rawFile}`);
                    console.log(`  RPS=${result.rps} P50=${result.p50} P95=${result.p95} P99=${result.p99} Errors=${result.errors}`);
                } catch (err) {
                    const naResult = { framework: fw, route, concurrency: c, status: 'N/A', reason: 'wrk_timeout', timestamp: new Date().toISOString() };
                    allResults.push(naResult);
                    await execCommand(client, `echo '${JSON.stringify(naResult)}' >> ${rawFile}`);
                    console.error(`  ❌ wrk 失败: ${err.message}`);
                }

                await killByPort(client, port);
                await new Promise(r => setTimeout(r, 2000));
            }
        }
    }

    summary.results = allResults;
    await execCommand(client, `cat > ${resultsFile} << 'ENDJSON'\n${JSON.stringify(summary, null, 2)}\nENDJSON`);

    console.log('');
    console.log('=== 压测完成 ===');
    console.log(`原始结果: ${rawFile}`);
    console.log(`汇总 JSON: ${resultsFile}`);
    console.log(`总组合数: ${allResults.length}`);
    console.log(`成功: ${allResults.filter(r => r.status === 'ok').length}`);
    console.log(`N/A: ${allResults.filter(r => r.status === 'N/A').length}`);

    client.end();
}

main().catch(err => {
    console.error('致命错误:', err.message);
    process.exit(1);
});