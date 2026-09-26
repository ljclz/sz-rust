#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
// 静态缺陷模式扫描门禁（v1.4 深度检测沉淀）
//
// 扫描 packages/*/src/**/*.rs（自动跳过 #[cfg(test)] 模块/函数），
// 按规则统计命中并与基线（scripts/audit/bug-pattern-baseline.json）比较：
//   - 任一「文件 × 规则」命中数 > 基线 → 退出码 1（CI 门禁）
//   - 命中数下降或清零是好事（棘轮只紧不松）
//
// 用法：
//   node scripts/audit/bug-pattern-scan.js            # 门禁模式（对比基线）
//   node scripts/audit/bug-pattern-scan.js --update   # 重算并写入新基线
//   node scripts/audit/bug-pattern-scan.js --report   # 只打印明细不判定
//
// 规则来源：2026-09-26 深度检测确认的 7 类缺陷模式，详见
// docs/review-suite/缺陷检测审查套件.md 第二节。

'use strict';

const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..', '..');
const BASELINE_PATH = path.join(__dirname, 'bug-pattern-baseline.json');

// ---------------------------------------------------------------------------
// 规则表：id / 说明 / 行级正则（对剔除测试代码后的源码逐行匹配）
// ---------------------------------------------------------------------------
const RULES = [
  {
    id: 'std-fs',
    severity: 'error',
    desc: '铁律违规：使用 std::fs（必须 tokio::fs）',
    re: /\bstd::fs::/,
  },
  {
    id: 'poison-expect',
    severity: 'error',
    desc: '锁中毒 panic：read()/write()/lock() 结果用 expect("锁被毒化")，应改 unwrap_or_else(PoisonError::into_inner)',
    re: /\.expect\(\s*"锁被毒化"\s*\)/,
  },
  {
    id: 'narrow-cast',
    severity: 'error',
    desc: '窄化截断：as_i64()/as_u64() 结果直接 as i8/i16/i32/u16/u32（应用 clamp/min 饱和）；行内已含 clamp/min/max 处理的不计',
    re: /^(?!.*\.(?:clamp|min|max)\()[^\n]*\.(?:as_i64|as_u64)\(\)[^\n;]*\bas\s(?:i8|i16|i32|u16|u32)\b/,
  },
  {
    id: 'select-star',
    severity: 'error',
    desc: 'SELECT * 违规（必须显式列投影，防 schema 漂移）',
    re: /SELECT\s+\*/i,
  },
  {
    id: 'unbounded-channel',
    severity: 'warn',
    desc: '无界 channel（确认消费速度上界，否则改 bounded + 背压）',
    re: /unbounded_channel\(\)/,
  },
  {
    id: 'gen-range-var',
    severity: 'warn',
    desc: 'gen_range 变量区间：区间端点为标识符时必须先保证非空/非零（gen_range(0..0) panic）',
    re: /gen_range\(\s*0\s*\.\.\s*[a-zA-Z_][a-zA-Z0-9_.]*\s*\)/,
  },
  {
    id: 'dyn-join-key',
    severity: 'warn',
    desc: '动态路径拼接：root.join(<变量>) 需确认 key 已过词法/前缀校验（路径穿越）',
    re: /\.join\(\s*(?:key|archive_key|filename|filepath|file_name|name)\s*\)/,
  },
];

// ---------------------------------------------------------------------------
// 测试代码剔除：#[cfg(test)] 之后的 mod/fn 块按花括号深度跳过
// ---------------------------------------------------------------------------
function stripTestCode(source) {
  const lines = source.split(/\r?\n/);
  const out = [];
  let i = 0;
  const isAttr = (l) => /^\s*#\[\s*cfg\s*\(\s*test\s*\)\s*\]/.test(l);
  const nextCodeLine = (idx) => {
    for (let j = idx + 1; j < lines.length; j++) {
      const t = lines[j].trim();
      if (t === '' || t.startsWith('//')) continue;
      return { line: lines[j], trimmed: t, index: j };
    }
    return null;
  };
  // 从含 '{' 的行开始按深度跳过整个块，返回恢复索引
  const skipBracedBlock = (startIdx) => {
    let depth = 0;
    let opened = false;
    for (let j = startIdx; j < lines.length; j++) {
      for (const ch of lines[j]) {
        if (ch === '{') { depth++; opened = true; }
        else if (ch === '}') { depth--; }
      }
      if (opened && depth <= 0) return j + 1;
    }
    return lines.length;
  };

  while (i < lines.length) {
    if (isAttr(lines[i])) {
      const nxt = nextCodeLine(i);
      if (!nxt) { i++; continue; }
      if (/^(?:pub\s+)?mod\s+\w+/.test(nxt.trimmed) && nxt.trimmed.includes('{')) {
        // 跳过属性行到 mod 块结束
        i = skipBracedBlock(nxt.index);
        continue;
      }
      if (/^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+\w+/.test(nxt.trimmed)) {
        // 单个测试函数：跳过其函数体
        i = skipBracedBlock(nxt.index);
        continue;
      }
      // 属性行后不是测试模块/函数（罕见）：只跳过属性行
      i++;
      continue;
    }
    out.push(lines[i]);
    i++;
  }
  return out;
}

// ---------------------------------------------------------------------------
// 扫描
// ---------------------------------------------------------------------------
function listRustSources() {
  const pkgDir = path.join(ROOT, 'packages');
  const files = [];
  const walk = (dir) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name === 'target' || entry.name === 'fuzz' || entry.name === 'benches' || entry.name === 'tests') continue;
        walk(full);
      } else if (entry.name.endsWith('.rs')) {
        files.push(full);
      }
    }
  };
  walk(pkgDir);
  // 根 src/
  const rootSrc = path.join(ROOT, 'src');
  if (fs.existsSync(rootSrc)) walk(rootSrc);
  return files;
}

