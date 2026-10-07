#!/usr/bin/env node
'use strict';
/**
 * v1.9.0 性能回归门禁脚本（spec §5.8.1 规则 5）
 *
 * 用法：
 *   node scripts/perf-regression.js                # 压测 + 对比基线，回归 > 5% 退出码 1
 *   node scripts/perf-regression.js --init         # 首次运行，建立当前为基线（spec §5.8.3 异常 1）
 *   node scripts/perf-regression.js --skip-bench   # 跳过压测，仅校验基线文件格式
 *   node scripts/perf-regression.js --host 127.0.0.1 --port 8300
 *
 * 热路径（spec §5.8.1 规则 1）：
 *   - GET  /health
 *   - POST /api/v1/product/list
 *   - POST /api/v1/order/list
 *
 * 回归判定：QPS < 基线 × 95% → 回归 > 5% → CI 失败
 */
const fs = require('fs');
const http = require('http');
const path = require('path');
const os = require('os');

const BASELINE_PATH = path.resolve(__dirname, '..', 'packages', 'sz-rust-benchmark-suite', 'benches', 'baseline.json');
const REGRESSION_THRESHOLD_PCT = 5.0;
const DEFAULT_HOST = process.env.SZ300_SERVER_HOST || '127.0.0.1';
const DEFAULT_PORT = parseInt(process.env.SZ300_SERVER_PORT || '8300', 10);
const CONCURRENCY = 50;
const DURATION_MS = 5000;
const WARMUP_MS = 1000;

function parseArgs(argv) {
    const args = { init: false, skipBench: false, host: DEFAULT_HOST, port: DEFAULT_PORT };
    for (let i = 2; i < argv.length; i++) {
        const a = argv[i];
        if (a === '--init') args.init = true;
        else if (a === '--skip-bench') args.skipBench = true;
        else if (a === '--host') args.host = argv[++i];
        else if (a === '--port') args.port = parseInt(argv[++i], 10);
    }
    return args;
}

const HOT_PATHS = [
    { path: '/health', method: 'GET', body: null },
    { path: '/api/v1/product/list', method: 'POST', body: JSON.stringify({ page: 1, page_size: 20 }) },
    { path: '/api/v1/order/list', method: 'POST', body: JSON.stringify({ page: 1, page_size: 20 }) },
];

function sendRequest(host, port, ep) {
    return new Promise((resolve) => {
        const start = process.hrtime.bigint();
        const req = http.request({
            hostname: host,
            port,
            path: ep.path,
            method: ep.method,
            headers: { 'Content-Type': 'application/json' },
            timeout: 5000,
        }, (res) => {
            res.resume();
            res.on('end', () => {
                const elapsedNs = Number(process.hrtime.bigint() - start);
                resolve({ ok: res.statusCode >= 200 && res.statusCode < 400, elapsedMs: elapsedNs / 1e6, status: res.statusCode });
            });
        });
        req.on('error', () => resolve({ ok: false, elapsedMs: 0, status: 0 }));
        req.on('timeout', () => { req.destroy(); resolve({ ok: false, elapsedMs: 5000, status: 0 }); });
        if (ep.body) req.write(ep.body);
        req.end();
    });
}

async function warmup(host, port) {
    for (const ep of HOT_PATHS) {
        await sendRequest(host, port, ep);
    }
}

async function benchEndpoint(host, port, ep, durationMs, concurrency) {
    const results = [];
    const deadline = Date.now() + durationMs;
    async function worker() {
        while (Date.now() < deadline) {
            const r = await sendRequest(host, port, ep);
            results.push(r);
        }
    }
    const workers = Array.from({ length: concurrency }, () => worker());
    await Promise.all(workers);

    const ok = results.filter((r) => r.ok);
    const total = results.length;
    const elapsedSec = durationMs / 1000;
    const qps = total / elapsedSec;
    const errorRate = total > 0 ? ((total - ok.length) / total) * 100 : 100;
    const latencies = ok.map((r) => r.elapsedMs).sort((a, b) => a - b);
    const pct = (p) => latencies.length > 0 ? latencies[Math.min(Math.floor(latencies.length * p), latencies.length - 1)] : 0;

    return {
        path: ep.path,
        method: ep.method,
        concurrency,
        duration_s: elapsedSec,
        qps: Math.round(qps * 10) / 10,
        p50_ms: Math.round(pct(0.50) * 100) / 100,
        p95_ms: Math.round(pct(0.95) * 100) / 100,
        p99_ms: Math.round(pct(0.99) * 100) / 100,
        error_rate_pct: Math.round(errorRate * 100) / 100,
        total_requests: total,
    };
}

