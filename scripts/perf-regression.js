#!/usr/bin/env node
'use strict';
/**
 * 性能回归门禁 v2（2026-10-29 重写，替代 v1.9.0 的 5s/1c 版本）
 *
 * ## 为什么重写
 *
 * 旧版实测存在四类缺陷（见 docs/audit/2026-10-29-性能工程评测报告.md §8）：
 *   1. 预热仅 1s、稳态仅 5s、单次测量 —— 测的是冷启动 + 噪声，比 60s 稳态高估 1.34–1.41×；
 *   2. QPS 分母用 successes+errors，且没有错误率门禁 —— 业务端点 100% 403 仍能"PASS"；
 *   3. 分位数只统计成功请求 —— 失败请求的真实延迟被隐藏；
 *   4. 基线是跨机器硬编码占位值（/health 50000）—— 任何机器都必然"回归"。
 *
 * ## 现行方法
 *
 *   - 每个端点：预热 `--warmup`（默认 10s，同并发）→ 稳态 `--duration`（默认 60s）× `--reps`（默认 3）
 *   - 闭环（closed-loop，N 并发 keep-alive worker）：报告必须按闭环解读，
 *     饱和时真实排队延迟高于实测值（coordinated omission）
 *   - QPS = 成功请求数（2xx/3xx）/ 稳态时长；错误率单独门禁
 *   - 分位数覆盖**全部已完成响应**（含 4xx/5xx），传输失败单独计 transport_errors
 *   - 统计口径：多次重复取**中位数**，同时打印 CV（标准差/均值）标识噪声
 *   - 判定：中位数回归超阈值 **且** ≥2 次重复同时回归才失败（抑制单次抖动误报）；
 *     阈值自适应 = max(名义阈值, 3×CV)，上限 30% —— 噪声大于阈值时不应假装能判回归
 *   - 错误率门禁独立生效（默认 >0.5% 直接失败），失败请求不得计入 QPS
 *   - 基线按**环境指纹**（CPU 型号/逻辑核数/内存/OS/Node 主版本）分槽存放，换机器不会误判
 *
 * ## 用法
 *
 *   node scripts/perf-regression.js --init                 # 为当前指纹写入基线
 *   node scripts/perf-regression.js                        # 压测 + 与当前指纹基线对比
 *   node scripts/perf-regression.js --skip-bench           # 只校验基线文件结构（无服务依赖；
 *                                                        # 无指纹基线时输出 GATE INACTIVE 告警）
 *   node scripts/perf-regression.js --host 127.0.0.1 --port 8300 \
 *        --jwt-secret "$SZ300_JWT_SECRET"                  # 业务端点需 JWT + CSRF
 *   node scripts/perf-regression.js --skip-business        # 仅压 /health（明确降级，CI 需显式传）
 *
 * ## 退出码
 *
 *   0 = PASS    1 = 门禁失败    2 = 执行错误    3 = 当前指纹无基线（需 --init）
 *
 * ## 安全约束
 *
 * 默认只允许回环地址（127.0.0.1/::1/localhost），防止误对共享/生产环境施压；
 * 确需对远端施加压力时必须显式传 `--allow-non-loopback`。
 */

const fs = require('fs');
const http = require('http');
const path = require('path');
const os = require('os');
const crypto = require('crypto');

const ROOT = path.resolve(__dirname, '..');
const BASELINE_PATH = path.join(
    ROOT,
    'packages',
    'sz-rust-benchmark-suite',
    'benches',
    'baseline.json'
);

const DEFAULTS = {
    host: process.env.SZ300_SERVER_HOST || '127.0.0.1',
    port: parseInt(process.env.SZ300_SERVER_PORT || '8300', 10),
    warmupMs: 10_000,
    durationMs: 60_000,
    reps: 3,
    concurrency: 50,
    qpsThresholdPct: 5.0,
    p99ThresholdPct: 25.0,
    maxErrorRatePct: 0.5,
    maxCvPct: 15.0,
    requestTimeoutMs: 10_000,
    jwtUserId: 1,
};

