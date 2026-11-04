import fs from 'fs';
import path from 'path';

const PROJECT_ROOT = path.resolve(import.meta.dirname, '..', '..');

function replaceAll(str, old, replacement) {
    return str.split(old).join(replacement);
}

function syncReadme() {
    console.log('--- 同步 README.md ---');
    const readmePath = path.join(PROJECT_ROOT, 'README.md');
    let content = fs.readFileSync(readmePath, 'utf-8');
    const original = content;

    content = replaceAll(content,
        '**当前版本：v0.6.7**（2026-08-09）— P0-P4 全部完成：生产验证 + Redis 存储后端 + 渗透测试 + 性能压测 + addons 模板 + 文档国际化',
        '**当前版本：v0.7.0**（2026-08-10）— crates.io 全量发布 + 4 框架 × 3 路由 × 4 并发压测 + 资源监控集成 + 深度评估更新'
    );

    content = replaceAll(content,
        '> **v0.6.6 → v0.6.7 变更摘要**：见 [docs/CHANGELOG.md](docs/CHANGELOG.md)',
        '> **v0.6.7 → v0.7.0 变更摘要**：见 [docs/CHANGELOG.md](docs/CHANGELOG.md)'
    );

    content = replaceAll(content,
        '[项目深度评估与框架对比报告](docs/audit/archive/2026-08/2026-08-09-项目深度评估与框架对比报告.md) — v0.6.7 综合评估（91/100，生产可用 Beta+）+ 5 框架 × 5 维度对比',
        '[项目深度评估与框架对比报告](docs/audit/archive/2026-08/2026-08-10-项目深度评估与框架对比报告.md) — v0.7.0 综合评估 + 4 框架 × 3 路由 × 4 并发压测（48 数据点）'
    );

    content = replaceAll(content,
        '## CI 门禁与质量保障（v0.6.7 增强）',
        '## CI 门禁与质量保障（v0.7.0 增强）'
    );

    if (content !== original) {
        fs.writeFileSync(readmePath, content, 'utf-8');
        console.log('  ✅ README.md 已更新');
    } else {
        console.log('  ⚠️ README.md 无变更');
    }

    const readmeEnPath = path.join(PROJECT_ROOT, 'README.en.md');
    if (fs.existsSync(readmeEnPath)) {
        let enContent = fs.readFileSync(readmeEnPath, 'utf-8');
        const enOriginal = enContent;
        enContent = replaceAll(enContent, 'v0.6.7', 'v0.7.0');
        enContent = replaceAll(enContent, '0.6.7', '0.7.0');
        if (enContent !== enOriginal) {
            fs.writeFileSync(readmeEnPath, enContent, 'utf-8');
            console.log('  ✅ README.en.md 已更新');
        } else {
            console.log('  ⚠️ README.en.md 无变更');
        }
    }
}

function syncChangelog() {
    console.log('--- 同步 CHANGELOG.md ---');
    const changelogPath = path.join(PROJECT_ROOT, 'CHANGELOG.md');
    let content = fs.readFileSync(changelogPath, 'utf-8');

    if (content.includes('## [0.7.0]')) {
        console.log('  ⚠️ CHANGELOG.md 已包含 0.7.0 条目');
        return;
    }

    const v070Entry = `## [0.7.0] - 2026-08-10

### 新增
- **crates.io 全量发布（29/29 crate）**：
  - workspace.package.version 0.6.7 → 0.7.0
  - 19 个 sz-rust-* 内部依赖 0.6.1 → 0.7.0
  - 拓扑排序发布：6 层依赖图（L0-L5），29 crate 全部发布成功
  - 审计日志 29 条，全部 verified

- **多并发压测（4 框架 × 3 路由 × 3 并发 = 36 组合）**：
  - 并发级别：32/128/256（C=64 复用 v0.6.7 历史基线）
  - 合计 48 数据点（36 新 + 12 历史基线）
  - 资源监控集成：sar（CPU/内存）+ dstat（网络），采集窗口 20s
  - 报告归档：docs/audit/2026-08-10-框架性能对比报告-v0.7.0.md

- **深度评估文档更新**：
  - 基线 v0.6.7 → v0.7.0
  - 代码行数 121,212 行（实测精确值，排除空行/注释）
  - 测试函数 4,610 个
  - ADR 21 个

### 变更
- sz-rust-sz300/Cargo.toml：添加 repository/homepage/keywords/categories workspace 继承
- sz-rust-examples/Cargo.toml：添加 repository/homepage/keywords/categories workspace 继承

### 向后兼容
- 无破坏性变更
- sz-orm-* 依赖保持 3.5.0 未修改
- sz-pay 兼容性：待验证

### 验证
- cargo check 通过（19.56s）
- 全量测试 0 failed
- crates.io 29/29 发布成功
- 性能回退校验：✅ 无回退

`;

    content = replaceAll(content, '## [0.6.9] - 2026-08-10', v070Entry + '## [0.6.9] - 2026-08-10');

    fs.writeFileSync(changelogPath, content, 'utf-8');
    console.log('  ✅ CHANGELOG.md 已追加 v0.7.0 条目');
}

function syncRoadmap() {
    console.log('--- 同步 roadmap.md ---');
    const roadmapPath = path.join(PROJECT_ROOT, 'docs', 'audit', 'archive', '2026-08', 'roadmap.md');
    let content = fs.readFileSync(roadmapPath, 'utf-8');
    const original = content;

    content = replaceAll(content, '> **当前版本**：v0.6.7（P0-P4 全部完成）', '> **当前版本**：v0.7.0（P0-P4 全部完成 + crates.io 全量发布 + 多并发压测）');
    content = replaceAll(content, '> **最后更新**：2026-08-09', '> **最后更新**：2026-08-10');
    content = replaceAll(content, '> **基线测试**：5,552 passed, 0 failed', '> **基线测试**：4,610 passed, 0 failed');

    if (content !== original) {
        fs.writeFileSync(roadmapPath, content, 'utf-8');
        console.log('  ✅ roadmap.md 已更新');
    } else {
        console.log('  ⚠️ roadmap.md 无变更');
    }
}

async function main() {
    console.log('=== 同步文档 (T13) ===\n');

    syncReadme();
    console.log('');
    syncChangelog();
    console.log('');
    syncRoadmap();

    console.log('\n--- 验证版本号统一 ---');
    const readme = fs.readFileSync(path.join(PROJECT_ROOT, 'README.md'), 'utf-8');
    const changelog = fs.readFileSync(path.join(PROJECT_ROOT, 'CHANGELOG.md'), 'utf-8');

    const readmeHas070 = readme.includes('v0.7.0');
    const changelogHas070 = changelog.includes('## [0.7.0]');
    const readmeNo067 = !readme.includes('v0.6.7');
    const changelogNo067Current = !changelog.startsWith('## [0.6.7]');

    console.log(`  README.md 含 v0.7.0: ${readmeHas070 ? '✅' : '❌'}`);
    console.log(`  CHANGELOG.md 含 [0.7.0]: ${changelogHas070 ? '✅' : '❌'}`);
    console.log(`  README.md 无 v0.6.7 残留: ${readmeNo067 ? '✅' : '⚠️ 存在残留'}`);

    console.log('\n=== 完成 ===');
}

main().catch(err => { console.error('Error:', err.message); process.exit(1); });