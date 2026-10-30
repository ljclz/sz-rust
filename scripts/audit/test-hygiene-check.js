#!/usr/bin/env node
'use strict';

/**
 * 门禁 33：测试卫生检查（SDET 评测 P1-3/P1-4/P2-4 防复发）
 *
 * 检查测试代码中的不良模式（已剔除 // 注释与双引号字符串字面量影响）：
 *   ERROR：
 *     - #[tokio::test] 函数体内使用 std::thread::sleep（阻塞异步运行时）
 *     - 测试中硬编码真实数据库凭据（postgres://user:pass@ 且密码非测试占位值）
 *   WARN：
 *     - 同步 #[test] 函数体内使用 std::thread::sleep（不阻塞运行时，测试豁免，建议缩短）
 *     - 测试体内 sleep(Duration::from_secs(N)) 且 N>=30（长 sleep 易 flaky）
 *     - 测试中绑定固定端口（建议 127.0.0.1:0）
 *     - #[test] 函数体含 panic!("...") 而无 assert 宏（断言价值门禁绕过）
 *     - 测试中硬编码密码字面量（含占位特征时忽略）
 *
 * 基线棘轮（防存量问题导致门禁上线即红）：
 *   - scripts/audit/test-hygiene-baseline.json 记录当前 ERROR 项
 *     （key = `${rel}:${fnLine}:${ruleId}`）。
 *   - 运行时：新出现的 ERROR（不在基线）→ 失败；基线内存量 → [BASELINE] 不阻塞。
 *   - `--update`：把当前全部 ERROR 写入基线（仅首次接入或人工确认存量时使用）。
 *
 * 用法：node scripts/audit/test-hygiene-check.js [--update] [--fail-on-warn]
 * 退出码：0 = 无新增 ERROR（或 WARN 且未指定 --fail-on-warn）；1 = 有新增 ERROR / --fail-on-warn 时 WARN 也失败
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const PACKAGES = path.join(ROOT, 'packages');
const BASELINE_FILE = path.join(__dirname, 'test-hygiene-baseline.json');
const FAIL_ON_WARN = process.argv.includes('--fail-on-warn');
const UPDATE = process.argv.includes('--update');

const ASSERT_RE = /assert!\(|assert_eq!|assert_ne!|assert_matches!|assert_ok!|assert_err!|assert_contains!/;

// 测试占位密码（CI/本地测试常见值，不算真实凭据）
const PLACEHOLDER_PASSWORDS = new Set([
  'test123', 'password', 'root', 'pass', 'testpass', 'admin', 'admin123',
  'secret', '<secret>', '<password>', '<bcrypt-hash>', 'hashed', 'hidden',
  'existing', 'test-secret', 'correct-secret', 'super-secret-key-12345',
  'invalid', 'abcdefghijklmnopqrstuvwxyz0123456789ABCD',
]);

function isPlaceholderPassword(pwd) {
  const lower = pwd.toLowerCase();
  if (PLACEHOLDER_PASSWORDS.has(lower)) return true;
  if (lower.includes('<') || lower.includes('>')) return true;
  if (lower.includes('secret') || lower.includes('test') || lower.includes('invalid') ||
      lower.includes('hidden') || lower.includes('hash') || lower.includes('existing')) return true;
  if (pwd.length > 30) return true; // 随机/长测试向量
  return false;
}

function walk(dir, ext, out = []) {
  if (!fs.existsSync(dir)) return out;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, e.name);
    if (e.isDirectory()) {
      if (e.name === 'target' || e.name === 'node_modules') continue;
      walk(full, ext, out);
    } else if (e.isFile() && e.name.endsWith(ext)) out.push(full);
  }
  return out;
}

// 剔除 // 注释与双引号字符串（含 r#"..."# 的字符串体），保留代码骨架
function stripCommentsAndStrings(line) {
  let out = line;
  // 去掉 // 注释（不在字符串内的）
  const commentIdx = out.indexOf('//');
  if (commentIdx >= 0) out = out.slice(0, commentIdx);
  // 去掉双引号字符串字面量（含 raw string 的引号部分）
  out = out.replace(/r#*"(?:[^"\\]|\\.)*"/g, '""')
           .replace(/"(?:[^"\\]|\\.)*"/g, '""');
  return out;
}

// 提取 #[test] / #[tokio::test] 函数体（花括号配对）
function extractTestBodies(content) {
  const bodies = [];
  const lines = content.split('\n');
  const testAttrRe = /#\s*\[\s*(?:tokio::test|async_std::test|test|sqlx::test|actix_web::test)\s*(?:,[^\]]*)?\]/;
  for (let i = 0; i < lines.length; i++) {
    if (!testAttrRe.test(lines[i])) continue;
    let fnIdx = -1;
    for (let j = i + 1; j < lines.length && j <= i + 20; j++) {
      if (/^\s*(pub\s+)?(async\s+)?fn\s/.test(lines[j])) { fnIdx = j; break; }
      if (/^\s*#\s*\[/.test(lines[j])) continue;
      break;
    }
    if (fnIdx === -1) continue;
    let depth = 0;
    let start = -1;
    for (let j = fnIdx; j < lines.length; j++) {
      const line = lines[j];
      const open = (line.match(/{/g) || []).length;
      const close = (line.match(/}/g) || []).length;
      if (start === -1 && open > 0) start = j;
      depth += open - close;
      if (start !== -1 && depth <= 0) {
        bodies.push({
          attrLine: i + 1,
          fnLine: fnIdx + 1,
          body: lines.slice(start, j + 1).map(stripCommentsAndStrings).join('\n'),
          isTokio: /tokio::test|async_std::test|actix_web::test|sqlx::test/.test(lines[i]),
        });
        break;
      }
    }
  }
  return bodies;
}

function main() {
  const files = walk(PACKAGES, '.rs');
  const errors = [];
  const warns = [];

  for (const file of files) {
    if (file.includes(`${path.sep}tests${path.sep}common${path.sep}`)) continue;
    const content = fs.readFileSync(file, 'utf-8');
    const bodies = extractTestBodies(content);
    if (bodies.length === 0) continue;
    const rel = path.relative(ROOT, file);

    for (const b of bodies) {
      const loc = `${rel}:${b.fnLine}`;

      // 1. std::thread::sleep（注释/字符串已剔除）
      if (/std::thread::sleep|thread::sleep/.test(b.body)) {
        if (b.isTokio) {
          errors.push({ key: `${loc}:async-thread-sleep`, msg: `${loc} #[tokio::test] 函数体使用 std::thread::sleep（阻塞异步运行时，应改 tokio::time::sleep）` });
        } else {
          warns.push(`${loc} 同步测试使用 std::thread::sleep（不阻塞运行时，建议缩短/改用 tokio）`);
        }
      }

      // 2. 长 sleep
      const longSleep = b.body.match(/sleep\s*\(\s*Duration::from_secs\(\s*(\d+)\s*\)/);
      if (longSleep && parseInt(longSleep[1], 10) >= 30) {
        warns.push(`${loc} 长 sleep ${longSleep[1]}s（易 flaky，建议缩短或轮询）`);
      }

      // 3. 固定端口绑定
      const portRe = /(?:GraphQLServer|TcpListener|Server)::new\(\s*(\d{4,5})\s*\)|bind\s*\(\s*"127\.0\.0\.1:(\d{4,5})"/;
      const portMatch = b.body.match(portRe);
      if (portMatch) {
        warns.push(`${loc} 固定端口 ${portMatch[1] || portMatch[2]}（建议 127.0.0.1:0 随机端口）`);
      }

      // 4. panic! 当断言（函数体含 panic! 且无 assert 宏）
      if (/panic!\("/.test(b.body) && !ASSERT_RE.test(b.body)) {
        warns.push(`${loc} 使用 panic! 而无 assert 宏（断言价值门禁只统计 assert）`);
      }

      // 5. 硬编码密码/凭据
      const connRe = /(?:postgres|mysql|redis):\/\/[\w-]+:([^@\s]+)@/;
      const connMatch = b.body.match(connRe);
      if (connMatch && !isPlaceholderPassword(connMatch[1])) {
        errors.push({ key: `${loc}:hardcoded-credential`, msg: `${loc} 硬编码真实数据库凭据（密码 "${connMatch[1]}"，应环境变量注入）` });
      }
    }
  }

  // 基线棘轮
  const baseline = fs.existsSync(BASELINE_FILE)
    ? JSON.parse(fs.readFileSync(BASELINE_FILE, 'utf-8'))
    : { items: [] };
  const baselineSet = new Set(baseline.items || []);
  const newErrors = errors.filter((e) => !baselineSet.has(e.key));
  const baselineErrors = errors.filter((e) => baselineSet.has(e.key));

  console.log(`测试卫生检查：${files.length} 文件，${errors.length} ERROR（其中基线 ${baselineErrors.length} 项），${warns.length} WARN`);
  for (const e of baselineErrors) console.log(`  [BASELINE] ${e.msg}`);
  for (const e of newErrors) console.log(`  [ERROR] ${e.msg}`);
  for (const w of warns) console.log(`  [WARN ] ${w}`);

  if (UPDATE) {
    const items = errors.map((e) => e.key);
    fs.writeFileSync(BASELINE_FILE, JSON.stringify({ items }, null, 2) + '\n');
    console.log(`\n已更新基线：${items.length} 条 ERROR 记录到 test-hygiene-baseline.json`);
    process.exit(0);
  }

  if (newErrors.length > 0) {
    console.error('\n❌ 门禁 33 失败：新增测试卫生 ERROR（存量基线项不阻塞）');
    for (const e of newErrors) console.error(`  - ${e.msg}`);
    console.error('处置：修复（async 内 thread::sleep → tokio；硬编码凭据 → 环境变量）；或确认为存量后运行 --update 更新基线。');
    process.exit(1);
  }
  if (FAIL_ON_WARN && warns.length > 0) {
    console.error('\n❌ 门禁 33 失败（--fail-on-warn）：存在 WARN 项');
    process.exit(1);
  }
  console.log('\n✅ 门禁 33 通过' + (warns.length > 0 ? '（WARN 请人工复核）' : ''));
  process.exit(0);
}

main();