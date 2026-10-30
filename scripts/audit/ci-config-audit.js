#!/usr/bin/env node
'use strict';

/**
 * 门禁 32：CI 配置审计（SDET 评测 P1-2/P1-5/P1-6/P1-7 防复发）
 *
 * 检查项：
 *   1. 覆盖率阈值一致性：ci.yml / coverage.yml 的 COVERAGE_THRESHOLD、
 *      project_rules 铁律 10、test-coverage SKILL 必须同为 85（行）。
 *   2. 深度门禁 fail-closed：ci.yml coverage / miri job、security.yml geiger
 *      step、mcdc.yml 不得出现 continue-on-error: true。
 *   3. 变异门禁 kill-rate 必须 fail-closed：mutants.yml 不得仅 ::warning::
 *      （必须 exit 1）。
 *   4. 性能回归门禁接线：benchmark.yml 与 ai-facade-perf-gate.yml 必须调用
 *      scripts/perf/regression_gate.sh；perf-gate.yml 必须调用
 *      scripts/perf-regression.js（2026-10-29 补：该脚本此前零 workflow 引用，
 *      「CI 性能回归门禁」只存在于文档中，本条防止再次脱钩）。
 *   5. 禁用的 workflow：存在 *.disabled 文件视为 ERROR（应重新启用或删除）。
 *
 * 用法：node scripts/audit/ci-config-audit.js
 * 退出码：0 = 通过；1 = 存在违规
 */

const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const WF_DIR = path.join(ROOT, '.github', 'workflows');
const CI_YML = path.join(WF_DIR, 'ci.yml');
const COVERAGE_YML = path.join(WF_DIR, 'coverage.yml');
const MUTANTS_YML = path.join(WF_DIR, 'mutants.yml');
const SECURITY_YML = path.join(WF_DIR, 'security.yml');
const MCDC_YML = path.join(WF_DIR, 'mcdc.yml');
const BENCHMARK_YML = path.join(WF_DIR, 'benchmark.yml');
const AI_PERF_YML = path.join(WF_DIR, 'ai-facade-perf-gate.yml');
const E2E_PERF_YML = path.join(WF_DIR, 'perf-gate.yml');
const PROJECT_RULES = path.join(ROOT, '.trae', 'rules', 'project_rules.md');
const SKILL = path.join(ROOT, '.trae', 'skills', 'sz-rust-test-coverage', 'SKILL.md');

const UNIFIED_THRESHOLD = 85;

function read(p) {
  return fs.existsSync(p) ? fs.readFileSync(p, 'utf-8') : '';
}

function extractThreshold(text) {
  const m = text.match(/COVERAGE_THRESHOLD:\s*(\d+)/);
  return m ? parseInt(m[1], 10) : null;
}

function hasContinueOnError(text) {
  return /continue-on-error\s*:\s*true/.test(text);
}

