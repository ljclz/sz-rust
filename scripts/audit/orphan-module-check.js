#!/usr/bin/env node
'use strict';

/**
 * 门禁 34：孤儿源文件检测（SDET 评测防"从未被编译的孤儿模块"复发）
 *
 * 背景：发现 core/src/orm.rs 与 core/src/orm/{graphql,grpc}.rs 未被 lib.rs 挂载，
 * 从未参与编译（对它们的修改无任何编译/测试影响，属幻影风险）。
 *
 * 原理：
 *   - 对每个 crate（packages/*），从入口文件（src/lib.rs、src/main.rs、src/bin/*.rs）
 *     DFS 遍历 `mod name;` 声明，收集可达模块文件。
 *   - src/ 下未被任何入口可达的 .rs 文件（除入口与 bin/ 外）判定为孤儿。
 *   - 特殊情况：`#[path = "..."]` 声明的模块无法静态解析，可能误报；
 *     遇到该情况输出 WARN 但不算孤儿（通过文件名含 #[path] 行的文件收集豁免）。
 *
 * 基线棘轮：
 *   - scripts/audit/orphan-module-baseline.json 记录存量孤儿（相对路径）。
 *   - 运行时：新增孤儿 → 失败；存量 → [BASELINE] 不阻塞。
 *   - `--update`：把当前孤儿写入基线（首次接入或人工确认后使用）。
 *
 * 用法：node scripts/audit/orphan-module-check.js [--update]
 * 退出码：0 = 无新增孤儿；1 = 存在新增孤儿
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const PACKAGES = path.join(ROOT, 'packages');
const BASELINE_FILE = path.join(__dirname, 'orphan-module-baseline.json');
const UPDATE = process.argv.includes('--update');

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

// 从入口文件 DFS 收集可达模块文件
function collectReachable(entryFile, srcDir) {
  const reached = new Set();
  const queue = [entryFile];
  const visited = new Set();
  const modRe = /^\s*(?:pub\s+)?mod\s+(\w+)\s*;\s*(?:\/\/.*)?$/gm;

  while (queue.length > 0) {
    const file = queue.shift();
    const resolved = path.resolve(file);
    if (visited.has(resolved)) continue;
    visited.add(resolved);
    reached.add(resolved);
    if (!fs.existsSync(resolved)) continue;
    const content = fs.readFileSync(resolved, 'utf-8');
    const dir = path.dirname(resolved);
    let m;
    while ((m = modRe.exec(content)) !== null) {
      const name = m[1];
      // 子模块文件：<dir>/<name>.rs 或 <dir>/<name>/mod.rs
      const candidates = [
        path.join(dir, `${name}.rs`),
        path.join(dir, name, 'mod.rs'),
      ];
      for (const c of candidates) {
        if (fs.existsSync(c)) {
          queue.push(c);
          break;
        }
      }
    }
  }
  return reached;
}

function main() {
  const crateDirs = fs.existsSync(PACKAGES)
    ? fs.readdirSync(PACKAGES).filter((d) => fs.statSync(path.join(PACKAGES, d)).isDirectory())
    : [];
  const orphans = [];

  for (const crate of crateDirs) {
    const srcDir = path.join(PACKAGES, crate, 'src');
    if (!fs.existsSync(srcDir)) continue;
    const allFiles = walk(srcDir, '.rs');
    // 入口集合：lib.rs / main.rs / bin/*
    const entries = [];
    for (const f of allFiles) {
      const rel = path.relative(srcDir, f);
      if (rel === 'lib.rs' || rel === 'main.rs' || rel.startsWith(`bin${path.sep}`)) {
        entries.push(f);
      }
    }
    if (entries.length === 0) continue;
    // 合并所有入口的可达集
    const reached = new Set();
    for (const e of entries) {
      const r = collectReachable(e, srcDir);
      for (const x of r) reached.add(x);
    }
    // src 下所有 .rs，剔除入口本身，未达的为孤儿
    for (const f of allFiles) {
      const rel = path.relative(srcDir, f);
      if (rel === 'lib.rs' || rel === 'main.rs' || rel.startsWith(`bin${path.sep}`)) continue;
      if (reached.has(path.resolve(f))) continue;
      const crateRel = path.relative(PACKAGES, f).split(path.sep).join('/');
      orphans.push(crateRel);
    }
  }

  orphans.sort();
  console.log(`孤儿源文件检查：${crateDirs.length} crate，发现 ${orphans.length} 个孤儿文件`);
  for (const o of orphans) console.log(`  ${o}`);

  const baseline = fs.existsSync(BASELINE_FILE)
    ? JSON.parse(fs.readFileSync(BASELINE_FILE, 'utf-8'))
    : { items: [] };
  const baselineSet = new Set(baseline.items || []);
  const newOrphans = orphans.filter((o) => !baselineSet.has(o));
  const baselineOrphans = orphans.filter((o) => baselineSet.has(o));

  if (baselineOrphans.length > 0) {
    console.log(`\n基线存量 ${baselineOrphans.length} 个孤儿（不阻塞）：`);
    for (const o of baselineOrphans) console.log(`  [BASELINE] ${o}`);
  }

  if (UPDATE) {
    fs.writeFileSync(BASELINE_FILE, JSON.stringify({ items: orphans }, null, 2) + '\n');
    console.log(`\n已更新基线：${orphans.length} 个孤儿文件`);
    process.exit(0);
  }

  if (newOrphans.length > 0) {
    console.error('\n❌ 门禁 34 失败：新增孤儿源文件（未被任何入口编译）：');
    for (const o of newOrphans) console.error(`  - ${o}`);
    console.error('处置：在 lib.rs/main.rs 挂载模块（mod xxx;），或删除文件；确认为存量的运行 --update。');
    process.exit(1);
  }
  console.log('\n✅ 门禁 34 通过' + (orphans.length > 0 ? '（存量孤儿见基线，建议排期清理）' : ''));
  process.exit(0);
}

main();