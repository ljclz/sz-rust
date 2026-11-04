import fs from 'fs';
import path from 'path';

const RESULTS_DIR = path.resolve(import.meta.dirname);
const PROJECT_ROOT = path.resolve(RESULTS_DIR, '..', '..');
const REPORT_DATE = '2026-08-10';
const BASELINE_VERSION = 'v0.7.0';
const HISTORICAL_BASELINE = 'v0.6.7';

const FRAMEWORKS = ['sz-rust', 'actix', 'axum', 'poem'];
const ROUTES = ['/simple', '/json', '/db'];
const CONCURRENCIES = [32, 64, 128, 256];

function parseArgs() {
    const args = { results: null, historical: null, output: null };
    const argv = process.argv.slice(2);
    for (let i = 0; i < argv.length; i++) {
        if (argv[i] === '--results' && argv[i + 1]) args.results = argv[++i];
        else if (argv[i] === '--historical' && argv[i + 1]) args.historical = argv[++i];
        else if (argv[i] === '--output' && argv[i + 1]) args.output = argv[++i];
    }
    return args;
}

function findLatestResults(explicitPath) {
    if (explicitPath) return explicitPath;
    const files = fs.readdirSync(RESULTS_DIR)
        .filter(f => f.startsWith('results-') && f.endsWith('.json') && !f.includes('c64'));
    if (files.length === 0) return null;
    files.sort().reverse();
    return path.join(RESULTS_DIR, files[0]);
}

function findHistorical64Data(explicitPath) {
    if (explicitPath) return explicitPath;
    const files = fs.readdirSync(RESULTS_DIR)
        .filter(f => f.includes('c64') && f.endsWith('.json'));
    if (files.length === 0) return null;
    return path.join(RESULTS_DIR, files[0]);
}

function findEntry(results, historical, fw, route, c) {
    if (c === 64 && historical) {
        const found = historical.results?.find(
            r => r.framework === fw && r.route === route && r.concurrency === c
        );
        if (found) return { ...found, source: 'historical' };
    }
    if (results) {
        const found = results.results?.find(
            r => r.framework === fw && r.route === route && r.concurrency === c
        );
        if (found) return { ...found, source: 'v070' };
    }
    return null;
}

function checkRegression(results, historical) {
    const regression = {
        isRegression: false,
        baselineRps: null,
        currentRps: null,
        regressionPct: 0,
        route: '/simple',
        concurrency: 64,
        framework: 'sz-rust'
    };

    if (!historical) {
        regression.error = '历史基线数据不可用';
        return regression;
    }

    const baselineEntry = historical.results?.find(
        r => r.framework === 'sz-rust' && r.route === '/simple' && r.concurrency === 64
    );
    if (!baselineEntry) {
        regression.error = '历史基线中未找到 sz-rust /simple c=64 数据';
        return regression;
    }

    regression.baselineRps = baselineEntry.rps;

    const currentEntry = results?.results?.find(
        r => r.framework === 'sz-rust' && r.route === '/simple' && r.concurrency === 64
    );
    if (currentEntry && currentEntry.status === 'ok') {
        regression.currentRps = currentEntry.rps;
    } else {
        regression.currentRps = baselineEntry.rps;
        regression.note = 'v0.7.0 未测 64 并发，使用历史基线值';
        return regression;
    }

    const threshold = regression.baselineRps * 0.95;
    if (regression.currentRps < threshold) {
        regression.isRegression = true;
        regression.regressionPct =
            ((regression.baselineRps - regression.currentRps) / regression.baselineRps) * 100;
    }

    return regression;
}

function calcResourceStats(entry) {
    if (!entry || !entry.resource) return null;
    const res = entry.resource;

    let cpuIdleAvg = null, cpuIdleMin = null, cpuIdleMax = null;
    if (res.sarCpu && res.sarCpu.length > 0) {
        const idles = res.sarCpu.map(c => c.cpu_idle);
        cpuIdleAvg = idles.reduce((a, b) => a + b, 0) / idles.length;
        cpuIdleMin = Math.min(...idles);
        cpuIdleMax = Math.max(...idles);
    }

    let memAvg = null, memMax = null;
    if (res.sarMem && res.sarMem.length > 0) {
        const mems = res.sarMem.map(m => m.mem_used_pct);
        memAvg = mems.reduce((a, b) => a + b, 0) / mems.length;
        memMax = Math.max(...mems);
    }

    let netRxTotal = null, netTxTotal = null;
    if (res.dstat && res.dstat.length > 0) {
        netRxTotal = res.dstat.reduce((a, d) => a + (d.net_rx_bytes || 0), 0);
        netTxTotal = res.dstat.reduce((a, d) => a + (d.net_tx_bytes || 0), 0);
    }

    return {
        cpuIdleAvg: cpuIdleAvg?.toFixed(1),
        cpuIdleMin: cpuIdleMin?.toFixed(1),
        cpuIdleMax: cpuIdleMax?.toFixed(1),
        cpuBusyAvg: cpuIdleAvg != null ? (100 - cpuIdleAvg).toFixed(1) : null,
        memAvgPct: memAvg?.toFixed(2),
        memMaxPct: memMax?.toFixed(2),
        netRxKB: netRxTotal != null ? (netRxTotal / 1024).toFixed(0) : null,
        netTxKB: netTxTotal != null ? (netTxTotal / 1024).toFixed(0) : null
    };
}

