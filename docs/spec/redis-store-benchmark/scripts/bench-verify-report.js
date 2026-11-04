import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPORT_PATH = path.resolve(__dirname, '..', 'benchmark-report.md');

function verifyReport() {
    let ok = true;

    if (!fs.existsSync(REPORT_PATH)) {
        console.log('❌ 报告文件不存在:', REPORT_PATH);
        process.exit(1);
    }

    const content = fs.readFileSync(REPORT_PATH, 'utf-8');

    // 1. 8 章节齐全
    const sections = ['整体结论', '指标汇总表', '分并发度详细表', 'file:line 证据表', '资源占用', 'Soak', '清理确认', '阻断项清单'];
    for (const s of sections) {
        if (content.includes(s)) { console.log(`✅ 章节: ${s}`); } else { console.log(`❌ 章节: ${s} 缺失`); ok = false; }
    }

    // 2. PERF 红线对照表 11 行
    const perfLines = content.match(/PERF-\d+/g) || [];
    console.log(`${perfLines.length >= 11 ? '✅' : '❌'} PERF 红线: ${perfLines.length} 条 (期望 11)`);
    if (perfLines.length < 11) ok = false;

    // 3. Go/No-Go 判定
    if (content.includes('✅') || content.includes('❌')) {
        console.log('✅ Go/No-Go 判定存在');
    } else {
        console.log('❌ Go/No-Go 判定缺失');
        ok = false;
    }

    // 4. 证据 file:line
    if (content.includes('redis_store.rs')) {
        console.log('✅ file:line 证据存在');
    } else {
        console.log('❌ file:line 证据缺失');
        ok = false;
    }

    // 5. 内嵌 JSON
    if (content.includes('```json')) {
        console.log('✅ 内嵌 JSON 存在');
    } else {
        console.log('❌ 内嵌 JSON 缺失');
        ok = false;
    }

    console.log(ok ? '\n✅ 报告验证全部通过' : '\n❌ 报告验证存在失败项');
    process.exit(ok ? 0 : 1);
}

verifyReport();