function scan() {
  const hits = {}; // { ruleId: { relFile: count } }
  for (const rule of RULES) hits[rule.id] = {};

  const files = listRustSources();
  for (const file of files) {
    const rel = path.relative(ROOT, file).replace(/\\/g, '/');
    const src = fs.readFileSync(file, 'utf8');
    const codeLines = stripTestCode(src);
    for (const line of codeLines) {
      for (const rule of RULES) {
        if (rule.re.test(line)) {
          hits[rule.id][rel] = (hits[rule.id][rel] || 0) + 1;
        }
      }
    }
  }
  return hits;
}

function loadBaseline() {
  if (!fs.existsSync(BASELINE_PATH)) return null;
  return JSON.parse(fs.readFileSync(BASELINE_PATH, 'utf8'));
}

function main() {
  const mode = process.argv[2] || '';
  const hits = scan();
  const totals = {};
  for (const rule of RULES) {
    totals[rule.id] = Object.values(hits[rule.id]).reduce((a, b) => a + b, 0);
  }

  console.log('=== 静态缺陷模式扫描（测试代码已剔除）===');
  for (const rule of RULES) {
    console.log(`[${rule.severity}] ${rule.id}: ${totals[rule.id]}`);
  }

  if (mode === '--update') {
    fs.writeFileSync(BASELINE_PATH, JSON.stringify({ updated: new Date().toISOString().slice(0, 10), totals, files: hits }, null, 2) + '\n');
    console.log(`\n基线已更新: ${path.relative(ROOT, BASELINE_PATH)}`);
    return;
  }

  const baseline = loadBaseline();
  if (!baseline) {
    console.error('\n基线不存在，请先运行: node scripts/audit/bug-pattern-scan.js --update');
    process.exit(2);
  }

  let violations = 0;
  console.log('\n=== 与基线比较（只紧不松棘轮）===');
  for (const rule of RULES) {
    const base = (baseline.totals && baseline.totals[rule.id]) || 0;
    const now = totals[rule.id];
    const mark = now > base ? 'REGRESSION' : now < base ? 'improved' : 'ok';
    console.log(`${rule.id}: baseline=${base} now=${now} [${mark}]`);
    if (now > base) {
      violations++;
      const baseFiles = (baseline.files && baseline.files[rule.id]) || {};
      for (const [file, count] of Object.entries(hits[rule.id])) {
        if ((baseFiles[file] || 0) < count) {
          console.log(`  新增/增多: ${file} (${baseFiles[file] || 0} -> ${count})`);
        }
      }
    }
  }

  if (mode === '--report') {
    console.log('\n（--report 模式不判定退出码）');
    return;
  }

  if (violations > 0) {
    console.error(`\nFAIL: ${violations} 条规则出现新增违规。修复或经评审后运行 --update 重设基线。`);
    process.exit(1);
  }
  console.log('\nPASS: 无新增缺陷模式违规。');
}

main();
