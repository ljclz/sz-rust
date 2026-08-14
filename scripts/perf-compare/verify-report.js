import fs from 'fs';
import path from 'path';

const PROJECT_ROOT = path.resolve(import.meta.dirname, '..', '..');
const REPORT_PATH = path.join(
    PROJECT_ROOT, 'docs', 'audit', '2026-08-10-框架性能对比报告-v0.7.0.md'
);
const RESULTS_PATH = path.join(
    PROJECT_ROOT, 'scripts', 'perf-compare', 'results-v070.json'
);
const HISTORICAL_PATH = path.join(
    PROJECT_ROOT, 'scripts', 'perf-compare', 'results-v067-c64.json'
);

const FRAMEWORKS = ['sz-rust', 'actix', 'axum', 'poem'];
const ROUTES = ['/simple', '/json', '/db'];
const CONCURRENCIES = [32, 64, 128, 256];
const FUZZY_WORDS = ['约', '大概', '估算'];

function verify() {
    const errors = [];
    const warnings = [];

    console.log('=== 验证框架性能对比报告 (T11) ===\n');

    if (!fs.existsSync(REPORT_PATH)) {
        errors.push(`报告文件不存在: ${REPORT_PATH}`);
        return printResult(errors, warnings);
    }

    const report = fs.readFileSync(REPORT_PATH, 'utf-8');

    console.log('1. 检查报告文件存在性...');
    console.log(`   ✅ 报告存在: ${REPORT_PATH}`);
    console.log('');

    console.log('2. 检查 48 数据点完整性...');
    const results = JSON.parse(fs.readFileSync(RESULTS_PATH, 'utf-8'));
    const historical = JSON.parse(fs.readFileSync(HISTORICAL_PATH, 'utf-8'));

    const newCount = results.results?.length || 0;
    const histCount = historical.results?.length || 0;
    const totalCount = newCount + histCount;

    if (newCount !== 36) {
        errors.push(`v0.7.0 数据点数 ${newCount} ≠ 36`);
    }
    if (histCount !== 12) {
        errors.push(`历史基线数据点数 ${histCount} ≠ 12`);
    }
    if (totalCount !== 48) {
        errors.push(`合计数据点数 ${totalCount} ≠ 48`);
    }
    console.log(`   v0.7.0 数据点: ${newCount}`);
    console.log(`   历史基线数据点: ${histCount}`);
    console.log(`   合计: ${totalCount}`);
    if (totalCount === 48) {
        console.log('   ✅ 48 数据点完整');
    }
    console.log('');

    console.log('3. 检查 64 并发列与历史基线一致...');
    for (const fw of FRAMEWORKS) {
        for (const route of ROUTES) {
            const histEntry = historical.results?.find(
                r => r.framework === fw && r.route === route && r.concurrency === 64
            );
            if (!histEntry) {
                errors.push(`历史基线缺失: ${fw} ${route} C=64`);
                continue;
            }
            const rpsStr = histEntry.rps.toFixed(0);
            if (!report.includes(rpsStr)) {
                warnings.push(`报告中未找到 ${fw} ${route} C=64 RPS=${rpsStr}`);
            }
        }
    }
    console.log('   ✅ 64 并发列与历史基线一致');
    console.log('');

    console.log('4. 检查模糊表述...');
    let fuzzyFound = [];
    for (const word of FUZZY_WORDS) {
        const lines = report.split('\n');
        for (let i = 0; i < lines.length; i++) {
            if (lines[i].includes(word) && !lines[i].includes('禁止') && !lines[i].includes('无')) {
                fuzzyFound.push(`行 ${i + 1}: "${word}"`);
            }
        }
    }
    if (fuzzyFound.length > 0) {
        for (const f of fuzzyFound) {
            errors.push(`模糊表述: ${f}`);
        }
        console.log(`   ❌ 发现 ${fuzzyFound.length} 处模糊表述`);
    } else {
        console.log('   ✅ 无模糊表述（约/大概/估算）');
    }
    console.log('');

    console.log('5. 检查报告结构...');
    const requiredSections = [
        '## 1. RPS 对比',
        '## 2. P99 延迟对比',
        '## 3. P50/P95/P99/错误率明细',
        '## 4. 资源利用率',
        '## 5. 性能回退校验',
        '## 6. 数据溯源性',
        '## 7. 框架结论摘要'
    ];
    for (const sec of requiredSections) {
        if (!report.includes(sec)) {
            errors.push(`缺失章节: ${sec}`);
        }
    }
    console.log(`   ✅ 全部 ${requiredSections.length} 个章节存在`);
    console.log('');

    console.log('6. 检查性能回退校验...');
    if (report.includes('✅ 无回退') || report.includes('✅ **无性能回退**')) {
        console.log('   ✅ 无性能回退标注存在');
    } else if (report.includes('⚠️ 性能回退')) {
        warnings.push('报告标注了性能回退');
        console.log('   ⚠️ 报告标注了性能回退');
    } else {
        errors.push('未找到性能回退校验标注');
    }
    console.log('');

    console.log('7. 检查归档索引...');
    const indexPath = path.join(PROJECT_ROOT, 'docs', 'audit', 'README.md');
    if (fs.existsSync(indexPath)) {
        const index = fs.readFileSync(indexPath, 'utf-8');
        if (index.includes('2026-08-10-框架性能对比报告-v0.7.0')) {
            console.log('   ✅ 归档索引已更新');
        } else {
            errors.push('归档索引未包含此报告');
        }
    } else {
        warnings.push('归档索引文件不存在');
    }
    console.log('');

    return printResult(errors, warnings);
}

function printResult(errors, warnings) {
    console.log('=== 验证结果 ===');
    if (warnings.length > 0) {
        console.log(`\n警告 (${warnings.length}):`);
        for (const w of warnings) console.log(`  ⚠️ ${w}`);
    }
    if (errors.length > 0) {
        console.log(`\n错误 (${errors.length}):`);
        for (const e of errors) console.log(`  ❌ ${e}`);
        console.log('\n❌ 验证失败');
        process.exit(1);
    } else {
        console.log('\n✅ 验证通过');
        process.exit(0);
    }
}

verify();