const ENDPOINTS = [
    { id: 'health', method: 'GET', path: '/health', body: null, auth: false },
    {
        id: 'product_list',
        method: 'POST',
        path: '/api/v1/product/list',
        body: JSON.stringify({ page: 1, page_size: 20 }),
        auth: true,
    },
    {
        id: 'order_list',
        method: 'POST',
        path: '/api/v1/order/list',
        body: JSON.stringify({ page: 1, page_size: 20 }),
        auth: true,
    },
];

function parseArgs(argv) {
    const args = {
        init: false,
        skipBench: false,
        skipBusiness: false,
        json: null,
        allowNonLoopback: false,
        jwt: process.env.SZ300_PERF_JWT || '',
        jwtSecret: process.env.SZ300_JWT_SECRET || '',
        csrfToken: process.env.SZ300_PERF_CSRF || '',
        ...DEFAULTS,
    };
    for (let i = 2; i < argv.length; i++) {
        const a = argv[i];
        switch (a) {
            case '--init': args.init = true; break;
            case '--skip-bench': args.skipBench = true; break;
            case '--skip-business': args.skipBusiness = true; break;
            case '--allow-non-loopback': args.allowNonLoopback = true; break;
            case '--host': args.host = argv[++i]; break;
            case '--port': args.port = parseInt(argv[++i], 10); break;
            case '--warmup': args.warmupMs = Math.round(parseFloat(argv[++i]) * 1000); break;
            case '--duration': args.durationMs = Math.round(parseFloat(argv[++i]) * 1000); break;
            case '--reps': args.reps = parseInt(argv[++i], 10); break;
            case '--concurrency': args.concurrency = parseInt(argv[++i], 10); break;
            case '--qps-threshold': args.qpsThresholdPct = parseFloat(argv[++i]); break;
            case '--p99-threshold': args.p99ThresholdPct = parseFloat(argv[++i]); break;
            case '--max-error-rate': args.maxErrorRatePct = parseFloat(argv[++i]); break;
            case '--json': args.json = argv[++i]; break;
            case '--jwt': args.jwt = argv[++i]; break;
            case '--jwt-secret': args.jwtSecret = argv[++i]; break;
            case '--user-id': args.jwtUserId = parseInt(argv[++i], 10); break;
            case '--help':
            case '-h':
                printUsage();
                process.exit(0);
                break;
            default:
                throw new Error(`未知参数: ${a}`);
        }
    }
    if (!Number.isFinite(args.port) || args.port <= 0 || args.port > 65535) {
        throw new Error(`非法端口: ${args.port}`);
    }
    if (args.reps < 1) throw new Error('--reps 必须 ≥ 1');
    if (args.durationMs < 1000) throw new Error('--duration 必须 ≥ 1s');
    if (args.concurrency < 1) throw new Error('--concurrency 必须 ≥ 1');
    return args;
}

function printUsage() {
    console.log(`用法: node scripts/perf-regression.js [options]

  --init                  为当前环境指纹写入基线
  --skip-bench            仅校验基线文件结构（不需要服务）
  --skip-business         仅压 /health（跳过需要鉴权的业务端点）
  --host / --port         目标地址（默认 127.0.0.1:8300）
  --warmup <s>            预热秒数（默认 10）
  --duration <s>          稳态秒数（默认 60）
  --reps <n>              重复次数（默认 3）
  --concurrency <n>       闭环并发（默认 50）
  --qps-threshold <pct>   QPS 回归阈值名义值（默认 5；实际取 max(该值, 3×CV)，上限 30）
  --p99-threshold <pct>   P99 退化阈值（默认 25）
  --max-error-rate <pct>  错误率上限（默认 0.5）
  --jwt <token>           业务端点访问令牌（优先）
  --jwt-secret <secret>   用 SZ300_JWT_SECRET 现场签发令牌
  --json <path>           额外输出机器可读结果
  --allow-non-loopback    允许对非回环地址施压（默认禁止）
`);
}

function isLoopback(host) {
    return host === 'localhost' || host === '127.0.0.1' || host === '::1' || host === '0:0:0:0:0:0:0:1';
}

// ============================================================================
// 环境指纹
// ============================================================================

