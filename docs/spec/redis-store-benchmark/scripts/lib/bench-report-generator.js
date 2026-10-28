import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = path.resolve(__dirname, '..', '..', '..', '..');

/**
 * 生成压测报告（8 章节）
 */
export async function generateBenchReport({ roundResults, soakResult, poolResult, cleanResult, config, projectRoot, reportPath }) {
    const lines = [];
    const ts = new Date().toISOString();

    lines.push(`# Redis 存储后端压测报告`);
    lines.push(``);
    lines.push(`> **生成时间**: ${ts}`);
    lines.push(`> **基线版本**: sz-rust v0.6.7`);
    lines.push(`> **被测文件**: packages/sz-rust-auth-facade/src/redis_store.rs (646 行)`);
    lines.push(`> **目标服务器**: ${config.server.host} (Redis 127.0.0.1:6379)`);
    lines.push('');

    // ① 整体结论
    const { overallPassed, blockers } = judgeGoNoGo(roundResults, soakResult, poolResult, cleanResult);
    lines.push(`## 1. 整体结论`);
    lines.push('');
    lines.push(overallPassed ? `✅ **可上生产** — 所有性能红线达标，无阻断项。` : `❌ **阻断** — 存在 ${blockers.length} 个阻断项，不可上生产。`);
    lines.push('');

    // ② 指标汇总表
    lines.push(`## 2. 指标汇总表（PERF 红线对照）`);
    lines.push('');
    lines.push(`| 红线编号 | 操作 | 并发 | 阈值QPS | 实测QPS | 阈值p99(ms) | 实测p99(ms) | 阈值错误率 | 实测错误率 | 判定 |`);
    lines.push(`|----------|------|------|---------|---------|-------------|-------------|-----------|-----------|------|`);
    for (const rl of config.perfRedLines) {
        const rr = roundResults.find(r => r.operation === rl.operation && r.concurrency === rl.concurrency);
        if (rr) {
            const verdict = rr.verdict === 'pass' ? '✅' : '❌';
            lines.push(`| ${rl.id} | ${rl.operation} | ${rl.concurrency} | ${rl.qpsMin} | ${rr.qps.toFixed(0)} | ${rl.p99MaxMs} | ${rr.latency_p99_ms.toFixed(2)} | ${(rl.errorRateMax * 100).toFixed(2)}% | ${(rr.error_rate * 100).toFixed(3)}% | ${verdict} |`);
        } else {
            lines.push(`| ${rl.id} | ${rl.operation} | ${rl.concurrency} | ${rl.qpsMin} | N/A | ${rl.p99MaxMs} | N/A | ${(rl.errorRateMax * 100).toFixed(2)}% | N/A | ⚠️ |`);
        }
    }
    lines.push('');

    // ③ 分并发度详细表
    lines.push(`## 3. 分并发度详细表`);
    lines.push('');
    lines.push(`| 操作 | 并发 | QPS | p50(ms) | p95(ms) | p99(ms) | 错误率 | 耗时(s) | 判定 |`);
    lines.push(`|------|------|-----|---------|---------|---------|--------|---------|------|`);
    for (const rr of roundResults) {
        lines.push(`| ${rr.operation} | ${rr.concurrency} | ${rr.qps.toFixed(0)} | ${rr.latency_p50_ms.toFixed(2)} | ${rr.latency_p95_ms.toFixed(2)} | ${rr.latency_p99_ms.toFixed(2)} | ${(rr.error_rate * 100).toFixed(3)}% | ${rr.duration_secs.toFixed(2)} | ${rr.verdict} |`);
    }
    lines.push('');

    // ④ file:line 证据表
    lines.push(`## 4. file:line 证据表`);
    lines.push('');
    lines.push(`| 操作 | 证据文件 | 证据行号 |`);
    lines.push(`|------|----------|----------|`);
    for (const rr of roundResults) {
        lines.push(`| ${rr.operation} | ${rr.evidence_file} | ${rr.evidence_line} |`);
    }
    lines.push('');

    // ⑤ 资源占用曲线
    lines.push(`## 5. 资源占用`);
    lines.push('');
    lines.push(`| 操作 | RSS峰值(KB) | RSS起始(KB) |`);
    lines.push(`|------|-------------|-------------|`);
    for (const rr of roundResults) {
        lines.push(`| ${rr.operation} | ${rr.rss_peak_kb} | ${rr.rss_start_kb} |`);
    }
    lines.push('');

    // ⑥ Soak 10 段快照
    if (soakResult && soakResult.snapshots) {
        lines.push(`## 6. Soak 10 段快照`);
        lines.push('');
        lines.push(`| 分钟 | QPS | p99(ms) | RSS(KB) | 错误率 |`);
        lines.push(`|------|-----|---------|---------|--------|`);
        for (const snap of soakResult.snapshots) {
            lines.push(`| ${snap.minute_index} | ${snap.qps.toFixed(0)} | ${snap.latency_p99_ms.toFixed(2)} | ${snap.rss_kb} | ${(snap.error_rate * 100).toFixed(3)}% |`);
        }
        lines.push('');
        lines.push(`- qps_stable: ${soakResult.qps_stable ? '✅' : '❌'}`);
        lines.push(`- p99_stable: ${soakResult.p99_stable ? '✅' : '❌'}`);
        lines.push(`- memory_ok: ${soakResult.memory_ok ? '✅' : '❌'} (peak=${soakResult.rss_peak_kb}KB, start=${soakResult.rss_start_kb}KB, end=${soakResult.rss_end_kb}KB)`);
        lines.push('');
    }

    // ⑦ 清理确认
    lines.push(`## 7. 清理确认`);
    lines.push('');
    if (cleanResult) {
        for (const item of cleanResult.cleaned) {
            lines.push(`- ✅ ${item.artifact}: ${item.status}`);
        }
        for (const item of cleanResult.failed) {
            lines.push(`- ❌ ${item.artifact}: ${item.reason}`);
        }
    }
    lines.push('');

    // ⑧ 阻断项清单
    lines.push(`## 8. 阻断项清单`);
    lines.push('');
    if (blockers.length === 0) {
        lines.push(`无阻断项。`);
    } else {
        lines.push(`| 操作 | 并发 | 红线编号 | 实测值 | 阈值 |`);
        lines.push(`|------|------|----------|--------|------|`);
        for (const b of blockers) {
            lines.push(`| ${b.operation} | ${b.concurrency} | ${b.redLineId} | ${b.actual} | ${b.threshold} |`);
        }
    }
    lines.push('');

    // 内嵌 JSON 块
    lines.push('```json');
    lines.push(JSON.stringify({ roundResults, soakResult, poolResult, overallPassed, blockers, timestamp: ts }, null, 2));
    lines.push('```');

    const content = lines.join('\n');
    try {
        fs.writeFileSync(reportPath, content, 'utf-8');
    } catch {
        console.log(content);
    }
    return { overallPassed, blockers };
}

