#!/usr/bin/env node
/**
 * 未接线 crate 清单门禁（2026-10-28 P1-8）
 *
 * 「未接线」= workspace 内没有任何其它成员以 normal/build 依赖引用它
 * （dev-dependencies 不计入：只在测试中使用不代表能力已交付）。
 *
 * 目的：防止新增 crate 后既不接线、也不登记，导致 README 宣称的能力无法兑现。
 *
 * 规则：
 * 1. 每个未接线成员必须出现在 scripts/audit/unwired-crates.json；
 * 2. 清单条目必须带 role + reason + review_by(YYYY-MM-DD)；
 * 3. review_by 过期 → 失败（强制重估：接线 / 删除 / 续期并说明）；
 * 4. 清单条目对应的 crate 已被接线或已不存在 → 失败（清单过期）；
 * 5. 入口 crate（应用/CLI/示例）在清单中以 role="entry" 登记。
 *
 * 退出码：0 = 合规；1 = 违规
 */
'use strict';

const { execFileSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const MANIFEST = path.join(__dirname, 'unwired-crates.json');
const VALID_ROLES = new Set(['entry', 'planned', 'feature-gated', 'internal']);

function fail(msg) {
  console.error(`FAIL: ${msg}`);
  process.exitCode = 1;
}

function loadMetadata() {
  // --no-deps：仅需各成员自己声明的依赖，不需要完整 resolve 图（后者在大型
  // workspace 上会触发全量解析，耗时数分钟且依赖网络/缓存锁）。
  const raw = execFileSync('cargo', ['metadata', '--no-deps', '--format-version', '1'], {
    encoding: 'utf8',
    maxBuffer: 256 * 1024 * 1024,
  });
  return JSON.parse(raw);
}

function main() {
  const meta = loadMetadata();
  const members = new Set(meta.workspace_members);
  const pkgs = meta.packages.filter((p) => members.has(p.id));
  const memberNames = new Set(pkgs.map((p) => p.name));

  // 引用计数（normal + build，忽略 dev）
  const refCount = new Map(pkgs.map((p) => [p.name, 0]));
  for (const pkg of pkgs) {
    for (const dep of pkg.dependencies || []) {
      const kind = dep.kind || 'normal';
      if (kind === 'dev') continue;
      const target = dep.rename ? dep.rename : dep.name;
      // 依赖名可能与成员 package 名不同（rename）；两种都尝试
      const targetName = memberNames.has(dep.name) ? dep.name : target;
      if (!memberNames.has(targetName) || targetName === pkg.name) continue;
      refCount.set(targetName, (refCount.get(targetName) || 0) + 1);
    }
  }

  const unwired = [...refCount.entries()].filter(([, n]) => n === 0).map(([name]) => name).sort();

  let manifest = { crates: [] };
  if (fs.existsSync(MANIFEST)) {
    manifest = JSON.parse(fs.readFileSync(MANIFEST, 'utf8'));
  } else {
    fail(`缺少清单 ${path.relative(process.cwd(), MANIFEST)}`);
  }

  const entries = new Map((manifest.crates || []).map((c) => [c.name, c]));
  const today = new Date().toISOString().slice(0, 10);

  for (const name of unwired) {
    const entry = entries.get(name);
    if (!entry) {
      fail(`未接线 crate 未登记: ${name}（接线或加入 unwired-crates.json）`);
      continue;
    }
    if (!VALID_ROLES.has(entry.role)) {
      fail(`${name}: role=${entry.role} 非法（允许: ${[...VALID_ROLES].join('/')}）`);
    }
    if (!entry.reason || entry.reason.trim().length < 8) {
      fail(`${name}: 缺少有意义的 reason`);
    }
    if (!/^\d{4}-\d{2}-\d{2}$/.test(entry.review_by || '')) {
      fail(`${name}: 缺少 review_by(YYYY-MM-DD)`);
    } else if (entry.review_by < today) {
      fail(`${name}: 复核期限已过（${entry.review_by} < ${today}）— 接线、删除或续期`);
    }
  }

  for (const [name, entry] of entries) {
    if (!refCount.has(name)) {
      fail(`清单条目 ${name} 不在 workspace members 中（删除该条目）`);
    } else if (!unwired.includes(name) && entry.role !== 'entry') {
      fail(`清单条目 ${name} 已被接线，属于过期登记（删除该条目）`);
    }
  }

  console.log(`workspace members: ${refCount.size}`);
  console.log(`unwired members: ${unwired.length}`);
  for (const name of unwired) {
    const e = entries.get(name) || {};
    console.log(`  - ${name} [${e.role || 'UNREGISTERED'}] review_by=${e.review_by || '-'}`);
  }
  if (process.exitCode) {
    console.error('未接线 crate 门禁未通过');
  } else {
    console.log('PASS: 未接线 crate 均已登记且未过期');
  }
}

main();