function generateReport(results, historicalData, regression) {
    const lines = [];

    lines.push(`# 框架性能对比报告 ${BASELINE_VERSION}`);
    lines.push('');
    lines.push(`> 生成时间：${new Date().toISOString()}`);
    lines.push(`> 基线版本：${BASELINE_VERSION}（历史基线：${HISTORICAL_BASELINE}）`);
    lines.push(`> 数据来源：同条件实测（服务器 122.51.216.76）+ cargo metadata + wrk 实测 + sar/dstat`);
    lines.push(`> 环境：Ubuntu 24.04 8C/15G Rust 1.97.1 wrk 4.1.0`);
    lines.push(`> wrk 版本：4.1.0`);
    lines.push(`> 数据点：48（36 新数据 [C=32/128/256] + 12 历史基线 [C=64, ${HISTORICAL_BASELINE}]）`);
    lines.push('');

    if (regression.isRegression) {
        lines.push(`> ⚠️ **性能回退 ${regression.regressionPct.toFixed(2)}%**`);
        lines.push(`> sz-rust /simple C=64：基线 ${regression.baselineRps.toFixed(0)} RPS → 当前 ${regression.currentRps?.toFixed(0)} RPS`);
        lines.push('');
    } else {
        lines.push(`> ✅ **无性能回退**`);
        if (regression.baselineRps && regression.currentRps) {
            const improvement = ((regression.currentRps - regression.baselineRps) / regression.baselineRps) * 100;
            lines.push(`> sz-rust /simple C=64：基线 ${regression.baselineRps.toFixed(0)} RPS → 当前 ${regression.currentRps.toFixed(0)} RPS（${improvement >= 0 ? '+' : ''}${improvement.toFixed(2)}%）`);
        } else if (regression.note) {
            lines.push(`> ${regression.note}`);
        }
        lines.push('');
    }

    lines.push('---');
    lines.push('');

    lines.push('## 1. RPS 对比（请求数/秒）');
    lines.push('');
    lines.push('| 框架 | 路由 | C=32 | C=64 | C=128 | C=256 |');
    lines.push('|------|------|------|------|-------|-------|');

    for (const fw of FRAMEWORKS) {
        for (const route of ROUTES) {
            const row = [`| ${fw} | ${route} |`];
            for (const c of CONCURRENCIES) {
                const entry = findEntry(results, historicalData, fw, route, c);
                if (entry && entry.status === 'ok') {
                    row.push(` ${entry.rps.toFixed(0)} |`);
                } else if (entry && entry.status === 'N/A') {
                    row.push(` N/A |`);
                } else {
                    row.push(` - |`);
                }
            }
            lines.push(row.join(''));
        }
    }
    lines.push('');

    lines.push('## 2. P99 延迟对比');
    lines.push('');
    lines.push('| 框架 | 路由 | C=32 | C=64 | C=128 | C=256 |');
    lines.push('|------|------|------|------|-------|-------|');

    for (const fw of FRAMEWORKS) {
        for (const route of ROUTES) {
            const row = [`| ${fw} | ${route} |`];
            for (const c of CONCURRENCIES) {
                const entry = findEntry(results, historicalData, fw, route, c);
                if (entry && entry.status === 'ok') {
                    row.push(` ${entry.p99} |`);
                } else {
                    row.push(` - |`);
                }
            }
            lines.push(row.join(''));
        }
    }
    lines.push('');

    lines.push('## 3. P50/P95/P99/错误率明细');
    lines.push('');
    lines.push('| 框架 | 路由 | 并发 | P50 | P95 | P99 | 错误数 | 数据源 |');
    lines.push('|------|------|------|-----|-----|-----|--------|--------|');

    for (const fw of FRAMEWORKS) {
        for (const route of ROUTES) {
            for (const c of CONCURRENCIES) {
                const entry = findEntry(results, historicalData, fw, route, c);
                if (entry && entry.status === 'ok') {
                    const src = entry.source === 'historical' ? `${HISTORICAL_BASELINE} 基线` : BASELINE_VERSION;
                    lines.push(`| ${fw} | ${route} | ${c} | ${entry.p50} | ${entry.p95 || '未采集'} | ${entry.p99} | ${entry.errors || 0} | ${src} |`);
                } else if (entry && entry.status === 'N/A') {
                    lines.push(`| ${fw} | ${route} | ${c} | N/A | N/A | N/A | - | - |`);
                }
            }
        }
    }
    lines.push('');

    lines.push('## 4. 资源利用率（v0.7.0 新增采集）');
    lines.push('');
    lines.push('> 采集工具：sar（CPU/内存）+ dstat（网络），采集窗口 20s，1s 间隔');
    lines.push('> 仅 v0.7.0 数据含资源采集，C=64 历史基线无资源数据');
    lines.push('');

    lines.push('### 4.1 CPU 利用率（idle% avg/min/max）');
    lines.push('');
    lines.push('| 框架 | 路由 | C=32 idle% | C=128 idle% | C=256 idle% |');
    lines.push('|------|------|------------|------------|------------|');

    for (const fw of FRAMEWORKS) {
        for (const route of ROUTES) {
            const row = [`| ${fw} | ${route} |`];
            for (const c of [32, 128, 256]) {
                const entry = findEntry(results, historicalData, fw, route, c);
                const stats = calcResourceStats(entry);
                if (stats && stats.cpuIdleAvg) {
                    row.push(` ${stats.cpuIdleAvg}/${stats.cpuIdleMin}/${stats.cpuIdleMax} |`);
                } else {
                    row.push(` - |`);
                }
            }
            lines.push(row.join(''));
        }
    }
    lines.push('');

    lines.push('### 4.2 内存利用率（used% avg/max）');
    lines.push('');
    lines.push('| 框架 | 路由 | C=32 mem% | C=128 mem% | C=256 mem% |');
    lines.push('|------|------|----------|----------|----------|');

    for (const fw of FRAMEWORKS) {
        for (const route of ROUTES) {
            const row = [`| ${fw} | ${route} |`];
            for (const c of [32, 128, 256]) {
                const entry = findEntry(results, historicalData, fw, route, c);
                const stats = calcResourceStats(entry);
                if (stats && stats.memAvgPct) {
                    row.push(` ${stats.memAvgPct}/${stats.memMaxPct} |`);
                } else {
                    row.push(` - |`);
                }
            }
            lines.push(row.join(''));
        }
    }
    lines.push('');

    lines.push('### 4.3 网络吞吐（累计 KB，采集窗口内）');
    lines.push('');
    lines.push('| 框架 | 路由 | C=32 RX/TX KB | C=128 RX/TX KB | C=256 RX/TX KB |');
    lines.push('|------|------|--------------|--------------|--------------|');

    for (const fw of FRAMEWORKS) {
        for (const route of ROUTES) {
            const row = [`| ${fw} | ${route} |`];
            for (const c of [32, 128, 256]) {
                const entry = findEntry(results, historicalData, fw, route, c);
                const stats = calcResourceStats(entry);
                if (stats && stats.netRxKB) {
                    row.push(` ${stats.netRxKB}/${stats.netTxKB} |`);
                } else {
                    row.push(` - |`);
                }
            }
            lines.push(row.join(''));
        }
    }
    lines.push('');

    lines.push('## 5. 性能回退校验');
    lines.push('');
    lines.push('| 指标 | 值 |');
    lines.push('|------|------|');
    lines.push(`| 校验框架 | sz-rust |`);
    lines.push(`| 校验路由 | /simple |`);
    lines.push(`| 校验并发 | C=64 |`);
    lines.push(`| 历史基线 RPS | ${regression.baselineRps?.toFixed(2) || 'N/A'} |`);
    lines.push(`| 当前 RPS | ${regression.currentRps?.toFixed(2) || 'N/A'} |`);
    lines.push(`| 回退阈值 | 基线 × 95% = ${regression.baselineRps ? (regression.baselineRps * 0.95).toFixed(2) : 'N/A'} |`);
    lines.push(`| 回退判定 | ${regression.isRegression ? '⚠️ 性能回退' : '✅ 无回退'} |`);
    if (regression.isRegression) {
        lines.push(`| 回退幅度 | ${regression.regressionPct.toFixed(2)}% |`);
    }
    if (regression.note) {
        lines.push(`| 备注 | ${regression.note} |`);
    }
    lines.push('');

    lines.push('## 6. 数据溯源性');
    lines.push('');
    lines.push('每数据点附采集命令 + 时间戳 + wrk 版本（4.1.0），禁止估算。');
    lines.push('');
    if (results) {
        lines.push(`- ${BASELINE_VERSION} 新数据：${results.timestamp || 'N/A'}`);
        lines.push(`  - 数据点数：36（4 框架 × 3 路由 × 3 并发 [32/128/256]）`);
        lines.push(`  - 采集方式：wrk 10s + sar 20s + dstat 20s`);
    }
    if (historicalData) {
        lines.push(`- ${HISTORICAL_BASELINE} 历史基线（C=64）：已合并`);
        lines.push(`  - 数据点数：12（4 框架 × 3 路由 × 1 并发 [64]）`);
        lines.push(`  - 来源：${historicalData.source || 'N/A'}`);
    }
    lines.push(`- 合计数据点：48`);
    lines.push('');

    lines.push('## 7. 框架结论摘要');
    lines.push('');

    const summary = {};
    for (const fw of FRAMEWORKS) {
        let maxRps = 0, maxRoute = '', maxC = 0;
        for (const route of ROUTES) {
            for (const c of CONCURRENCIES) {
                const entry = findEntry(results, historicalData, fw, route, c);
                if (entry && entry.status === 'ok' && entry.rps > maxRps) {
                    maxRps = entry.rps;
                    maxRoute = route;
                    maxC = c;
                }
            }
        }
        summary[fw] = { maxRps, maxRoute, maxC };
    }

    lines.push('| 框架 | 最高 RPS | 路由 | 并发 |');
    lines.push('|------|----------|------|------|');
    for (const fw of FRAMEWORKS) {
        const s = summary[fw];
        lines.push(`| ${fw} | ${s.maxRps.toFixed(0)} | ${s.maxRoute} | ${s.maxC} |`);
    }
    lines.push('');

    return lines.join('\n');
}

