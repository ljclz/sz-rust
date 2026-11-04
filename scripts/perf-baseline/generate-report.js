import fs from 'fs';
import path from 'path';
import { execSync } from 'child_process';

export async function generateReport(benchmarkResults, resourceSamples, healthStatus, metadata = {}) {
    const date = new Date().toISOString().slice(0, 10);
    const reportPath = path.resolve(import.meta.dirname, '..', '..', 'docs', 'audit', `${date}-perf-baseline.md`);

    const szRustVersion = metadata.szRustVersion ?? _tryGitVersion();
    const heyVersion = metadata.heyVersion ?? 'unknown';
    const hardwareSpec = metadata.hardwareSpec ?? '腾讯云 CVM 2C4G (122.51.216.76)';
    const duration = metadata.duration ?? '30s';
    const qpsCap = metadata.qpsCap ?? 2000;
    const benchCommand = metadata.benchCommand ?? `ab -c <并发> -t ${duration} -n 10000000 <url>`;

    const lines = [
        `# 性能基线报告 ${date}`,
        '',
        '> 所有数字均来自 ab 原始 stdout（归档于 artifacts/perf-baseline-' + date.replace(/-/g, '') + '/），禁止估算。',
        '',
        '## 1. 环境信息',
        '',
        `| 字段 | 值 |`,
        `|------|-----|`,
        `| sz-rust 版本 | ${szRustVersion} |`,
        `| 压测工具 | ${heyVersion} |`,
        `| 硬件规格 | ${hardwareSpec} |`,
        `| QPS 上限 | ${qpsCap}（ab 不支持限速，自然吞吐） |`,
        `| 单次持续时间 | ${duration} |`,
        `| 压测命令 | \`${benchCommand}\` |`,
        '',
        '## 2. 压测结果',
        '',
        `| 端点 | 并发 | QPS | P50(ms) | P95(ms) | P99(ms) | 错误率(%) | 总请求 |`,
        `|------|------|-----|---------|---------|---------|-----------|--------|`,
    ];

    for (const r of benchmarkResults) {
        if (r.error) {
            lines.push(`| ${r.endpoint} | ${r.concurrency} | ERROR | - | - | - | - | - |`);
        } else {
            lines.push(`| ${r.endpoint} | ${r.concurrency} | ${r.qps.toFixed(1)} | ${r.p50Ms.toFixed(2)} | ${r.p95Ms.toFixed(2)} | ${r.p99Ms.toFixed(2)} | ${r.errorRate.toFixed(2)} | ${r.totalReqs} |`);
        }
    }

    lines.push('', '## 3. 资源采样', '');
    if (resourceSamples) {
        lines.push(
            `| 字段 | 值 |`,
            `|------|-----|`,
            `| 峰值 RSS (MB) | ${resourceSamples.peakRssMb?.toFixed(1) ?? 'N/A'} |`,
            `| 平均 CPU (%) | ${resourceSamples.avgCpuPercent?.toFixed(1) ?? 'N/A'} |`,
            `| 采样次数 | ${resourceSamples.sampleCount ?? 0} |`,
            `| 采样间隔 | 2s |`,
        );
    } else {
        lines.push('N/A（未采集）');
    }

    lines.push('', '## 4. 健康探测', '');
    if (healthStatus) {
        lines.push(
            `| 字段 | 值 |`,
            `|------|-----|`,
            `| 探测次数 | ${healthStatus.history?.length ?? 0} |`,
            `| 健康率 (%) | ${healthStatus.healthyRate?.toFixed(1) ?? 'N/A'} |`,
            `| 最终状态 | ${healthStatus.unhealthy ? '不健康（已中止）' : '健康'} |`,
        );
    } else {
        lines.push('N/A（未探测）');
    }

    lines.push('', '## 5. 可复现性', '');
    lines.push(`- **压测命令**：\`${benchCommand}\``);
    lines.push(`- **生成时间**：${new Date().toISOString()}`);
    lines.push(`- **数据来源**：ab stdout 原始输出（归档 artifacts/perf-baseline-${date.replace(/-/g, '')}/）`);

    fs.mkdirSync(path.dirname(reportPath), { recursive: true });
    fs.writeFileSync(reportPath, lines.join('\n') + '\n', 'utf-8');
    return { reportPath, fieldCount: 12 };
}

function _tryGitVersion() {
    try { return execSync('git rev-parse --short HEAD', { encoding: 'utf-8' }).trim(); } catch { return 'unknown'; }
}

const isMain = process.argv[1] && import.meta.url === `file://${process.argv[1].replace(/\\/g, '/')}`;
if (isMain) {
    generateReport(
        JSON.parse(process.argv[2] ?? '[]'),
        JSON.parse(process.argv[3] ?? 'null'),
        JSON.parse(process.argv[4] ?? 'null'),
        JSON.parse(process.argv[5] ?? '{}'),
    ).then(r => { console.log(r.reportPath); process.exit(0); })
        .catch(e => { console.error(e.message); process.exit(1); });
}