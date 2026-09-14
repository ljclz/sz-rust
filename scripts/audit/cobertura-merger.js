#!/usr/bin/env node
/**
 * Cobertura XML 合并器
 *
 * 合并多个分片 Cobertura XML 为单一 workspace 级 XML。
 *
 * 用法：
 *   node scripts/audit/cobertura-merger.js --inputs "coverage-*.xml" --output cobertura-workspace.xml
 */

const fs = require('fs');
const path = require('path');
const glob = require('path');

function parseArgs(argv) {
  const args = { inputs: null, output: null };
  for (let i = 2; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--inputs') args.inputs = argv[++i];
    else if (arg === '--output') args.output = argv[++i];
    else if (arg === '--help' || arg === '-h') {
      console.log('用法: node cobertura-merger.js --inputs <glob> --output <path>');
      process.exit(0);
    }
  }
  if (!args.inputs || !args.output) {
    console.error('错误: --inputs 和 --output 参数必填');
    process.exit(1);
  }
  return args;
}

function expandGlob(pattern) {
  const dir = path.dirname(pattern);
  const base = path.basename(pattern);
  const regex = new RegExp('^' + base.replace(/\./g, '\\.').replace(/\*/g, '.*') + '$');
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir)
    .filter(f => regex.test(f))
    .map(f => path.join(dir, f));
}

function mergeCobertura(inputFiles) {
  const packages = new Map();

  for (const file of inputFiles) {
    if (!fs.existsSync(file)) {
      console.warn(`警告: 跳过不存在的文件: ${file}`);
      continue;
    }
    const xml = fs.readFileSync(file, 'utf-8');
    const packageRegex = /<package\s+name="([^"]+)"[^>]*>([\s\S]*?)<\/package>/g;
    let match;
    while ((match = packageRegex.exec(xml)) !== null) {
      const pkgName = match[1];
      if (!packages.has(pkgName)) {
        packages.set(pkgName, match[0]);
      }
    }
  }

  const timestamp = new Date().toISOString();
  const packagesXml = Array.from(packages.values()).join('\n  ');

  return `<?xml version="1.0" ?>
<coverage version="6.0.0" timestamp="${timestamp}">
  <sources>
    <source>.</source>
  </sources>
  ${packagesXml}
</coverage>`;
}

function main() {
  const args = parseArgs(process.argv);
  const inputFiles = expandGlob(args.inputs);
  if (inputFiles.length === 0) {
    console.error(`错误: 未找到匹配文件: ${args.inputs}`);
    process.exit(1);
  }
  console.log(`合并 ${inputFiles.length} 个文件: ${inputFiles.join(', ')}`);
  const merged = mergeCobertura(inputFiles);
  fs.writeFileSync(args.output, merged, 'utf-8');
  console.log(`✅ 合并完成: ${args.output} (${merged.length} 字节)`);
}

main();