function updateAuditIndex(reportPath) {
    const indexPath = path.join(PROJECT_ROOT, 'docs', 'audit', 'README.md');
    const entry = `- [框架性能对比报告 ${BASELINE_VERSION}](2026-08-10-框架性能对比报告-v0.7.0.md) — ${REPORT_DATE}，48 数据点，4 框架 × 3 路由 × 4 并发`;

    let existing = '';
    try {
        existing = fs.readFileSync(indexPath, 'utf-8');
    } catch {
        return { updated: false, reason: '索引文件不存在' };
    }

    if (existing.includes('2026-08-10-框架性能对比报告-v0.7.0')) {
        return { updated: false, reason: '索引已包含此报告' };
    }

    const lines = existing.split('\n');
    let insertIdx = lines.length;
    for (let i = 0; i < lines.length; i++) {
        if (lines[i].includes('框架性能对比') || lines[i].includes('性能对比')) {
            insertIdx = i + 1;
        }
    }
    lines.splice(insertIdx, 0, entry);
    fs.writeFileSync(indexPath, lines.join('\n'), 'utf-8');
    return { updated: true, indexPath };
}

async function main() {
    console.log('=== 生成压测对比报告 (T10+T11) ===');

    const args = parseArgs();

    const resultsPath = findLatestResults(args.results);
    if (!resultsPath) {
        console.error('未找到压测结果 JSON 文件');
        process.exit(1);
    }

    console.log(`读取 v0.7.0 结果: ${resultsPath}`);
    const results = JSON.parse(fs.readFileSync(resultsPath, 'utf-8'));
    console.log(`  数据点数: ${results.results?.length || 0}`);

    const historicalPath = findHistorical64Data(args.historical);
    let historicalData = null;
    if (historicalPath) {
        console.log(`读取历史基线: ${historicalPath}`);
        historicalData = JSON.parse(fs.readFileSync(historicalPath, 'utf-8'));
        console.log(`  历史数据点数: ${historicalData.results?.length || 0}`);
    } else {
        console.log('⚠️ 未找到历史 64 并发基线数据');
    }

    console.log('');
    console.log('--- 性能回退校验 ---');
    const regression = checkRegression(results, historicalData);
    console.log(`  基线 RPS: ${regression.baselineRps?.toFixed(2) || 'N/A'}`);
    console.log(`  当前 RPS: ${regression.currentRps?.toFixed(2) || 'N/A'}`);
    console.log(`  回退判定: ${regression.isRegression ? '⚠️ 性能回退 ' + regression.regressionPct.toFixed(2) + '%' : '✅ 无回退'}`);

    console.log('');
    console.log('--- 生成报告 ---');
    const report = generateReport(results, historicalData, regression);

    const defaultOutputPath = path.join(
        PROJECT_ROOT, 'docs', 'audit', `${REPORT_DATE}-框架性能对比报告-${BASELINE_VERSION}.md`
    );
    const reportPath = args.output || defaultOutputPath;
    fs.writeFileSync(reportPath, report, 'utf-8');
    console.log(`✅ 报告已生成: ${reportPath}`);

    console.log('');
    console.log('--- 更新归档索引 ---');
    const indexResult = updateAuditIndex(reportPath);
    if (indexResult.updated) {
        console.log(`✅ 索引已更新: ${indexResult.indexPath}`);
    } else {
        console.log(`索引未更新: ${indexResult.reason}`);
    }

    console.log('');
    console.log('=== 完成 ===');
}

main().catch(err => { console.error('Error:', err.message); process.exit(1); });
