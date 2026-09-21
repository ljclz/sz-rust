import fs from 'fs';
import path from 'path';

const PROJECT_ROOT = path.resolve(import.meta.dirname, '..');

function check(name, condition, detail) {
    const status = condition ? '✅' : '❌';
    console.log(`  ${status} ${name}${detail ? ': ' + detail : ''}`);
    return condition;
}

async function main() {
    console.log('=== 全链路验证流水线 (T14) ===\n');
    let allPass = true;

    console.log('1. 发布准备验证');
    const orderPath = path.join(PROJECT_ROOT, 'scripts/publish/publish-order.json');
    if (fs.existsSync(orderPath)) {
        const order = JSON.parse(fs.readFileSync(orderPath, 'utf-8'));
        allPass &= check('publish-order.json 存在', true, `${order.length} 个 crate`);
        allPass &= check('crate 数量 = 29', order.length === 29);
    } else {
        allPass &= check('publish-order.json 存在', false);
    }
    console.log('');

    console.log('2. 发布执行验证');
    const summaryPath = path.join(PROJECT_ROOT, 'scripts/publish/publish-summary.json');
    if (fs.existsSync(summaryPath)) {
        const summary = JSON.parse(fs.readFileSync(summaryPath, 'utf-8'));
        allPass &= check('publish-summary.json 存在', true);
        const verified = summary.records?.filter(r => r.status === 'verified').length || 0;
        allPass &= check('全部 verified', verified === 29, `${verified}/29`);
    } else {
        allPass &= check('publish-summary.json 存在', false);
    }

    const auditLogs = fs.readdirSync(path.join(PROJECT_ROOT, 'scripts/publish'))
        .filter(f => f.startsWith('audit-log-') && f.endsWith('.jsonl'));
    allPass &= check('审计日志存在', auditLogs.length > 0, `${auditLogs.length} 个文件`);
    console.log('');

    console.log('3. 压测结果验证');
    const resultsPath = path.join(PROJECT_ROOT, 'scripts/perf-compare/results-v070.json');
    if (fs.existsSync(resultsPath)) {
        const results = JSON.parse(fs.readFileSync(resultsPath, 'utf-8'));
        allPass &= check('results-v070.json 存在', true);
        allPass &= check('36 条记录', results.results?.length === 36, `${results.results?.length} 条`);
        const okCount = results.results?.filter(r => r.status === 'ok').length || 0;
        allPass &= check('全部 status=ok', okCount === 36, `${okCount}/36`);
    } else {
        allPass &= check('results-v070.json 存在', false);
    }
    console.log('');

    console.log('4. 报告生成验证');
    const reportPath = path.join(PROJECT_ROOT, 'docs/audit/2026-08-10-框架性能对比报告-v0.7.0.md');
    allPass &= check('框架对比报告存在', fs.existsSync(reportPath));
    if (fs.existsSync(reportPath)) {
        const report = fs.readFileSync(reportPath, 'utf-8');
        allPass &= check('含 48 数据点', report.includes('48'));
        allPass &= check('无性能回退', report.includes('无性能回退') || report.includes('无回退'));
    }
    console.log('');

    console.log('5. 深度评估验证');
    const assessmentPath = path.join(PROJECT_ROOT, 'docs/audit/archive/2026-08/2026-08-10-项目深度评估与框架对比报告.md');
    allPass &= check('深度评估文档存在', fs.existsSync(assessmentPath));
    if (fs.existsSync(assessmentPath)) {
        const assessment = fs.readFileSync(assessmentPath, 'utf-8');
        allPass &= check('基线 = v0.7.0', assessment.includes('v0.7.0'));
        allPass &= check('日期 = 2026-08-10', assessment.includes('2026-08-10'));
        allPass &= check('代码行数 121,212', assessment.includes('121,212'));
        allPass &= check('测试函数 4,610', assessment.includes('4,610'));
    }
    console.log('');

    console.log('6. 文档同步验证');
    const readme = fs.readFileSync(path.join(PROJECT_ROOT, 'README.md'), 'utf-8');
    allPass &= check('README.md 含 v0.7.0', readme.includes('v0.7.0'));
    const changelog = fs.readFileSync(path.join(PROJECT_ROOT, 'CHANGELOG.md'), 'utf-8');
    allPass &= check('CHANGELOG.md 含 [0.7.0]', changelog.includes('## [0.7.0]'));
    const roadmap = fs.readFileSync(path.join(PROJECT_ROOT, 'docs/audit/archive/2026-08/roadmap.md'), 'utf-8');
    allPass &= check('roadmap.md 含 v0.7.0', roadmap.includes('v0.7.0'));
    console.log('');

    console.log('7. 归档索引验证');
    const indexPath = path.join(PROJECT_ROOT, 'docs/audit/README.md');
    if (fs.existsSync(indexPath)) {
        const index = fs.readFileSync(indexPath, 'utf-8');
        allPass &= check('索引含 v0.7.0 报告', index.includes('2026-08-10-框架性能对比报告-v0.7.0'));
    } else {
        allPass &= check('归档索引存在', false);
    }
    console.log('');

    console.log('8. 历史基线数据验证');
    const histPath = path.join(PROJECT_ROOT, 'scripts/perf-compare/results-v067-c64.json');
    if (fs.existsSync(histPath)) {
        const hist = JSON.parse(fs.readFileSync(histPath, 'utf-8'));
        allPass &= check('历史基线 JSON 存在', true, `${hist.results?.length} 条 C=64 数据`);
        allPass &= check('12 条历史数据', hist.results?.length === 12);
    } else {
        allPass &= check('历史基线 JSON 存在', false);
    }
    console.log('');

    console.log('=== 验证结果 ===');
    if (allPass) {
        console.log('✅ 全链路验证通过');
        process.exit(0);
    } else {
        console.log('❌ 存在失败项');
        process.exit(1);
    }
}

main().catch(err => { console.error('Error:', err.message); process.exit(1); });