function main() {
  const failures = [];
  const warns = [];

  // 1. 阈值一致性
  const ciThreshold = extractThreshold(read(CI_YML));
  const covThreshold = extractThreshold(read(COVERAGE_YML));
  const rule10 = /测试覆盖率\s*≥\s*(\d+)/.test(read(PROJECT_RULES))
    ? /测试覆盖率\s*≥\s*(\d+)/.exec(read(PROJECT_RULES))[1]
    : null;
  const skill = />=?\s*(\d+)%/.test(read(SKILL))
    ? />=?\s*(\d+)%/.exec(read(SKILL))[1]
    : null;

  const thresholds = [
    ['ci.yml', ciThreshold],
    ['coverage.yml', covThreshold],
    ['project_rules 铁律 10', rule10 ? parseInt(rule10, 10) : null],
    ['test-coverage SKILL', skill ? parseInt(skill, 10) : null],
  ];
  console.log('阈值检查（统一 ' + UNIFIED_THRESHOLD + '）：');
  for (const [name, val] of thresholds) {
    console.log(`  ${name}: ${val}`);
    if (val === null) {
      warns.push(`${name} 未提取到阈值`);
    } else if (val !== UNIFIED_THRESHOLD) {
      failures.push(`${name} 阈值 ${val} != ${UNIFIED_THRESHOLD}`);
    }
  }

  // 2. 深度门禁 continue-on-error
  const cioeChecks = [
    ['ci.yml coverage job', CI_YML, /^  coverage:/m],
    ['ci.yml miri job', CI_YML, /^  miri:/m],
    ['security.yml geiger', SECURITY_YML, /geiger:/],
    ['mcdc.yml', MCDC_YML, /mcdc:/],
  ];
  console.log('\ncontinue-on-error 检查：');
  for (const [name, file, anchor] of cioeChecks) {
    const text = read(file);
    // 仅检查相关 job 段落
    const idx = text.search(anchor);
    const seg = idx >= 0 ? text.slice(idx, idx + 2000) : text;
    const hit = hasContinueOnError(seg);
    console.log(`  ${name}: ${hit ? '❌ continue-on-error' : 'OK'}`);
    if (hit) failures.push(`${name} 存在 continue-on-error: true（应 fail-closed）`);
  }

  // 3. mutants kill-rate fail-closed
  const mutants = read(MUTANTS_YML);
  const mutantsFailClosed = /kill.rate[^\n]*\n(?:.*\n){0,8}.*exit\s+1/s.test(mutants) ||
    /RATE.*&lt;.*THRESHOLD[\s\S]{0,200}exit\s+1/.test(mutants);
  console.log(`\nmutants kill-rate fail-closed（exit 1）: ${mutantsFailClosed ? 'OK' : '❌'}`);
  if (!mutantsFailClosed) {
    failures.push('mutants.yml kill-rate 门禁未 fail-closed（需 exit 1）');
  }

  // 4. 性能回归门禁接线
  for (const [name, file] of [['benchmark.yml', BENCHMARK_YML], ['ai-facade-perf-gate.yml', AI_PERF_YML]]) {
    const text = read(file);
    const wired = /regression_gate\.sh/.test(text);
    console.log(`\n${name} 调用 regression_gate.sh: ${wired ? 'OK' : '❌'}`);
    if (!wired) failures.push(`${name} 未调用 scripts/perf/regression_gate.sh`);
  }

  // 4b. 端到端性能门禁接线（HTTP 负载，非 criterion 微基准）
  {
    const text = read(E2E_PERF_YML);
    const wired = /scripts\/perf-regression\.js/.test(text);
    const hasInit = /--init/.test(text);
    const noBaselineFails = /--init[\s\S]{0,800}?exit\s+1/.test(text);
    console.log(`\nperf-gate.yml 调用 perf-regression.js: ${wired ? 'OK' : '❌'}`);
    if (!wired) {
      failures.push('perf-gate.yml 未调用 scripts/perf-regression.js（端到端性能门禁脱钩）');
    } else {
      console.log(`perf-gate.yml 基线播种（--init）: ${hasInit ? 'OK' : '❌'}`);
      if (!hasInit) failures.push('perf-gate.yml 缺少无基线时的 --init 播种路径');
      console.log(`perf-gate.yml 无基线判定失败（exit 1）: ${noBaselineFails ? 'OK' : '❌'}`);
      if (!noBaselineFails) {
        failures.push('perf-gate.yml 在无指纹基线时未判定失败（会产生"假绿"）');
      }
    }
  }

  // 5. 禁用 workflow
  const disabled = fs.existsSync(WF_DIR)
    ? fs.readdirSync(WF_DIR).filter((f) => f.endsWith('.disabled'))
    : [];
  console.log(`\n禁用的 workflow（*.disabled）: ${disabled.length > 0 ? disabled.join(', ') : '无'}`);
  if (disabled.length > 0) {
    failures.push(`存在禁用 workflow：${disabled.join(', ')}（应重新启用或删除）`);
  }

  if (failures.length > 0) {
    console.error('\n❌ 门禁 32 失败：');
    for (const f of failures) console.error(`  - ${f}`);
    console.error('处置：统一阈值为 85；深度门禁移除 continue-on-error；kill-rate exit 1；接线 regression_gate.sh / perf-regression.js；删除/启用 .disabled。');
    process.exit(1);
  }
  console.log('\n✅ 门禁 32 通过（WARN ' + warns.length + ' 条）');
  for (const w of warns) console.log(`  ⚠ ${w}`);
  process.exit(0);
}

main();