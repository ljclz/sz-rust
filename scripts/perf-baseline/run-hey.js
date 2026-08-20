import { createSSHClient, execCommand, closeClient, SERVER_HOST, SERVER_PORT } from './_ssh.js';
import { fileURLToPath } from 'url';

import { sampleResource } from './sample-resource.js';
import { generateReport } from './generate-report.js';

const AB_BIN = '/usr/bin/ab';
const DURATION_SEC = 30;
const QPS_CAP = 2000;
const CONCURRENCIES = [1, 10, 50, 100, 200];

const WASM_BYTES = Buffer.from([
    0x00,0x61,0x73,0x6d, 0x01,0x00,0x00,0x00,
    0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f,
    0x03,0x02,0x01,0x00,
    0x07,0x07,0x01,0x03,0x61,0x64,0x64,0x00,0x00,
    0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6a,0x0b,
]);
const WASM_B64 = WASM_BYTES.toString('base64');

const ENDPOINTS = {
    health: { method: 'GET', url: `http://localhost:${SERVER_PORT}/health`, body: null },
    graphql: { method: 'POST', url: `http://localhost:${SERVER_PORT}/graphql`, body: JSON.stringify({ query: '{ health { status version } }' }) },
    wasm: { method: 'POST', url: `http://localhost:${SERVER_PORT}/api/wasm/execute`, body: JSON.stringify({ wasm: WASM_B64, function: 'add', args: [{ I32: 1 }, { I32: 2 }] }) },
};

function buildAbCmd(ep, concurrency, bodyFile) {
    const parts = [AB_BIN, `-c ${concurrency}`, `-t ${DURATION_SEC}`, '-n 10000000'];
    if (ep.method === 'POST') {
        parts.push(`-p ${bodyFile}`, '-T application/json');
    }
    parts.push(ep.url);
    return parts.join(' ');
}

function parseAbOutput(stdout) {
    const qps = parseFloat(stdout.match(/Requests per second:\s*([\d.]+)/)?.[1] ?? '0');
    const p50 = parseInt(stdout.match(/^\s*50%\s+(\d+)/m)?.[1] ?? '0', 10);
    const p95 = parseInt(stdout.match(/^\s*95%\s+(\d+)/m)?.[1] ?? '0', 10);
    const p99 = parseInt(stdout.match(/^\s*99%\s+(\d+)/m)?.[1] ?? '0', 10);
    const total = parseFloat(stdout.match(/Time taken for tests:\s*([\d.]+)\s+seconds/)?.[1] ?? '0');
    const complete = parseInt(stdout.match(/Complete requests:\s*(\d+)/)?.[1] ?? '0', 10);
    const failed = parseInt(stdout.match(/Failed requests:\s*(\d+)/)?.[1] ?? '0', 10);
    const errorRate = complete > 0 ? (failed / complete) * 100 : 0;

    return { qps, p50Ms: p50, p95Ms: p95, p99Ms: p99, errorRate, totalReqs: complete, totalSecs: total };
}

export async function runBenchmark(config = {}) {
    const concurrencies = config.concurrencies ?? CONCURRENCIES;
    const endpoints = config.endpoints ?? Object.keys(ENDPOINTS);
    const client = await createSSHClient();
    const results = [];

    try {
        for (const epName of endpoints) {
            const ep = ENDPOINTS[epName];
            let bodyFile = null;
            if (ep.method === 'POST') {
                bodyFile = `/tmp/ab_body_${epName}.json`;
                const escapedBody = ep.body.replace(/'/g, "'\\''");
                await execCommand(client, `printf '%s' '${escapedBody}' > ${bodyFile}`, 5000);
            }
            for (const c of concurrencies) {
                const cmd = buildAbCmd(ep, c, bodyFile);
                console.log(`[run-hey] ${epName} c=${c} ...`);
                const { stdout, stderr, exitCode } = await execCommand(client, cmd, 600000);
                if (exitCode !== 0 && !stdout) {
                    console.error(`[run-hey] FAILED ${epName} c=${c}: ${stderr}`);
                    results.push({ endpoint: epName, concurrency: c, error: stderr.trim(), raw: '' });
                    continue;
                }
                const parsed = parseAbOutput(stdout);
                console.log(`[run-hey] ${epName} c=${c}: QPS=${parsed.qps.toFixed(0)} P95=${parsed.p95Ms}ms err=${parsed.errorRate.toFixed(2)}%`);
                results.push({ endpoint: epName, concurrency: c, ...parsed, raw: stdout });
            }
        }
    } finally {
        await closeClient(client);
    }
    return results;
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isMain) {
    if (process.argv.includes('--dry-run')) {
        console.log(JSON.stringify({ tool: 'ab', endpoints: Object.keys(ENDPOINTS), concurrencies: CONCURRENCIES, duration: `${DURATION_SEC}s`, qpsCap: QPS_CAP }, null, 2));
        process.exit(0);
    }

    (async () => {
        const sshClient = await createSSHClient();
        let resourceSamples = null;
        let pid = null;
        let unhealthy = false;
        const probeHistory = [];

        async function healthCheck() {
            const { stdout } = await execCommand(sshClient, `curl -s -o /dev/null -w "%{http_code}" http://localhost:${SERVER_PORT}/health`, 5000);
            return parseInt(stdout.trim(), 10) || 0;
        }

        async function healthProbeLoop(durationMs) {
            const end = Date.now() + durationMs;
            let consecutiveFailures = 0;
            while (Date.now() < end && !unhealthy) {
                const status = await healthCheck();
                probeHistory.push({ time: new Date().toISOString(), status });
                if (status === 200) consecutiveFailures = 0;
                else {
                    consecutiveFailures++;
                    if (consecutiveFailures >= 3) { unhealthy = true; console.error('[health-probe] 不健康，中止'); break; }
                }
                await new Promise(r => setTimeout(r, 5000));
            }
        }

        try {
            const { stdout: pidOut } = await execCommand(sshClient, 'pgrep -x sz300-server | head -1', 5000);
            pid = pidOut.trim();
            console.log(`[run-hey] sz300-server pid=${pid}`);

            const benchPromise = runBenchmark();
            const probePromise = healthProbeLoop(450000);
            const samplePromise = pid ? sampleResource(sshClient, pid, 2000, 450000) : Promise.resolve(null);
            const [results, , samples] = await Promise.all([benchPromise, probePromise, samplePromise]);
            resourceSamples = samples;

            const healthyRate = probeHistory.length > 0 ? (probeHistory.filter(h => h.status === 200).length / probeHistory.length) * 100 : 100;
            const healthStatus = { history: probeHistory, healthyRate, unhealthy };

            const { reportPath } = await generateReport(results, resourceSamples, healthStatus, {
                heyVersion: 'ab (Apache Benchmark 2.3)',
                serverPort: SERVER_PORT,
                duration: `${DURATION_SEC}s`,
                qpsCap: QPS_CAP,
                benchCommand: `ab -c <并发> -t ${DURATION_SEC} -n 10000000 http://122.51.216.76:${SERVER_PORT}<端点路径>`,
            });
            console.log(`[run-hey] 报告已生成: ${reportPath}`);
            console.log(JSON.stringify(results.map(({ raw, ...rest }) => rest), null, 2));
        } finally {
            await closeClient(sshClient);
        }
    })().catch(e => { console.error(e.message); process.exit(1); });
}
