import fs from 'fs';
import path from 'path';
import { EvidenceCollector } from './evidence-collector.js';

export async function generateReport(moduleResults, cleanResult, config, projectRoot, reportPath) {
    const evidenceCollector = new EvidenceCollector(projectRoot);
    const allEvidences = [];

    for (const result of moduleResults) {
        if (result.evidences) {
            allEvidences.push(...result.evidences);
        }
    }

    const verifyResult = await evidenceCollector.verifyAll(allEvidences);

    const lines = [];
    lines.push('# 服务器真实数据全链路验证报告');
    lines.push('');
    lines.push(`> **验证时间**: ${new Date().toISOString()}`);
    lines.push(`> **基线版本**: sz-rust v${config.szRustVersion}`);
    lines.push(`> **目标服务器**: ${config.server.host}`);
    lines.push(`> **sz-orm 版本**: ${config.szOrmVersion}`);
    lines.push('');

    const overallPassed = moduleResults.every(r => r.passed) && cleanResult.passed;
    lines.push(`## 整体结论: ${overallPassed ? '✅ 可上生产' : '❌ 不可上生产'}`);
    lines.push('');

    lines.push('## 验证项结论');
    lines.push('');
    lines.push('| 模块 | 结论 | 耗时(ms) | 错误数 |');
    lines.push('|------|------|----------|--------|');
    for (const result of moduleResults) {
        lines.push(`| ${result.module} | ${result.passed ? '✅ 通过' : '❌ 失败'} | ${result.duration} | ${result.errors?.length || 0} |`);
    }
    lines.push(`| Cleaner | ${cleanResult.passed ? '✅ 通过' : '❌ 失败'} | ${cleanResult.duration} | ${cleanResult.failed?.length || 0} |`);
    lines.push('');

    lines.push('## file:line 证据');
    lines.push('');
    lines.push(`| 结论 | 文件 | 行号 | 校验 |`);
    lines.push(`|------|------|------|------|`);
    for (const evidence of allEvidences) {
        lines.push(`| ${evidence.conclusion} | ${evidence.file} | ${evidence.line} | ${evidence.verified ? '✅' : '❌ ' + (evidence.verifyError || '')} |`);
    }
    lines.push('');
    lines.push(`**证据校验统计**: 总计 ${verifyResult.total} 条，通过 ${verifyResult.passed} 条，失败 ${verifyResult.failed.length} 条`);
    lines.push('');

    lines.push('## 错误详情');
    lines.push('');
    let hasErrors = false;
    for (const result of moduleResults) {
        if (result.errors && result.errors.length > 0) {
            hasErrors = true;
            lines.push(`### ${result.module}`);
            lines.push('');
            for (const err of result.errors) {
                lines.push(`- **错误类型**: ${err.error || 'UNKNOWN'}`);
                lines.push(`  **详情**: ${err.detail || JSON.stringify(err)}`);
                if (err.database) lines.push(`  **数据库**: ${err.database}`);
                if (err.app) lines.push(`  **应用**: ${err.app}`);
                lines.push('');
            }
        }
    }
    if (!hasErrors) {
        lines.push('无错误');
        lines.push('');
    }

    lines.push('## 清理确认');
    lines.push('');
    lines.push('| 产物 | 状态 |');
    lines.push('|------|------|');
    for (const item of cleanResult.cleaned) {
        lines.push(`| ${item.artifact} | ✅ ${item.status} |`);
    }
    for (const item of cleanResult.failed) {
        lines.push(`| ${item.artifact} | ❌ ${item.reason} |`);
    }
    lines.push('');

    if (!overallPassed) {
        lines.push('## 阻断项清单');
        lines.push('');
        for (const result of moduleResults) {
            if (!result.passed && result.errors) {
                for (const err of result.errors) {
                    lines.push(`- **${result.module}**: ${err.error || 'UNKNOWN'} — ${err.detail || ''}`);
                }
            }
        }
        if (!cleanResult.passed) {
            for (const item of cleanResult.failed) {
                lines.push(`- **Cleaner**: ${item.artifact} — ${item.reason}`);
            }
        }
        lines.push('');
    }

    lines.push('## 进程状态');
    lines.push('');
    const deployResult = moduleResults.find(r => r.module === 'Deploy');
    if (deployResult && deployResult.processes) {
        lines.push('| 应用 | PID | 端口 | RSS(KB) | 启动时间 |');
        lines.push('|------|-----|------|---------|----------|');
        for (const proc of deployResult.processes) {
            lines.push(`| ${proc.name} | ${proc.pid} | ${proc.port} | ${proc.rssKB} | ${proc.startedAt} |`);
        }
    } else {
        lines.push('无进程信息');
    }
    lines.push('');

    const reportContent = lines.join('\n');
    fs.writeFileSync(reportPath, reportContent, 'utf-8');

    return {
        reportPath,
        overallPassed,
        evidenceVerifyResult: verifyResult,
    };
}