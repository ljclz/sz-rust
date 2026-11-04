#!/usr/bin/env node
'use strict';

/**
 * 门禁 31：覆盖率矩阵完整性（SDET 评测 P1-1 防复发）
 *
 * 背景：coverage.yml 的 P0-P3 分片只覆盖 22/66 crate，约 44 个 crate 无任何
 * per-crate 覆盖率门禁。本门禁强制每个 workspace member 要么出现在
 * coverage.yml 的 `-p <crate>` 分片列表中，要么登记在
 * docs/audit/coverage-exemption.md 豁免清单（含到期日）。
 *
 * 规则：
 *   - ERROR：workspace member 未出现在 coverage.yml 任一 `-p/--package` 命令，
 *     且未登记豁免清单。
 *   - 豁免清单同时校验有效期：expiry_date 早于当天的视为过期 ERROR。
 *     （approval_date 格式 YYYY-MM-DD；expiry_date 格式 YYYY-MM-DD）
 *
 * 用法：node scripts/audit/coverage-matrix-completeness.js
 * 退出码：0 = 全覆盖；1 = 存在未纳入矩阵的 crate / 过期豁免
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const WORKSPACE_TOML = path.join(ROOT, 'Cargo.toml');
const COVERAGE_YML = path.join(ROOT, '.github', 'workflows', 'coverage.yml');
const EXEMPTION_FILE = path.join(ROOT, 'docs', 'audit', 'coverage-exemption.md');

// ---------------------------------------------------------------------------
// 1. 解析 workspace members
// ---------------------------------------------------------------------------
function parseWorkspaceMembers() {
  const content = fs.readFileSync(WORKSPACE_TOML, 'utf-8');
  const members = [];
  const re = /^\s*["'](packages\/[^"']+)["'],\s*$/gm;
  let m;
  while ((m = re.exec(content)) !== null) {
    const member = m[1].replace(/\\/g, '/');
    // 取 crate 名（member 最后一段）
    const crate = member.split('/').filter(Boolean).pop();
    if (crate) members.push(crate);
  }
  return members;
}

// ---------------------------------------------------------------------------
// 2. 从 coverage.yml 提取被 -p 引用的 crate
// ---------------------------------------------------------------------------
function collectCoveredCrates() {
  const covered = new Set();
  if (!fs.existsSync(COVERAGE_YML)) return covered;
  const content = fs.readFileSync(COVERAGE_YML, 'utf-8');
  const re = /(?:-p|--package)\s+([\w-]+)/g;
  let m;
  while ((m = re.exec(content)) !== null) covered.add(m[1]);
  return covered;
}

// ---------------------------------------------------------------------------
// 3. 解析豁免清单（crate + 到期日）
// ---------------------------------------------------------------------------
function parseExemptions() {
  const map = new Map(); // crate -> { expiryDate }
  if (!fs.existsSync(EXEMPTION_FILE)) return map;
  const lines = fs.readFileSync(EXEMPTION_FILE, 'utf-8').split('\n');
  for (const line of lines) {
    // | crate_name | reason | approver | approval_date | expiry_date |
    const parts = line.split('|').map((s) => s.trim());
    if (parts.length < 6) continue;
    const crate = parts[1];
    const expiry = parts[5];
    if (!/^[\w-]+$/.test(crate) || crate === 'crate_name') continue;
    if (!/^\d{4}-\d{2}-\d{2}$/.test(expiry)) continue;
    map.set(crate, { expiryDate: expiry });
  }
  return map;
}

// ---------------------------------------------------------------------------
// 4. 当天日期（UTC，与 CI 一致）
// ---------------------------------------------------------------------------
function todayUTC() {
  return new Date().toISOString().slice(0, 10);
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------
function main() {
  const members = parseWorkspaceMembers();
  const covered = collectCoveredCrates();
  const exempt = parseExemptions();
  const failures = [];
  const expired = [];

  console.log(`workspace members: ${members.length}`);
  console.log(`coverage.yml 覆盖: ${covered.size} 个 crate；豁免登记: ${exempt.size} 条`);

  for (const crate of members) {
    const inCoverage = covered.has(crate);
    const ex = exempt.get(crate);
    const inExempt = ex !== undefined;
    if (inCoverage || inExempt) {
      if (inExempt && ex.expiryDate < todayUTC()) {
        console.log(`  [EXPIRED] ${crate}（豁免 ${ex.expiryDate} 已到期）`);
        expired.push(crate);
      } else {
        console.log(`  [OK] ${crate}${inCoverage ? ' (coverage.yml)' : ' (exempt)'}`);
      }
    } else {
      console.log(`  [FAIL] ${crate}`);
      failures.push(crate);
    }
  }

  if (failures.length > 0 || expired.length > 0) {
    if (failures.length > 0) {
      console.error('\n❌ 门禁 31 失败：以下 crate 未进入覆盖率矩阵且未登记豁免：');
      for (const f of failures) console.error(`  - ${f}`);
      console.error('处置：a) 在 coverage.yml 添加 -p 分片；b) 或登记 docs/audit/coverage-exemption.md（含到期日）。');
    }
    if (expired.length > 0) {
      console.error('\n❌ 门禁 31 失败：以下 crate 豁免已到期，需补测或重新审批：');
      for (const e of expired) console.error(`  - ${e}`);
    }
    process.exit(1);
  }
  console.log('\n✅ 门禁 31 通过：全部 workspace member 已纳入覆盖率矩阵或有效豁免');
  process.exit(0);
}

main();