async function runBench(host, port) {
    console.log(`[perf-regression] 压测目标: http://${host}:${port}`);
    console.log(`[perf-regression] 预热 ${WARMUP_MS}ms ...`);
    await warmup(host, port);
    await new Promise((r) => setTimeout(r, WARMUP_MS));

    const results = [];
    for (const ep of HOT_PATHS) {
        console.log(`[perf-regression] 压测 ${ep.method} ${ep.path} (c=${CONCURRENCY}, t=${DURATION_MS}ms) ...`);
        const r = await benchEndpoint(host, port, ep, DURATION_MS, CONCURRENCY);
        console.log(`  QPS=${r.qps} P50=${r.p50_ms}ms P95=${r.p95_ms}ms P99=${r.p99_ms}ms err=${r.error_rate_pct}%`);
        results.push(r);
    }
    return results;
}

function loadBaseline() {
    if (!fs.existsSync(BASELINE_PATH)) {
        return null;
    }
    return JSON.parse(fs.readFileSync(BASELINE_PATH, 'utf8'));
}

function saveBaseline(results) {
    const cpuModel = os.cpus()[0] ? os.cpus()[0].model : 'unknown';
    const baseline = {
        version: `v1.9.0-${new Date().toISOString().slice(0, 10)}`,
        measured_at: new Date().toISOString(),
        note: '由 scripts/perf-regression.js --init 自动建立',
        hardware_spec: `${process.platform}/${process.arch} ${cpuModel}`,
        regression_threshold_pct: REGRESSION_THRESHOLD_PCT,
        hot_paths: results,
    };
    fs.writeFileSync(BASELINE_PATH, JSON.stringify(baseline, null, 2) + '\n', 'utf8');
    console.log(`[perf-regression] 基线已保存到 ${BASELINE_PATH}`);
}

function compareWithBaseline(current, baseline) {
    const report = [];
    let failed = false;
    const baselineMap = new Map(baseline.hot_paths.map((h) => [h.path, h]));
    for (const cur of current) {
        const base = baselineMap.get(cur.path);
        if (!base) {
            report.push({ path: cur.path, status: 'NO_BASELINE', cur_qps: cur.qps, base_qps: null, regression_pct: null });
            continue;
        }
        const regressionPct = ((base.qps - cur.qps) / base.qps) * 100;
        const threshold = baseline.regression_threshold_pct || REGRESSION_THRESHOLD_PCT;
        const isRegression = regressionPct > threshold;
        if (isRegression) failed = true;
        report.push({
            path: cur.path,
            status: isRegression ? 'REGRESSION' : 'PASS',
            cur_qps: cur.qps,
            base_qps: base.qps,
            regression_pct: Math.round(regressionPct * 100) / 100,
            threshold_pct: threshold,
            cur_p99_ms: cur.p99_ms,
            base_p99_ms: base.p99_ms,
        });
    }
    return { report, failed };
}

function printReport(report, failed) {
    console.log('\n========== 性能回归报告 ==========');
    console.log('| 端点 | 状态 | 当前QPS | 基线QPS | 回归% | 阈值% | 当前P99 | 基线P99 |');
    console.log('|------|------|---------|---------|-------|-------|---------|---------|');
    for (const r of report) {
        console.log(`| ${r.path} | ${r.status} | ${r.cur_qps || '-'} | ${r.base_qps || '-'} | ${r.regression_pct || '-'} | ${r.threshold_pct || '-'} | ${r.cur_p99_ms || '-'} | ${r.base_p99_ms || '-'} |`);
    }
    console.log('==================================');
    if (failed) {
        console.error('\n[perf-regression] FAIL: 性能回归超过阈值，阻塞合并');
    } else {
        console.log('\n[perf-regression] PASS: 性能回归在阈值内');
    }
}

async function main() {
    const args = parseArgs(process.argv);

    if (args.skipBench) {
        const baseline = loadBaseline();
        if (!baseline) {
            console.error('[perf-regression] FAIL: 基线文件不存在');
            process.exit(1);
        }
        if (!baseline.hot_paths || baseline.hot_paths.length < 3) {
            console.error(`[perf-regression] FAIL: 基线热路径数 < 3 (实际 ${baseline.hot_paths ? baseline.hot_paths.length : 0})`);
            process.exit(1);
        }
        console.log(`[perf-regression] PASS: 基线文件有效，含 ${baseline.hot_paths.length} 条热路径`);
        process.exit(0);
    }

    const current = await runBench(args.host, args.port);

    if (args.init) {
        saveBaseline(current);
        console.log('[perf-regression] INIT: 基线已建立，CI 通过');
        process.exit(0);
    }

    const baseline = loadBaseline();
    if (!baseline) {
        console.log('[perf-regression] 无历史基线，自动建立当前为基线（spec §5.8.3 异常 1）');
        saveBaseline(current);
        process.exit(0);
    }

    const { report, failed } = compareWithBaseline(current, baseline);
    printReport(report, failed);
    process.exit(failed ? 1 : 0);
}

main().catch((err) => {
    console.error('[perf-regression] FATAL:', err.message);
    process.exit(2);
});