#!/usr/bin/env node
'use strict';

/**
 * 门禁 30：ignored 测试路由检查（SDET 评测 P0-1 防复发）
 *
 * 背景：SDET 评测发现 189 个 `#[ignore]` 测试（含全部 DB/Redis/SSO 集成测试）
 * 在 CI 中从不执行——没有任何 workflow 传 `--ignored`。本门禁强制每个
 * `#[ignore]` 测试要么被某 workflow 的 `--ignored` 命令显式覆盖，要么登记在
 * docs/audit/ignored-test-exemption.md 豁免清单（仅限外部凭据/收费服务）。
 *
 * 规则：
 *   - ERROR：`#[ignore]` 测试文件未出现在任何 workflow 的 `--ignored` 命令中，
 *     且未登记豁免清单。
 *   - 判定"被覆盖"：workflow 中 `cargo test` 命令文本（含 `-p` 包名 /
 *     `--test 文件名` / `--test-threads` 等）与测试文件路径存在可匹配关系：
 *     a) 命令含 `--test <文件名>` 且 `-p <包名>` 与文件所在 crate 一致；或
 *     b) 命令对某包运行 `--lib -- --ignored`（覆盖该包 src 内 ignored 测试）；
 *     c) 命令对某包 `-- --ignored` 且带测试名过滤器（如 redis_gateway）——
 *        覆盖 src 内名称匹配的 ignored 测试（近似匹配，需人工兜底）。
 *   - 不把 `--workspace -- --ignored` 视作覆盖（避免"看似跑全量实则必然失败"）。
 *
 * 用法：node scripts/audit/ignored-test-routing.js
 * 退出码：0 = 全覆盖；1 = 存在未被路由的 ignored 测试
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const WORKFLOWS_DIR = path.join(ROOT, '.github', 'workflows');
const EXEMPTION_FILE = path.join(ROOT, 'docs', 'audit', 'ignored-test-exemption.md');
const PACKAGES_DIR = path.join(ROOT, 'packages');

// ---------------------------------------------------------------------------
// 1. 收集所有 #[ignore] 测试文件（含 src 内嵌测试）
// ---------------------------------------------------------------------------
function walkDir(dir, ext, out = []) {
  if (!fs.existsSync(dir)) return out;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === 'target' || entry.name === 'node_modules') continue;
      walkDir(full, ext, out);
    } else if (entry.isFile() && entry.name.endsWith(ext)) {
      out.push(full);
    }
  }
  return out;
}

function hasIgnoredAttr(content) {
  // 忽略字符串字面量/注释中的 "#[ignore"，只匹配真实属性行
  return /^\s*#\s*\[(?:[^\]]*,\s*)?ignore\s*[=\]]/m.test(content);
}

function collectIgnoredTests() {
  const tests = [];
  const crateDirs = fs.existsSync(PACKAGES_DIR)
    ? fs.readdirSync(PACKAGES_DIR).filter((d) => fs.statSync(path.join(PACKAGES_DIR, d)).isDirectory())
    : [];
  for (const crate of crateDirs) {
    for (const sub of ['tests', 'src']) {
      const dirPath = path.join(PACKAGES_DIR, crate, sub);
      if (!fs.existsSync(dirPath)) continue;
      for (const file of walkDir(dirPath, '.rs')) {
        const content = fs.readFileSync(file, 'utf-8');
        if (!hasIgnoredAttr(content)) continue;
        const rel = path.relative(PACKAGES_DIR, file).split(path.sep).join('/');
        const isSrc = rel.includes('/src/');
        tests.push({ crate, file, rel, isSrc, content });
      }
    }
  }
  return tests;
}

// ---------------------------------------------------------------------------
// 2. 从 workflow 提取 --ignored 命令
// ---------------------------------------------------------------------------
function collectWorkflowCommands() {
  const cmds = [];
  if (!fs.existsSync(WORKFLOWS_DIR)) return cmds;
  for (const wf of fs.readdirSync(WORKFLOWS_DIR)) {
    if (!wf.endsWith('.yml') && !wf.endsWith('.yaml')) continue;
    const lines = fs.readFileSync(path.join(WORKFLOWS_DIR, wf), 'utf-8').split('\n');
    for (let i = 0; i < lines.length; i++) {
      const trimmed = lines[i].trim();
      if (!trimmed.startsWith('run:')) continue;
      const rest = trimmed.slice(4).trim();
      let block = '';
      if (rest === '|' || rest === '>' || rest === '|-' || rest === '>-' ||
          rest === '|+' || rest === '>+') {
        const indent = (lines[i].match(/^(\s*)/) || ['', ''])[1].length;
        for (let j = i + 1; j < lines.length; j++) {
          const line = lines[j];
          if (!line.trim()) continue;
          const curIndent = (line.match(/^(\s*)/) || ['', ''])[1].length;
          if (curIndent <= indent) break;
          block += line + '\n';
        }
      } else {
        block = rest;
      }
      if (block.includes('--ignored') && block.includes('cargo test')) {
        cmds.push({ workflow: wf, text: block });
      }
    }
  }
  return cmds;
}

// ---------------------------------------------------------------------------
// 3. 解析豁免清单
// ---------------------------------------------------------------------------
function parseExemptions() {
  const set = new Set();
  if (!fs.existsSync(EXEMPTION_FILE)) return set;
  const lines = fs.readFileSync(EXEMPTION_FILE, 'utf-8').split('\n');
  for (const line of lines) {
    // | crate | tests/xxx.rs | reason | ... |
    const m = line.match(/^\|\s*([\w-]+)\s*\|\s*(tests\/[^\s|]+|src\/[^\s|]+)\s*\|/);
    if (m && m[1] !== 'crate') set.add(`${m[1]}/${m[2]}`);
  }
  return set;
}

// ---------------------------------------------------------------------------
// 4. 判定覆盖
// ---------------------------------------------------------------------------
function isCovered(test, cmds) {
  const { crate, rel, isSrc } = test;
  for (const cmd of cmds) {
    // 该命令是否针对此 crate？（-p 或 --package）
    const crateRe = new RegExp(`(?:-p|--package)\\s+${escapeRegex(crate)}\\b`);
    if (!crateRe.test(cmd.text)) continue;
    if (isSrc) {
      // src 内 ignored 测试：--lib -- --ignored 或测试名过滤器
      if (/--lib\b/.test(cmd.text) && /--\s*--ignored/.test(cmd.text)) return true;
      // 近似：命令带名称过滤器（如 redis_gateway）且文件路径含该词
      const filterMatch = cmd.text.match(/--\s*--ignored[^#\n]*\s([\w_:]+)/);
      if (filterMatch && rel.toLowerCase().includes(filterMatch[1].toLowerCase())) {
        return true;
      }
      continue;
    }
    // tests/ 目录：--test <basename> 匹配
    const base = path.basename(test.file, '.rs');
    const testFlagRe = new RegExp(`--test\\s+${escapeRegex(base)}(?![\\w-])`);
    if (testFlagRe.test(cmd.text)) return true;
    // 或 --test 名带子串（e.g. windows_memory_baseline 与 --test 精确匹配已覆盖）
  }
  return false;
}

function escapeRegex(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------
function main() {
  const tests = collectIgnoredTests();
  const cmds = collectWorkflowCommands();
  const exempt = parseExemptions();
  const failures = [];

  console.log(`扫描到 ${tests.length} 个含 #[ignore] 的测试文件`);
  console.log(`workflow --ignored 命令 ${cmds.length} 条；豁免登记 ${exempt.size} 条`);

  for (const t of tests) {
    const exemptKey = t.rel; // rel 已含 crate 前缀（如 sz-rust-ai-facade/tests/real_api_test.rs）
    const isExempt = exempt.has(exemptKey);
    const covered = isCovered(t, cmds);
    const status = isExempt ? 'EXEMPT' : covered ? 'OK' : 'FAIL';
    console.log(`  [${status}] ${t.rel}`);
    if (!isExempt && !covered) {
      failures.push(t.rel);
    }
  }

  if (failures.length > 0) {
    console.error('\n❌ 门禁 30 失败：以下 ignored 测试未接入任何 workflow 且未登记豁免：');
    for (const f of failures) console.error(`  - ${f}`);
    console.error('处置：a) 在对应 workflow 添加 --test/--lib 显式执行；b) 或登记 docs/audit/ignored-test-exemption.md（仅限外部凭据）。');
    process.exit(1);
  }
  console.log('\n✅ 门禁 30 通过：全部 ignored 测试已被 workflow 路由或登记豁免');
  process.exit(0);
}

main();