function buildFingerprint() {
    const cpus = os.cpus();
    return {
        platform: process.platform,
        arch: process.arch,
        cpu_model: cpus.length > 0 ? cpus[0].model.trim().replace(/\s+/g, ' ') : 'unknown',
        logical_cpus: cpus.length,
        mem_gib: Math.round(os.totalmem() / 1024 ** 3),
        os_release: os.release(),
        node_major: process.versions.node.split('.')[0],
    };
}

function fingerprintKey(fp) {
    return crypto.createHash('sha256').update(JSON.stringify(fp)).digest('hex').slice(0, 16);
}

// ============================================================================
// 鉴权：现场签发 HS256 令牌（与 sz-orm JwtClaims 对齐）
// ============================================================================

function b64url(buf) {
    return Buffer.from(buf)
        .toString('base64')
        .replace(/=+$/, '')
        .replace(/\+/g, '-')
        .replace(/\//g, '_');
}

function mintJwt(secret, userId) {
    const now = Math.floor(Date.now() / 1000);
    const header = b64url(JSON.stringify({ alg: 'HS256', typ: 'JWT' }));
    const claims = b64url(
        JSON.stringify({
            sub: 'perf-gate',
            exp: now + 3600,
            iat: now,
            user_id: userId,
            roles: ['admin'],
            permissions: [],
            token_use: 'access',
        })
    );
    const signingInput = `${header}.${claims}`;
    const signature = b64url(
        crypto.createHmac('sha256', secret).update(signingInput).digest()
    );
    return `${signingInput}.${signature}`;
}

function buildAuthContext(args) {
    let token = args.jwt;
    if (!token && args.jwtSecret) {
        token = mintJwt(args.jwtSecret, args.jwtUserId);
    }
    const csrf = args.csrfToken || crypto.randomBytes(24).toString('hex');
    return { token, csrf };
}

function headersFor(ep, auth) {
    const headers = { 'Content-Type': 'application/json' };
    if (ep.auth && auth.token) {
        headers.Authorization = `Bearer ${auth.token}`;
        // CSRF 双提交：Cookie 与 Header 必须一致（sz-rust-middleware-facade/csrf.rs）
        headers.Cookie = `csrf_token=${auth.csrf}`;
        headers['X-CSRF-TOKEN'] = auth.csrf;
    }
    return headers;
}

// ============================================================================
// 负载生成（闭环）
// ============================================================================

function createAgent(concurrency) {
    return new http.Agent({ keepAlive: true, maxSockets: concurrency });
}

function sendOnce(agent, args, ep, headers) {
    return new Promise((resolve) => {
        const startedAt = process.hrtime.bigint();
        let settled = false;
        const finish = (status) => {
            if (settled) return;
            settled = true;
            resolve({
                status,
                ms: Number(process.hrtime.bigint() - startedAt) / 1e6,
            });
        };
        const req = http.request(
            {
                hostname: args.host,
                port: args.port,
                path: ep.path,
                method: ep.method,
                headers,
                agent,
                timeout: args.requestTimeoutMs,
            },
            (res) => {
                res.resume();
                res.on('end', () => finish(res.statusCode));
                res.on('error', () => finish(0));
            }
        );
        req.on('error', () => finish(0));
        req.on('timeout', () => {
            req.destroy();
            finish(0);
        });
        if (ep.body) req.write(ep.body);
        req.end();
    });
}

async function runLoad(agent, args, ep, headers, durationMs, concurrency) {
    const latencies = [];
    const statuses = new Map();
    let transportErrors = 0;
    const deadline = Date.now() + durationMs;
    const startedAt = Date.now();

    async function worker() {
        while (Date.now() < deadline) {
            const r = await sendOnce(agent, args, ep, headers);
            if (r.status === 0) {
                transportErrors += 1;
            } else {
                latencies.push(r.ms);
                statuses.set(r.status, (statuses.get(r.status) || 0) + 1);
            }
        }
    }

    await Promise.all(Array.from({ length: concurrency }, () => worker()));
    const elapsedSec = (Date.now() - startedAt) / 1000;
    latencies.sort((a, b) => a - b);

    const ok = [...statuses.entries()]
        .filter(([code]) => code >= 200 && code < 400)
        .reduce((sum, [, count]) => sum + count, 0);
    const completed = latencies.length;
    const attempts = completed + transportErrors;

    return {
        elapsed_s: round(elapsedSec, 2),
        attempts,
        completed,
        ok,
        transport_errors: transportErrors,
        status_distribution: Object.fromEntries([...statuses.entries()].sort((a, b) => a[0] - b[0])),
        qps_ok: round(elapsedSec > 0 ? ok / elapsedSec : 0, 1),
        error_rate_pct: round(attempts > 0 ? ((attempts - ok) / attempts) * 100 : 100, 3),
        p50_ms: round(percentile(latencies, 0.5), 3),
        p90_ms: round(percentile(latencies, 0.9), 3),
        p95_ms: round(percentile(latencies, 0.95), 3),
        p99_ms: round(percentile(latencies, 0.99), 3),
        p999_ms: round(percentile(latencies, 0.999), 3),
        max_ms: round(latencies.length > 0 ? latencies[latencies.length - 1] : 0, 3),
        latencies,
    };
}

/** 最近秩分位数（覆盖全部已完成响应） */
function percentile(sorted, p) {
    if (sorted.length === 0) return 0;
    const idx = Math.min(sorted.length - 1, Math.ceil(p * sorted.length) - 1);
    return sorted[Math.max(0, idx)];
}

function round(n, digits) {
    const f = 10 ** digits;
    return Math.round(n * f) / f;
}

function median(values) {
    if (values.length === 0) return 0;
    const sorted = [...values].sort((a, b) => a - b);
    const mid = Math.floor(sorted.length / 2);
    return sorted.length % 2 === 0 ? (sorted[mid - 1] + sorted[mid]) / 2 : sorted[mid];
}

function mean(values) {
    return values.length === 0 ? 0 : values.reduce((a, b) => a + b, 0) / values.length;
}

function stddev(values) {
    if (values.length < 2) return 0;
    const m = mean(values);
    return Math.sqrt(mean(values.map((v) => (v - m) ** 2)));
}

function cvPct(values) {
    const m = mean(values);
    return m === 0 ? 0 : (stddev(values) / m) * 100;
}

// ============================================================================
// 主流程
// ============================================================================

async function measureEndpoint(args, ep, auth) {
    const agent = createAgent(args.concurrency);
    const headers = headersFor(ep, auth);

    process.stdout.write(
        `  [${ep.id}] 预热 ${args.warmupMs / 1000}s @ c=${args.concurrency} ... `
    );
    const warm = await runLoad(agent, args, ep, headers, args.warmupMs, args.concurrency);
    console.log(`完成 (${warm.attempts} req, ${warm.error_rate_pct}% err)`);

    const reps = [];
    for (let i = 0; i < args.reps; i++) {
        process.stdout.write(
            `  [${ep.id}] 第 ${i + 1}/${args.reps} 次: ${args.durationMs / 1000}s @ c=${args.concurrency} ... `
        );
        const r = await runLoad(agent, args, ep, headers, args.durationMs, args.concurrency);
        delete r.latencies;
        reps.push(r);
        console.log(
            `QPS=${r.qps_ok} P50=${r.p50_ms} P90=${r.p90_ms} P95=${r.p95_ms} P99=${r.p99_ms} P99.9=${r.p999_ms} err=${r.error_rate_pct}% (${r.attempts} req)`
        );
    }
    agent.destroy();

    const summary = {
        warmup: { attempts: warm.attempts, error_rate_pct: warm.error_rate_pct },
        concurrency: args.concurrency,
        duration_s: args.durationMs / 1000,
        reps,
        qps_median: round(median(reps.map((r) => r.qps_ok)), 1),
        qps_cv_pct: round(cvPct(reps.map((r) => r.qps_ok)), 2),
        p50_median_ms: round(median(reps.map((r) => r.p50_ms)), 3),
        p90_median_ms: round(median(reps.map((r) => r.p90_ms)), 3),
        p95_median_ms: round(median(reps.map((r) => r.p95_ms)), 3),
        p99_median_ms: round(median(reps.map((r) => r.p99_ms)), 3),
        p999_median_ms: round(median(reps.map((r) => r.p999_ms)), 3),
        max_error_rate_pct: round(Math.max(...reps.map((r) => r.error_rate_pct)), 3),
    };
    return summary;
}

function loadBaselineFile() {
    if (!fs.existsSync(BASELINE_PATH)) return null;
    return JSON.parse(fs.readFileSync(BASELINE_PATH, 'utf8'));
}

function validateBaselineShape(doc) {
    const problems = [];
    if (!doc || typeof doc !== 'object') return ['基线文件不是 JSON 对象'];
    if (doc.schema !== 2) problems.push(`schema=${doc.schema}（期望 2）`);
    const fps = doc.fingerprints;
    if (!fps || typeof fps !== 'object' || Object.keys(fps).length === 0) {
        problems.push('fingerprints 为空（需先运行 --init）');
        return problems;
    }
    for (const [key, entry] of Object.entries(fps)) {
        if (!entry || typeof entry !== 'object') {
            problems.push(`${key}: 条目非对象`);
            continue;
        }
        if (!entry.endpoints || typeof entry.endpoints !== 'object') {
            problems.push(`${key}: 缺少 endpoints`);
            continue;
        }
        for (const [epId, s] of Object.entries(entry.endpoints)) {
            if (typeof s.qps_median !== 'number' || typeof s.p99_median_ms !== 'number') {
                problems.push(`${key}/${epId}: 缺少 qps_median 或 p99_median_ms`);
            }
        }
    }
    return problems;
}

function persistBaseline(key, fp, results, args) {
    const doc =
        loadBaselineFile() ||
        {
            schema: 2,
            regression_threshold_pct: args.qpsThresholdPct,
            p99_threshold_pct: args.p99ThresholdPct,
            fingerprints: {},
        };
    doc.schema = 2;
    doc.regression_threshold_pct = args.qpsThresholdPct;
    doc.p99_threshold_pct = args.p99ThresholdPct;
    doc.fingerprints = doc.fingerprints || {};
    doc.fingerprints[key] = {
        fingerprint: fp,
        measured_at: new Date().toISOString(),
        concurrency: args.concurrency,
        warmup_s: args.warmupMs / 1000,
        duration_s: args.durationMs / 1000,
        reps: args.reps,
        endpoints: results,
    };
    fs.writeFileSync(BASELINE_PATH, JSON.stringify(doc, null, 2) + '\n', 'utf8');
    console.log(`\n[perf-regression] 基线已写入 ${BASELINE_PATH}（指纹 ${key}）`);
}

function evaluate(current, base, args) {
    const rows = [];
    const reasons = [];
    // ≥2 次重复同时回归才判失败（reps=1 时降级为单次判定，reps=2/3 时要求 2 次）
    const minRegressedReps = Math.min(args.reps, 2);

    for (const [epId, cur] of Object.entries(current)) {
        const b = base.endpoints[epId];
        if (!b) {
            reasons.push(`${epId}: 基线缺少该端点`);
            rows.push({ endpoint: epId, status: 'NO_BASELINE' });
            continue;
        }
        // 阈值自适应：3 次重复的 QPS 本身就有测量噪声，噪声大于阈值时固定 5% 会持续误报
        // （评测实测 /health CV 7.3%、业务端点 CV 22.7%，5% 阈值误报率 24.8%/41.4%）。
        // 取 max(名义阈值, 3×CV)，上限 30% 防止噪声把门禁彻底失效。
        const adaptivePct = Math.min(30, cur.qps_cv_pct * 3);
        const effectivePct = Math.max(args.qpsThresholdPct, adaptivePct);
        const threshold = b.qps_median * (1 - effectivePct / 100);
        const regressedReps = cur.reps.filter((r) => r.qps_ok < threshold).length;
        const qpsBelow = cur.qps_median < threshold;
        const qpsFail = qpsBelow && regressedReps >= minRegressedReps;

        const p99Limit = b.p99_median_ms * (1 + args.p99ThresholdPct / 100);
        const p99Fail = cur.p99_median_ms > p99Limit;
        const errFail = cur.max_error_rate_pct > args.maxErrorRatePct;

        if (qpsFail) {
            reasons.push(
                `${epId}: QPS 中位数 ${cur.qps_median} < 阈值 ${round(threshold, 1)}` +
                    `（有效阈值 ${round(effectivePct, 2)}%（名义 ${args.qpsThresholdPct}% / 3×CV ${round(adaptivePct, 2)}%），` +
                    `${regressedReps}/${cur.reps.length} 次重复低于阈值）`
            );
        }
        if (p99Fail) {
            reasons.push(
                `${epId}: P99 中位数 ${cur.p99_median_ms}ms > 上限 ${round(p99Limit, 3)}ms`
            );
        }
        if (errFail) {
            reasons.push(
                `${epId}: 错误率 ${cur.max_error_rate_pct}% > 上限 ${args.maxErrorRatePct}%`
            );
        }

        rows.push({
            endpoint: epId,
            status: qpsFail || p99Fail || errFail ? 'FAIL' : 'PASS',
            qps_current: cur.qps_median,
            qps_baseline: round(b.qps_median, 1),
            qps_delta_pct: round(((cur.qps_median - b.qps_median) / b.qps_median) * 100, 2),
            qps_cv_pct: cur.qps_cv_pct,
            qps_threshold_pct_effective: round(effectivePct, 2),
            regressed_reps: regressedReps,
            p99_current_ms: cur.p99_median_ms,
            p99_baseline_ms: round(b.p99_median_ms, 3),
            p99_delta_pct: round(
                ((cur.p99_median_ms - b.p99_median_ms) / b.p99_median_ms) * 100,
                2
            ),
            error_rate_pct: cur.max_error_rate_pct,
        });
    }

    for (const [epId] of Object.entries(base.endpoints)) {
        if (!current[epId] && !args.skipBusiness) {
            reasons.push(`${epId}: 本次未测量（基线要求覆盖）`);
        }
    }

    return { rows, reasons, failed: reasons.length > 0 };
}

function printReport(rows, reasons, args, fp, key) {
    console.log('\n=============== 性能回归报告（闭环，中位数口径） ===============');
    console.log(`环境指纹: ${key} | ${fp.cpu_model} × ${fp.logical_cpus} LP | ${fp.mem_gib} GiB | node${fp.node_major}`);
    console.log(
        `测量参数: c=${args.concurrency} warmup=${args.warmupMs / 1000}s duration=${args.durationMs / 1000}s reps=${args.reps}`
    );
    console.log('');
    console.log(
        '| 端点 | 状态 | QPS(中位) | 基线 | Δ% | CV% | 有效阈值% | 回归次数 | P99(中位) | 基线 | Δ% | 错误率% |'
    );
    console.log(
        '|------|------|-----------|------|----|-----|-----------|----------|-----------|------|----|---------|'
    );
    for (const r of rows) {
        if (r.status === 'NO_BASELINE') {
            console.log(`| ${r.endpoint} | NO_BASELINE | - | - | - | - | - | - | - | - | - | - |`);
            continue;
        }
        console.log(
            `| ${r.endpoint} | ${r.status} | ${r.qps_current} | ${r.qps_baseline} | ${r.qps_delta_pct} | ${r.qps_cv_pct} | ${r.qps_threshold_pct_effective} | ${r.regressed_reps} | ${r.p99_current_ms} | ${r.p99_baseline_ms} | ${r.p99_delta_pct} | ${r.error_rate_pct} |`
        );
    }
    console.log('=============================================================');
    for (const r of rows) {
        if (r.qps_cv_pct !== undefined && r.qps_cv_pct > args.maxCvPct) {
            console.warn(
                `[perf-regression] WARN: ${r.endpoint} QPS CV=${r.qps_cv_pct}% > ${args.maxCvPct}%，测量噪声偏大，结论置信度下降`
            );
        }
    }
    if (reasons.length > 0) {
        console.error('\n[perf-regression] FAIL:');
        for (const reason of reasons) console.error(`  - ${reason}`);
    } else {
        console.log('\n[perf-regression] PASS: 所有端点在中位数口径下未超过阈值');
    }
}

async function main() {
    const args = parseArgs(process.argv);

    if (args.skipBench) {
        const doc = loadBaselineFile();
        if (!doc) {
            console.error(`[perf-regression] FAIL: 基线文件不存在 (${BASELINE_PATH})`);
            process.exit(1);
        }
        if (!doc || doc.schema !== 2) {
            console.error(
                `[perf-regression] FAIL: schema=${doc ? doc.schema : 'missing'}（期望 2），请先运行 --init`
            );
            process.exit(1);
        }
        const keys = Object.keys(doc.fingerprints || {});
        if (keys.length === 0) {
            // 结构合法但无实测基线：不谎称门禁已生效
            console.warn(
                '[perf-regression] WARN: GATE INACTIVE —— 基线文件尚无任何环境指纹，' +
                    '门禁不会执行任何比较。请在目标机器运行 `--init` 后提交基线。'
            );
            process.exit(0);
        }
        const problems = validateBaselineShape(doc);
        if (problems.length > 0) {
            console.error('[perf-regression] FAIL: 基线文件不合法');
            for (const p of problems) console.error(`  - ${p}`);
            process.exit(1);
        }
        console.log(
            `[perf-regression] PASS: 基线 schema=2，含 ${keys.length} 个环境指纹（${keys.join(', ')}）`
        );
        process.exit(0);
    }

    if (!isLoopback(args.host) && !args.allowNonLoopback) {
        console.error(
            `[perf-regression] 拒绝执行: 目标 ${args.host} 不是回环地址。` +
                ' 压测只允许本机实例；确需远端请显式传 --allow-non-loopback 并自担风险。'
        );
        process.exit(2);
    }

    const fp = buildFingerprint();
    const key = fingerprintKey(fp);
    const auth = buildAuthContext(args);
    const endpoints = ENDPOINTS.filter((ep) => !ep.auth || !args.skipBusiness);

    if (!args.skipBusiness && !auth.token) {
        console.error(
            '[perf-regression] FAIL: 业务端点需要鉴权。请提供 --jwt <token> 或 --jwt-secret <SZ300_JWT_SECRET>；' +
                ' 若确实只测 /health，显式传 --skip-business。'
        );
        process.exit(2);
    }

    console.log('[perf-regression] 性能回归门禁 v2');
    console.log(`  目标: http://${args.host}:${args.port}`);
    console.log(`  指纹: ${key}`);
    console.log(
        `  端点: ${endpoints.map((e) => `${e.method} ${e.path}`).join(', ')}`
    );
    console.log(
        `  参数: c=${args.concurrency} warmup=${args.warmupMs / 1000}s duration=${args.durationMs / 1000}s reps=${args.reps}`
    );

    const current = {};
    for (const ep of endpoints) {
        current[ep.id] = await measureEndpoint(args, ep, auth);
    }

    if (args.json) {
        fs.writeFileSync(
            args.json,
            JSON.stringify({ fingerprint: fp, fingerprint_key: key, args, results: current }, null, 2) + '\n',
            'utf8'
        );
        console.log(`[perf-regression] 结果已写入 ${args.json}`);
    }

    if (args.init) {
        persistBaseline(key, fp, current, args);
        process.exit(0);
    }

    const doc = loadBaselineFile();
    if (!doc) {
        console.error(
            `[perf-regression] FAIL: 基线文件不存在 (${BASELINE_PATH})，请先运行 --init`
        );
        process.exit(3);
    }
    const problems = validateBaselineShape(doc);
    if (problems.length > 0) {
        console.error('[perf-regression] FAIL: 基线文件不合法');
        for (const p of problems) console.error(`  - ${p}`);
        process.exit(3);
    }
    const entry = doc.fingerprints[key];
    if (!entry) {
        console.error(
            `[perf-regression] 当前环境指纹 ${key} 无基线。已知指纹: ${Object.keys(doc.fingerprints).join(', ')}。` +
                ' 首次在新环境运行请执行 --init 并把基线提交入库。'
        );
        process.exit(3);
    }

    const { rows, reasons, failed } = evaluate(current, entry, args);
    printReport(rows, reasons, args, fp, key);
    process.exit(failed ? 1 : 0);
}

main().catch((err) => {
    console.error('[perf-regression] FATAL:', err && err.stack ? err.stack : err);
    process.exit(2);
});