/**
 * Go/No-Go 判定
 */
export function judgeGoNoGo(roundResults, soakResult, poolResult, cleanResult) {
    const blockers = [];
    let overallPassed = true;

    for (const rr of roundResults) {
        if (rr.verdict !== 'pass') {
            overallPassed = false;
            blockers.push({
                operation: rr.operation,
                concurrency: rr.concurrency,
                redLineId: 'PERF',
                actual: `qps=${rr.qps.toFixed(0)},p99=${rr.latency_p99_ms.toFixed(2)}ms,err=${(rr.error_rate * 100).toFixed(3)}%`,
                threshold: 'see config',
            });
        }
    }

    if (soakResult) {
        if (!soakResult.qps_stable || !soakResult.p99_stable || !soakResult.memory_ok) {
            overallPassed = false;
            blockers.push({ operation: 'soak', concurrency: 0, redLineId: 'SOAK', actual: `qps_stable=${soakResult.qps_stable},p99_stable=${soakResult.p99_stable},memory_ok=${soakResult.memory_ok}`, threshold: 'all=true' });
        }
    }

    if (poolResult && poolResult.service_unavailable_rate > 0.001) {
        overallPassed = false;
        blockers.push({ operation: 'pool_stability', concurrency: 0, redLineId: 'POOL', actual: `su_rate=${poolResult.service_unavailable_rate}`, threshold: '<=0.001' });
    }

    if (cleanResult && cleanResult.failed.length > 0) {
        overallPassed = false;
        blockers.push({ operation: 'cleanup', concurrency: 0, redLineId: 'CLEAN', actual: `${cleanResult.failed.length} failures`, threshold: '0' });
    }

    return { overallPassed, blockers };
}