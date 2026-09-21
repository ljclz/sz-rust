#!/usr/bin/env node
/**
 * 分 crate 覆盖率分析脚本
 *
 * 解析 Cobertura XML，按 crate 拆分覆盖率，校验门槛。
 *
 * 用法：
 *   node scripts/audit/per-crate-coverage.js --xml cobertura.xml --threshold 85
 *   node scripts/audit/per-crate-coverage.js --xml cobertura.xml --threshold 85 --exempt docs/audit/coverage-exemption.md
 */

const fs = require('fs');
const path = require('path');

function parseArgs(argv) {
  const args = { threshold: 85, xml: null, exempt: null };
  for (let i = 2; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--xml') args.xml = argv[++i];
    else if (arg === '--threshold') args.threshold = parseInt(argv[++i], 10);
    else if (arg === '--exempt') args.exempt = argv[++i];
    else if (arg === '--help' || arg === '-h') {
      console.log('用法: node per-crate-coverage.js --xml <path> --threshold <N> [--exempt <path>]');
      process.exit(0);
    }
  }
  if (!args.xml) {
    console.error('错误: --xml 参数必填');
    process.exit(1);
  }
  return args;
}

function parseExemptList(exemptPath) {
  if (!exemptPath || !fs.existsSync(exemptPath)) return new Set();
  const content = fs.readFileSync(exemptPath, 'utf-8');
  const crates = new Set();
  const lines = content.split('\n');
  for (const line of lines) {
    const match = line.match(/^\|\s*([\w-]+)\s*\|/);
    if (match && match[1] !== 'crate_name' && match[1] !== '_(空清单') {
      crates.add(match[1]);
    }
  }
  return crates;
}

function analyzeCobertura(xmlPath, threshold, exemptSet) {
  const xml = fs.readFileSync(xmlPath, 'utf-8');
  const packageRegex = /<package\s+name="([^"]+)"[^>]*>([\s\S]*?)<\/package>/g;
  const classRegex = /<class\s+filename="([^"]+)"[^>]*>([\s\S]*?)<\/class>/g;
  const lineRegex = /<line\s+number="(\d+)"[^>]*hits="(\d+)"/g;

  const crates = [];
  let match;
  while ((match = packageRegex.exec(xml)) !== null) {
    const pkgName = match[1];
    const pkgContent = match[2];

    let totalLines = 0;
    let coveredLines = 0;

    let classMatch;
    while ((classMatch = classRegex.exec(pkgContent)) !== null) {
      const classContent = classMatch[2];
      let lineMatch;
      while ((lineMatch = lineRegex.exec(classContent)) !== null) {
        totalLines++;
        if (parseInt(lineMatch[2], 10) > 0) coveredLines++;
      }
    }

    if (totalLines === 0) continue;

    const coveragePct = (coveredLines / totalLines) * 100;
    const gapLines = Math.max(0, Math.ceil(threshold / 100 * totalLines) - coveredLines);
    const exempted = exemptSet.has(pkgName);
    const pass = exempted || coveragePct >= threshold;

    crates.push({
      name: pkgName,
      execLines: totalLines,
      coveredLines,
      coveragePct: parseFloat(coveragePct.toFixed(2)),
      gapLines,
      pass,
      exempted,
    });
  }

  const overallPass = crates.every(c => c.pass);
  return { crates, overallPass };
}

function printTable(result, threshold) {
  const { crates, overallPass } = result;
  console.log('\n┌────────────────────────────────────────────────────────────────────────────────┐');
  console.log('│ Crate                          │  Exec  │ Covered │   %    │  Gap  │ Status  │');
  console.log('├────────────────────────────────────────────────────────────────────────────────┤');
  for (const c of crates) {
    const status = c.exempted ? 'EXEMPT ' : c.pass ? '  OK   ' : ' FAIL  ';
    const name = c.name.padEnd(30).slice(0, 30);
    const exec = String(c.execLines).padStart(6);
    const cov = String(c.coveredLines).padStart(7);
    const pct = (c.coveragePct.toFixed(1) + '%').padStart(6);
    const gap = String(c.gapLines).padStart(5);
    console.log(`│ ${name} │ ${exec} │ ${cov} │ ${pct} │ ${gap} │ ${status} │`);
  }
  console.log('└────────────────────────────────────────────────────────────────────────────────┘');
  console.log(`\n门槛: ${threshold}% | 总: ${crates.length} crate | 通过: ${crates.filter(c => c.pass).length} | 失败: ${crates.filter(c => !c.pass).length} | 豁免: ${crates.filter(c => c.exempted).length}`);
  console.log(`总体: ${overallPass ? '✅ PASS' : '❌ FAIL'}\n`);
}

function main() {
  const args = parseArgs(process.argv);
  if (!fs.existsSync(args.xml)) {
    console.error(`错误: XML 文件不存在: ${args.xml}`);
    process.exit(1);
  }
  const exemptSet = parseExemptList(args.exempt);
  const result = analyzeCobertura(args.xml, args.threshold, exemptSet);
  printTable(result, args.threshold);
  console.log(JSON.stringify(result, null, 2));
  process.exit(result.overallPass ? 0 : 1);
}

main();