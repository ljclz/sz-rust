#!/usr/bin/env node
/**
 * 覆盖率缺口定位脚本
 *
 * 解析 Cobertura XML，提取指定 crate 的未覆盖行清单 + 上下文代码片段。
 *
 * 用法：
 *   node scripts/audit/coverage-gap-locator.js --xml cobertura.xml --crate sz-rust-orm-facade
 */

const fs = require('fs');
const path = require('path');

function parseArgs(argv) {
  const args = { xml: null, crate: null };
  for (let i = 2; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--xml') args.xml = argv[++i];
    else if (arg === '--crate') args.crate = argv[++i];
    else if (arg === '--help' || arg === '-h') {
      console.log('用法: node coverage-gap-locator.js --xml <path> --crate <name>');
      process.exit(0);
    }
  }
  if (!args.xml || !args.crate) {
    console.error('错误: --xml 和 --crate 参数必填');
    process.exit(1);
  }
  return args;
}

function locateGaps(xmlPath, crateName) {
  const xml = fs.readFileSync(xmlPath, 'utf-8');
  const packageRegex = new RegExp(
    `<package\\s+name="${crateName}"[^>]*>([\\s\\S]*?)<\\/package>`, 'g'
  );

  const pkgMatch = packageRegex.exec(xml);
  if (!pkgMatch) {
    console.error(`错误: crate "${crateName}" 不在 XML 中`);
    process.exit(1);
  }

  const pkgContent = pkgMatch[1];
  const classRegex = /<class\s+filename="([^"]+)"[^>]*>([\s\S]*?)<\/class>/g;
  const lineRegex = /<line\s+number="(\d+)"[^>]*hits="(\d+)"/g;

  const uncoveredFiles = [];
  let totalGapLines = 0;

  let classMatch;
  while ((classMatch = classRegex.exec(pkgContent)) !== null) {
    const filename = classMatch[1];
    const classContent = classMatch[2];

    const uncoveredLines = [];
    let lineMatch;
    while ((lineMatch = lineRegex.exec(classContent)) !== null) {
      if (parseInt(lineMatch[2], 10) === 0) {
        uncoveredLines.push(parseInt(lineMatch[1], 10));
      }
    }

    if (uncoveredLines.length === 0) continue;

    totalGapLines += uncoveredLines.length;

    const lineRanges = compressRanges(uncoveredLines);
    const surroundingContext = extractContext(filename, uncoveredLines);

    uncoveredFiles.push({
      path: filename,
      uncoveredCount: uncoveredLines.length,
      lineRanges,
      surroundingContext,
    });
  }

  return { crate: crateName, uncoveredFiles, totalGapLines };
}

function compressRanges(lines) {
  if (lines.length === 0) return [];
  const ranges = [];
  let start = lines[0];
  let end = lines[0];
  for (let i = 1; i < lines.length; i++) {
    if (lines[i] === end + 1) {
      end = lines[i];
    } else {
      ranges.push([start, end]);
      start = lines[i];
      end = lines[i];
    }
  }
  ranges.push([start, end]);
  return ranges;
}

function extractContext(filename, uncoveredLines) {
  const fullPath = path.resolve(filename);
  if (!fs.existsSync(fullPath)) return null;

  const content = fs.readFileSync(fullPath, 'utf-8').split('\n');
  const snippets = [];

  for (const lineNum of uncoveredLines.slice(0, 10)) {
    const start = Math.max(0, lineNum - 4);
    const end = Math.min(content.length - 1, lineNum + 3);
    const snippet = [];
    for (let i = start; i <= end; i++) {
      const marker = i + 1 === lineNum ? ' >>> ' : '     ';
      snippet.push(`${marker}${i + 1}: ${content[i]}`);
    }
    snippets.push({ line: lineNum, code: snippet.join('\n') });
  }

  return snippets;
}

function main() {
  const args = parseArgs(process.argv);
  if (!fs.existsSync(args.xml)) {
    console.error(`错误: XML 文件不存在: ${args.xml}`);
    process.exit(1);
  }
  const result = locateGaps(args.xml, args.crate);

  console.log(`\n=== ${result.crate} 覆盖率缺口 ===`);
  console.log(`总未覆盖行: ${result.totalGapLines}`);
  console.log(`未覆盖文件: ${result.uncoveredFiles.length}\n`);

  for (const file of result.uncoveredFiles) {
    console.log(`📄 ${file.path} (${file.uncoveredCount} 行未覆盖)`);
    const ranges = file.lineRanges.map(r => r[0] === r[1] ? `${r[0]}` : `${r[0]}-${r[1]}`);
    console.log(`   行范围: ${ranges.join(', ')}`);
    if (file.surroundingContext) {
      for (const s of file.surroundingContext) {
        console.log(`\n   --- 第 ${s.line} 行上下文 ---`);
        console.log(s.code.split('\n').map(l => '   ' + l).join('\n'));
      }
    }
    console.log('');
  }

  console.log('\n' + JSON.stringify({ crate: result.crate, totalGapLines: result.totalGapLines, fileCount: result.uncoveredFiles.length }, null, 2));
}

main();