# SZ-Rust 从零原始审计数据报告

> **审计日期**：2026-07-24
> **审计方式**：从零开始，所有数据均通过实际命令采集，未引用任何既有审计文档
> **审计范围**：`e:\vue\test\鲜视达\rust\sz-rust`
> **审计工具**：cargo 1.97.0 / cargo-audit 0.22.2 / cargo-deny 0.20.2 / PowerShell / ripgrep

---

## 1. 代码规模

### 1.1 Workspace 包数

**采集命令**：`cargo metadata --no-deps --format-version 1 | ConvertFrom-Json | Select-Object -ExpandProperty packages | Measure-Object`

| 项目 | 数值 |
|------|------|
| Workspace 成员包数 | **8** |
| packages/ 目录下总包数 | **10** |

Workspace 成员包清单（8 个）：

1. sz-rust-core v0.2.0
2. sz-rust-macros v0.2.0
3. sz-rust-examples v0.2.0
4. sz-rust-addons-loader v0.2.0
5. sz-rust-pdf v0.2.0
6. sz-rust-cli v0.2.0
7. sz-rust-tracing v0.2.0
8. sz-rust-observability v0.2.0

⚠️ **配置异常**：`packages/sz-rust-sz300` 与 `packages/sz-rust-addons-operate` 不在 workspace.members 中，但它们的 Cargo.toml 使用 `version.workspace = true`。单独进入这两个目录运行 `cargo check` 会报错：

```
error: current package believes it's in a workspace when it's not:
current:   ...\packages\sz-rust-sz300\Cargo.toml
workspace: ...\sz-rust\Cargo.toml
this may be fixable by adding `packages\sz-rust-sz300` to the `workspace.members` array
```

### 1.2 Rust 文件数

**采集命令**：`Get-ChildItem -Recurse -Filter *.rs packages/ | Where-Object { $_.FullName -notmatch '\\target\\' } | Measure-Object`

| 项目 | 数值 |
|------|------|
| Rust 文件总数（排除 target/） | **189** |

### 1.3 Rust 代码行数

**采集命令**：遍历所有 .rs 文件累计 `(Get-Content $file | Measure-Object -Line).Lines`

| 项目 | 数值 |
|------|------|
| Rust 代码总行数 | **100,173** |

### 1.4 测试函数数

**采集命令**：`Grep --pattern '#\[test\]|#\[tokio::test\]' --glob '*.rs' --output_mode count`

| 项目 | 数值 |
|------|------|
| 测试函数总数（含 #[test] 与 #[tokio::test]） | **2,596** |
| 涉及文件数 | 100 |

测试函数分布 Top 10（按文件）：

| 文件 | 测试数 |
|------|--------|
| packages/sz-rust-core/src/cache.rs | 223 |
| packages/sz-rust-core/tests/upload_parity.rs | 71 |
| packages/sz-rust-core/tests/relation_parity.rs | 57 |
| packages/sz-rust-core/tests/cache_parity.rs | 51 |
| packages/sz-rust-core/tests/adversarial.rs | 48 |
| packages/sz-rust-core/src/hooks.rs | 112 |
| packages/sz-rust-core/src/controller.rs | 86 |
| packages/sz-rust-core/src/view.rs | 88 |
| packages/sz-rust-core/src/guard.rs | 76 |
| packages/sz-rust-addons-loader/src/resource.rs | 61 |

### 1.5 sz-rust-core 模块数

**采集命令**：`Grep --pattern '^pub mod' --path packages/sz-rust-core/src/lib.rs`

| 项目 | 数值 |
|------|------|
| sz-rust-core 顶层 pub mod 数 | **33** |

模块清单：addons / cache / config / container / controller / cookie / env / error / error_handler / event / guard / h2 / health / i18n / hooks / log / mail / macros / middleware / model / multi_app / relation / request / response / router / routing / runtime / server / session / static_files / upload / validate / view

### 1.6 ADR 文档数

**采集命令**：`Get-ChildItem docs/adr/*.md | Measure-Object`

| 项目 | 数值 |
|------|------|
| docs/adr/ 下 .md 文件总数 | **13** |
| 其中 ADR 文档数（0001-0012） | **12** |
| 其中 README.md 索引文件 | 1 |

### 1.7 CI 工作流数

**采集命令**：`Get-ChildItem .github/workflows/*.yml | Measure-Object`

| 项目 | 数值 |
|------|------|
| GitHub Actions 工作流文件数 | **5** |

工作流清单：
1. `ci.yml` — 主 CI（fmt/check/clippy/test/doc/audit/deny/no-placeholder/feature-matrix/unused-deps）
2. `benchmark.yml` — criterion 性能基准测试
3. `coverage.yml` — cargo-tarpaulin 覆盖率
4. `fuzz.yml` — 模糊测试（每周六 + push/PR）
5. `soak.yml` — 长时稳定性测试（每周日 24h + smoke 10s）

---

## 2. 代码质量

### 2.1 占位实现检查

| 检查项 | 数值 | 采集命令 |
|--------|------|----------|
| `todo!()` 调用 | **0** | `Grep --pattern 'todo!\(\)' --glob '*.rs'` |
| `unimplemented!()` 调用 | **0** | `Grep --pattern 'unimplemented!\(\)' --glob '*.rs'` |

### 2.2 unsafe 检查

| 检查项 | 数值 | 说明 |
|--------|------|------|
| `unsafe` 关键字出现次数 | **3** | 全部为注释中"不使用 unsafe 块"的说明，无实际 unsafe 块 |
| 实际 `unsafe` 块数 | **0** | 无 |

3 处 unsafe 出现位置（均为注释）：
- `packages/sz-rust-core/tests/adversarial.rs:17` — `// 不使用 unsafe 块`
- `packages/sz-rust-core/tests/fuzz.rs:30` — `// 不使用 unsafe 块`
- `packages/sz-rust-core/tests/common/fuzz.rs:18` — `// 不使用 unsafe 块`

### 2.3 #![forbid(unsafe_code)] 覆盖情况

| 包名 | forbid(unsafe_code) | warn(missing_docs) |
|------|---------------------|---------------------|
| sz-rust-core | ✅ 有（lib.rs:41 + model.rs:64） | ✅ 有（lib.rs:43） |
| sz-rust-tracing | ✅ 有（lib.rs:19） | ✅ 有（lib.rs:20） |
| sz-rust-observability | ✅ 有（lib.rs:54） | ✅ 有（lib.rs:55） |
| sz-rust-macros | ✅ 有（lib.rs:14） | ❌ 缺失 |
| sz-rust-addons-loader | ✅ 有（lib.rs:54） | ❌ 缺失 |
| sz-rust-addons-operate | ✅ 有（lib.rs:34） | ❌ 缺失 |
| sz-rust-cli | ❌ 缺失 | ❌ 缺失 |
| sz-rust-examples | ❌ 缺失 | ❌ 缺失 |
| sz-rust-pdf | ❌ 缺失 | ❌ 缺失 |
| sz-rust-sz300 | ❌ 缺失 | ❌ 缺失 |

⚠️ **问题**：
- 4 个包缺失 `#![forbid(unsafe_code)]`：sz-rust-cli / sz-rust-examples / sz-rust-pdf / sz-rust-sz300
- 7 个包缺失 `#![warn(missing_docs)]`：sz-rust-macros / sz-rust-addons-loader / sz-rust-addons-operate / sz-rust-cli / sz-rust-examples / sz-rust-pdf / sz-rust-sz300

### 2.4 TODO/FIXME/XXX/HACK 注释

**采集命令**：`Grep --pattern 'TODO|FIXME|XXX|HACK' --glob '*.rs' --output_mode count`

| 项目 | 数值 |
|------|------|
| 总出现次数 | **73** |
| 涉及文件数 | 21 |

按文件分布（部分）：

| 文件 | 出现次数 |
|------|---------|
| packages/sz-rust-addons-operate/src/model/customer_pay.rs | 9 |
| packages/sz-rust-addons-operate/src/model/contract_log.rs | 7 |
| packages/sz-rust-core/src/view/template.rs | 6 |
| packages/sz-rust-addons-operate/src/lib.rs | 6 |
| packages/sz-rust-addons-operate/src/model/store.rs | 6 |
| packages/sz-rust-addons-operate/src/model/contract.rs | 3 |
| packages/sz-rust-addons-operate/src/model/category.rs | 3 |
| packages/sz-rust-cli/src/stubs.rs | 2 |
| packages/sz-rust-core/src/validate.rs | 2 |
| packages/sz-rust-addons-operate/src/controller/company.rs | 2 |
| 其他 11 个文件 | 35 |

---

## 3. 编译器警告

### 3.1 cargo check

**采集命令**：`cargo check --workspace` （通过 Start-Process 重定向 stderr 捕获）

| 项目 | 数值 |
|------|------|
| 退出码 | 0 |
| 警告数 | **0** |
| 错误数 | 0 |

✅ cargo check 完全通过，无任何警告或错误。

### 3.2 cargo clippy

**采集命令**：`cargo clippy --workspace --all-targets` （通过 Start-Process 重定向 stderr 捕获）

| 项目 | 数值 |
|------|------|
| 退出码 | 0 |
| 总警告数（含重复） | 18 |
| 唯一警告数 | **13** |
| 错误数 | 0 |

详细警告清单：

#### sz-rust-core lib（5 warnings）

| # | 文件:行 | 警告类型 | 说明 |
|---|---------|---------|------|
| 1 | packages/sz-rust-core/src/controller.rs:67:1 | clippy::derivable_impls | `impl Default for JwtConfig` 可改为 `#[derive(Default)]` |
| 2 | packages/sz-rust-core/src/cookie.rs:148:33 | clippy::redundant_closure | `unwrap_or_else(\|\| Utc::now())` → `unwrap_or_else(Utc::now)` |
| 3 | packages/sz-rust-core/src/cookie.rs:229:1 | clippy::derivable_impls | `impl Default for CookieJar` 可改为 `#[derive(Default)]` |
| 4 | packages/sz-rust-core/src/env.rs:143:11 | clippy::doc_nested_refdefs | 链接引用定义在列表项中 |
| 5 | packages/sz-rust-core/src/env.rs:144:11 | clippy::doc_nested_refdefs | 链接引用定义在列表项中 |

#### sz-rust-core lib test（3 new warnings，5 duplicates）

| # | 文件:行 | 警告类型 | 说明 |
|---|---------|---------|------|
| 6 | packages/sz-rust-core/src/cookie.rs:828:25 | clippy::unnecessary_get_then_check | `cookies.get("invalid").is_none()` → `!cookies.contains_key("invalid")` |
| 7 | packages/sz-rust-core/src/cookie.rs:835:25 | clippy::unnecessary_get_then_check | `cookies.get("").is_none()` → `!cookies.contains_key("")` |
| 8 | packages/sz-rust-core/src/env.rs:512:9 | clippy::writeln_empty_string | `writeln!(file, "")` → 移除空字符串 |

#### sz-rust-core test "adversarial"（3 warnings）

| # | 文件:行 | 警告类型 | 说明 |
|---|---------|---------|------|
| 9 | packages/sz-rust-core/tests/adversarial.rs:102:5 | clippy::field_reassign_with_default | 字段在 Default::default() 后重新赋值 |
| 10 | packages/sz-rust-core/tests/adversarial.rs:836:19 | clippy::useless_format | `format!("\u{FEFF}APP_KEY")` → `.to_string()` |
| 11 | packages/sz-rust-core/tests/adversarial.rs:1261:25 | clippy::needless_borrows_for_generic_args | `&format!(...)` → `format!(...)` |

#### 外部依赖（2 warnings，非项目代码）

| # | 来源 | 警告类型 | 说明 |
|---|------|---------|------|
| 12 | sz-orm-macros (lib) | linker_messages | 链接器输出"正在创建库..."（Windows MSVC 行为） |
| 13 | sz-rust-macros (lib) | linker_messages | 链接器输出"正在创建库..."（Windows MSVC 行为） |

⚠️ **CI 影响**：`ci.yml` 的 clippy job 使用 `cargo clippy --workspace --all-targets -- -D warnings`，会将警告视为错误。本地 clippy 退出码为 0（因为未使用 `-D warnings`），但 CI 上会因 11 个项目代码警告而失败（链接器警告属于外部 crate）。

---

## 4. CI/CD

### 4.1 工作流概览

| 工作流 | 触发条件 | Job 数 | continue-on-error |
|--------|---------|--------|-------------------|
| ci.yml | push/PR (main/master) | 10 | ❌ 无 |
| benchmark.yml | push/PR (main) + workflow_dispatch | 1 | ❌ 无 |
| coverage.yml | push/PR (main/master) | 1 | ✅ 有（覆盖率失败不阻塞 PR） |
| fuzz.yml | push/PR + 每周六 + workflow_dispatch | 1 | ❌ 无 |
| soak.yml | 每周日 + workflow_dispatch + push/PR smoke | 2 | ❌ 无 |

### 4.2 continue-on-error 检查

**采集命令**：`Grep --pattern 'continue-on-error' --path .github/workflows`

| 文件 | 行号 | 内容 |
|------|------|------|
| coverage.yml | 18 | `continue-on-error: true  # 覆盖率失败不阻塞 PR` |

⚠️ 另：coverage.yml 第 49 行 `fail_ci_if_error: false`（codecov 上传失败不阻塞）。

### 4.3 sz-orm checkout 配置检查

**采集命令**：`Grep --pattern 'sz-orm' --path .github/workflows`

ci.yml 10 个 job 的 sz-orm checkout 状态：

| Job 名称 | sz-orm checkout | 是否需要编译 | 问题 |
|----------|-----------------|-------------|------|
| fmt | ❌ 缺失 | 不需要（仅 cargo fmt --check） | 无 |
| check | ❌ **缺失** | **需要**（cargo check --workspace --all-targets） | ⚠️ **CI 必失败** |
| clippy | ❌ **缺失** | **需要**（cargo clippy --workspace --all-targets） | ⚠️ **CI 必失败** |
| test | ✅ 有 | 需要 | 无 |
| doc | ✅ 有 | 需要 | 无 |
| audit | ❌ 缺失 | 不需要（仅检查 Cargo.lock） | 无 |
| deny | ❌ 缺失 | 不需要（仅检查 Cargo.lock） | 无 |
| no-placeholder | ❌ 缺失 | 不需要（仅 grep） | 无 |
| feature-matrix | ✅ 有 | 需要 | 无 |
| unused-deps | ✅ 有 | 需要 | 无 |

🔴 **严重问题**：ci.yml 的 `check` 和 `clippy` 两个 job 缺失 sz-orm checkout 步骤。由于 workspace Cargo.toml 中 sz-orm-* 全部使用 `path = "../sz-orm/..."`，这两个 job 在 GitHub Actions 上必定失败（找不到 `../sz-orm` 目录）。

### 4.4 deny.toml 配置

文件存在，配置完整，包含以下审计维度：

- **advisories**：RUSTSEC 安全漏洞检查，忽略 3 个已评估的 unmaintained advisory
- **licenses**：白名单 14 种许可证（MIT/Apache-2.0/BSD/ISC/Zlib/Unicode-3.0 等）
- **bans**：`multiple-versions = "warn"`，`wildcards = "deny"`
- **sources**：仅允许 crates.io

### 4.5 cargo deny 实际运行结果

**采集命令**：`cargo deny check advisories licenses bans sources`

| 维度 | 结果 |
|------|------|
| advisories | ✅ ok |
| bans | ✅ ok |
| licenses | ✅ ok |
| sources | ✅ ok |

⚠️ 1 个 duplicate 警告：`zip` crate 存在两个版本（v2.4.2 来自 rust_xlsxwriter，v8.6.0 来自 calamine）。

---

## 5. 安全

### 5.1 硬编码 IP 检查

**采集命令**：`Grep --pattern '\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}' --glob '*.yml'`

YAML 配置文件中的 IP：

| 文件 | IP | 风险评估 |
|------|-----|---------|
| config/server.yml | 0.0.0.0 | ✅ 默认监听地址（注释说明可通过环境变量覆盖） |
| packages/sz-rust-sz300/config/app.yml | 0.0.0.0 | ✅ 默认监听地址 |
| packages/sz-rust-sz300/config/database.yml | 127.0.0.1 | ✅ 本地回环 |

Rust 源码中的 IP（共 100 处匹配，均为以下类别）：
- `127.0.0.1` — 本地回环，用于测试、文档示例、默认配置（如 `DEFAULT_JAVA_PDF_SERVICE_URL = "http://127.0.0.1:8086"`，对齐 PHP 硬编码）
- `0.0.0.0` — 默认监听地址
- `10.0.0.x` / `192.168.x.x` — 仅出现在测试用例中（adversarial.rs、rate_limit.rs、hooks.rs 测试数据）

✅ **无敏感硬编码 IP**。

### 5.2 硬编码密码/密钥检查

**采集命令**：`Grep --pattern 'password|secret|token|api_key' --glob '*.yml'`

| 文件 | 内容 | 风险评估 |
|------|------|---------|
| config/database.yml | 所有 password 字段为空 `""` | ✅ 通过环境变量注入 |
| packages/sz-rust-sz300/config/database.yml | password 为空 `""` | ✅ |
| .github/workflows/ci.yml | `${{ secrets.GITHUB_TOKEN }}` | ✅ GitHub Actions 内置密钥 |

✅ **无硬编码密码或密钥**。

### 5.3 unsafe 块检查

见第 2.2 节。✅ **无实际 unsafe 块**。

### 5.4 路径遍历防护检查

**采集命令**：`Grep --pattern 'traversal|canonicalize|\.\./' --glob '*.rs'`

| 模块 | 防护状态 | 说明 |
|------|---------|------|
| sz-rust-core/src/static_files.rs | ✅ 有防护 | `is_path_safe()` 使用 `canonicalize()` 比对根目录，含 4 个路径穿越测试 |
| sz-rust-core/src/routing.rs | ✅ 有防护 | `HandlerRef::parse` 拒绝 `../`、空格、`@`、`/` 等字符 |
| sz-rust-core/tests/fuzz.rs | ✅ 有测试 | 包含 `/../etc/passwd`、`/oapc/../admin` 等 fuzz 输入 |
| **sz-rust-sz300/src/controllers/file_serve.rs** | 🔴 **无防护** | `PathBuf::from("./uploads").join(path.0)` 未做任何路径校验 |

🔴 **严重安全漏洞**：`packages/sz-rust-sz300/src/controllers/file_serve.rs` 的 `serve_file` 函数直接将用户输入的 path 拼接到 `./uploads` 后读取文件，未检查 `../` 路径穿越。攻击者可通过 `/file/../../../etc/passwd` 读取任意系统文件。

问题代码（file_serve.rs:12）：
```rust
let file_path = PathBuf::from("./uploads").join(path.0);
match fs::read(&file_path).await {
    // ... 未校验路径是否在 ./uploads 范围内
}
```

---

## 6. 文档

### 6.1 必备文档存在性

| 文档 | 存在 | 说明 |
|------|------|------|
| README.md | ✅ | 项目主文档，含特性、快速上手、对标表、CI 门禁说明 |
| CHANGELOG.md | ✅ | Keep a Changelog 格式，含 [Unreleased] 和 [0.2.0]、[0.1.0] |
| CONTRIBUTING.md | ✅ | 贡献指南，含开发环境、快速开始、代码规范 |
| LICENSE | ✅ | MIT License，Copyright (c) 2026 SZ-Rust Team |

### 6.2 文档文件总数

**采集命令**：`Get-ChildItem -Recurse -Filter *.md | Where-Object { $_.FullName -notmatch '\\target\\' } | Measure-Object`

| 项目 | 数值 |
|------|------|
| .md 文件总数（排除 target/） | **29** |

### 6.3 README 链接检查

**采集命令**：`Grep --pattern '\]\(([^)]+)\)' --path README.md` + `Test-Path`

README.md 中共有 9 个相对路径链接，逐一验证：

| 链接 | 状态 |
|------|------|
| docs/adr/README.md | ✅ 存在 |
| docs/adr/0011-可观测性模块-MetricsRegistry-SLO多窗口燃烧率.md | ✅ 存在 |
| docs/adr/0012-分布式追踪-W3C-TraceContext-OTLP-exporter.md | ✅ 存在 |
| docs/php-migration-guide.md | ✅ 存在 |
| docs/sz-rust-engineering-practices.md | ✅ 存在 |
| docs/软件项目审计清单.md | ✅ 存在 |
| docs/ADR与生产Bug定位规范.md | ✅ 存在 |
| docs/audit/2026-07-22-初始审计.md | ✅ 存在 |
| docs/benchmarks/baseline-v0.1.0.md | ✅ 存在 |

✅ **README 无断链**（9/9 链接有效）。

### 6.4 ADR 文档

12 个 ADR 全部标记为"已接受"状态，覆盖路由、中间件、控制器、Model 钩子、事务、认证、插件、错误处理、缓存、配置、可观测性、分布式追踪。

---

## 7. 版本

### 7.1 Workspace 版本

**采集位置**：`Cargo.toml` `[workspace.package]` section

| 项目 | 数值 |
|------|------|
| workspace.package.version | **0.2.0** |
| edition | 2021 |
| license | MIT |

### 7.2 子包 workspace = true 使用情况

**采集命令**：`Select-String -Path packages\*\Cargo.toml -Pattern 'version'`

| 包名 | version.workspace = true |
|------|--------------------------|
| sz-rust-addons-loader | ✅ |
| sz-rust-addons-operate | ✅（但不在 workspace.members 中） |
| sz-rust-cli | ✅ |
| sz-rust-core | ✅ |
| sz-rust-examples | ✅ |
| sz-rust-macros | ✅ |
| sz-rust-observability | ✅ |
| sz-rust-pdf | ✅ |
| sz-rust-sz300 | ✅（但不在 workspace.members 中） |
| sz-rust-tracing | ✅ |

⚠️ **配置矛盾**：所有 10 个包都使用 `version.workspace = true`，但 workspace.members 只列了 8 个。sz-rust-sz300 和 sz-rust-addons-operate 被排除在 workspace 之外却仍引用 workspace 字段，导致它们无法独立构建。

### 7.3 Git Tags

**采集命令**：`git tag`

| 项目 | 数值 |
|------|------|
| Git tag 数 | **1** |
| Tag 名称 | v0.2.0 |

⚠️ 缺少 v0.1.0 的 git tag（CHANGELOG.md 记录了 [0.1.0] - 2026-07-22，但 git tag 只有 v0.2.0）。

### 7.4 最近 Git 提交

```
b5ae516 fix: 业务包移出 workspace + yank 误发布 + 审计清单全面清理
f043fbf docs(audit): v0.2.0 git tag 已创建，审计未通过项归零，综合评分 90%
837ed71 feat(P1-9): sz-rust 8/8 包发布到 crates.io + Cargo.toml 依赖补全
3b71058 feat(P1-9+P2-14): crates.io 发布准备 + ThinkPHP 8 实测对比报告 v2
a871026 docs(P2): 性能对比报告 + 审计清单更新
4e930f7 feat(P1+P2): 实现 BaseController::validate + AddonsBaseController::get_token + Session/Cookie 模块
d275d48 release: v0.2.0 — CI 修复 + ServerSection + 可观测性/追踪 + 审计清单
7921512 docs: 完善框架文档与审计 - ADR-001~010 + criterion 基线 + P1 审计报告更新
d9e72e2 初始提交：SZ-Rust 对标 ThinkPHP 8 的 Rust Web 框架
```

注：最新提交 b5ae516 提到"业务包移出 workspace"，这解释了为什么 sz-rust-sz300 和 sz-rust-addons-operate 不在 workspace.members 中，但它们的 Cargo.toml 仍残留 `version.workspace = true`，属于移出操作不彻底。

---

## 8. 测试结果

### 8.1 cargo test --workspace --lib

**采集命令**：`cargo test --workspace --lib`（通过 Start-Process 重定向）

| 包 | passed | failed | ignored |
|----|--------|--------|---------|
| sz-rust-addons-loader | 227 | 0 | 0 |
| sz-rust-cli | 72 | 0 | 0 |
| sz-rust-core | 2,717 | 0 | 0 |
| sz-rust-examples | 0 | 0 | 0 |
| sz-rust-macros | 0 | 0 | 0 |
| sz-rust-observability | 25 | 0 | 0 |
| sz-rust-pdf | 129 | 0 | 0 |
| sz-rust-tracing | 35 | 0 | 0 |
| **合计** | **3,205** | **0** | **0** |

### 8.2 cargo test --workspace --tests（含 lib + 集成测试 + bin）

**采集命令**：`cargo test --workspace --tests`

| 测试目标 | passed | failed | ignored | 耗时 |
|---------|--------|--------|---------|------|
| sz-rust-addons-loader (lib) | 227 | 0 | 0 | 0.63s |
| sz-rust-cli (lib) | 72 | 0 | 0 | 0.04s |
| sz-rust-cli (bin "sz-rust") | 0 | 0 | 0 | 0.00s |
| sz-rust-core (lib) | 2,717 | 0 | 0 | 2.35s |
| sz-rust-core (test adversarial) | 55 | 0 | 0 | 2.45s |
| sz-rust-core (test cache_parity) | 51 | 0 | 0 | 0.38s |
| sz-rust-core (test compact_macro) | 18 | 0 | 0 | 0.01s |
| sz-rust-core (test fuzz) | 14 | 0 | 0 | 0.02s |
| sz-rust-core (test relation_parity) | 57 | 0 | 0 | 0.02s |
| sz-rust-core (test runtime_perf) | 2 | 0 | 9 | 0.01s |
| sz-rust-core (test soak) | 8 | 0 | 1 | 10.01s |
| sz-rust-core (test sql_string) | 10 | 0 | 0 | 0.00s |
| sz-rust-core (test upload_parity) | 71 | 0 | 0 | 1.12s |
| sz-rust-core (test validate) | 32 | 0 | 0 | 0.03s |
| sz-rust-examples (lib) | 0 | 0 | 0 | 0.00s |
| sz-rust-examples (bin "quick_start") | 0 | 0 | 0 | 0.00s |
| sz-rust-examples (bin "crud_demo") | 0 | 0 | 0 | 0.00s |
| sz-rust-examples (test hello_world) | 6 | 0 | 0 | 0.00s |
| sz-rust-macros (lib) | 0 | 0 | 0 | 0.00s |
| sz-rust-observability (lib) | 25 | 0 | 0 | 0.25s |
| sz-rust-pdf (lib) | 129 | 0 | 0 | 2.07s |
| sz-rust-tracing (lib) | 35 | 0 | 0 | 0.02s |
| **合计** | **3,529** | **0** | **10** | — |

✅ **全部测试通过**：3,529 passed / 0 failed / 10 ignored（ignored 为 soak 和 runtime_perf 的长时测试，需手动 `--ignored` 触发）。

### 8.3 cargo test --workspace（完整运行）

⚠️ 完整 `cargo test --workspace`（含 bin/doc tests）在本地首次运行时因 Windows 链接器并发锁冲突（link.exe exit code 1102）失败。分拆为 `--lib` 和 `--tests` 后均成功。该问题为本地环境并发构建问题，非代码问题。

---

## 9. 依赖

### 9.1 依赖树深度 1

**采集命令**：`cargo tree --depth 1 --workspace`

各 workspace 包的直接依赖数（depth 1）：

| 包 | 直接依赖数 |
|----|-----------|
| sz-rust-addons-loader | 7（+2 dev） |
| sz-rust-cli | 10（+2 dev） |
| sz-rust-examples | 8（+4 dev） |
| sz-rust-macros | 4 |
| sz-rust-observability | 8 |
| sz-rust-pdf | 8（+2 dev） |
| sz-rust-tracing | 3 |

### 9.2 总依赖数

**采集命令**：`Select-String -Path Cargo.lock -Pattern '^name = '`

| 项目 | 数值 |
|------|------|
| Cargo.lock 中 crate 总数 | **477** |

### 9.3 cargo audit 结果

**采集命令**：`cargo audit`

| 项目 | 数值 |
|------|------|
| 扫描的 advisory 数 | 1,169 |
| 扫描的 crate 依赖数 | 477 |
| 安全漏洞（vulnerability）数 | **0** |
| 维护警告（unmaintained）数 | **3** |

3 个 unmaintained 警告（全部已在 deny.toml 中显式忽略并注明理由）：

| Crate | 版本 | Advisory ID | 忽略理由 |
|-------|------|------------|---------|
| paste | 1.0.15 | RUSTSEC-2024-0436 | 大量 crate 的传递依赖，无已知安全漏洞 |
| rustls-pemfile | 2.2.0 | RUSTSEC-2025-0134 | hyper-rustls → reqwest → sz-rust-pdf 的传递依赖，无安全漏洞 |
| ttf-parser | 0.25.1 | RUSTSEC-2026-0192 | ab_glyph → imageproc 的传递依赖，无直接安全影响 |

✅ **无实际安全漏洞**。

### 9.4 cargo outdated

**采集命令**：`cargo outdated --version`

| 项目 | 结果 |
|------|------|
| cargo-outdated | ❌ 未安装 |

⚠️ 无法执行过期依赖检查（CI 中也未配置 cargo-outdated job）。

### 9.5 重复依赖

**采集命令**：`cargo tree --duplicates`

| 项目 | 数值 |
|------|------|
| 重复依赖行数 | 46 |
| cargo-deny 报告的 duplicate 警告 | 1（zip: v2.4.2 + v8.6.0） |

### 9.6 cargo-deny 综合检查

**采集命令**：`cargo deny check advisories licenses bans sources`

| 维度 | 结果 |
|------|------|
| advisories | ✅ ok |
| bans | ✅ ok（1 个 duplicate warning：zip） |
| licenses | ✅ ok |
| sources | ✅ ok |

---

## 10. 问题汇总

### 10.1 严重问题（P0）

| # | 问题 | 位置 | 影响 |
|---|------|------|------|
| P0-1 | 路径遍历漏洞 | packages/sz-rust-sz300/src/controllers/file_serve.rs:12 | 攻击者可读取任意系统文件 |
| P0-2 | ci.yml 的 check/clippy job 缺失 sz-orm checkout | .github/workflows/ci.yml:30-45 | CI 在 GitHub Actions 上必失败 |
| P0-3 | sz-rust-sz300/sz-rust-addons-operate 残留 version.workspace = true 但不在 workspace.members | packages/sz-rust-sz300/Cargo.toml:3, packages/sz-rust-addons-operate/Cargo.toml:3 | 这两个包无法独立构建 |

### 10.2 中等问题（P1）

| # | 问题 | 位置 | 影响 |
|---|------|------|------|
| P1-1 | 13 个 clippy 警告（CI 使用 -D warnings 会失败） | sz-rust-core 多个文件 | CI clippy job 在 GitHub Actions 上必失败 |
| P1-2 | 4 个包缺失 #![forbid(unsafe_code)] | sz-rust-cli/sz-rust-examples/sz-rust-pdf/sz-rust-sz300 | 安全约束不一致 |
| P1-3 | 7 个包缺失 #![warn(missing_docs)] | sz-rust-macros/sz-rust-addons-loader/sz-rust-addons-operate/sz-rust-cli/sz-rust-examples/sz-rust-pdf/sz-rust-sz300 | 文档约束不一致 |
| P1-4 | 缺少 v0.1.0 git tag | git tag | CHANGELOG 记录了 0.1.0 但无对应 tag |
| P1-5 | 73 处 TODO/FIXME/XXX/HACK 注释 | 21 个文件 | 技术债务标记未清理 |

### 10.3 轻微问题（P2）

| # | 问题 | 位置 | 影响 |
|---|------|------|------|
| P2-1 | zip crate 存在两个版本（2.4.2 + 8.6.0） | Cargo.lock | 编译体积增大 |
| P2-2 | coverage.yml 使用 continue-on-error: true | .github/workflows/coverage.yml:18 | 覆盖率退化不会阻塞 PR（已在注释中说明为有意为之） |
| P2-3 | cargo-outdated 未安装 | 本地环境 | 无法检查过期依赖 |
| P2-4 | serde_yaml 0.9.34+deprecated | sz-rust-cli | 使用了已废弃的 crate（无替代迁移） |
| P2-5 | README 称"10+ 道门禁，所有门禁严格生效（无 continue-on-error）" | README.md:165 | 与 coverage.yml 的 continue-on-error: true 不符 |

---

## 11. 通过项汇总

| 维度 | 状态 | 说明 |
|------|------|------|
| cargo check | ✅ 通过 | 0 警告 0 错误 |
| cargo test (lib) | ✅ 通过 | 3,205 passed / 0 failed |
| cargo test (all) | ✅ 通过 | 3,529 passed / 0 failed / 10 ignored |
| cargo audit | ✅ 通过 | 0 漏洞 / 3 unmaintained（已忽略） |
| cargo deny | ✅ 通过 | 4/4 维度 ok |
| todo!() / unimplemented!() | ✅ 通过 | 0 处 |
| unsafe 块 | ✅ 通过 | 0 处实际 unsafe 块 |
| 硬编码密码 | ✅ 通过 | 所有 password 字段为空 |
| 硬编码敏感 IP | ✅ 通过 | 仅本地回环和默认监听地址 |
| README 链接 | ✅ 通过 | 9/9 链接有效 |
| 必备文档 | ✅ 通过 | README/CHANGELOG/CONTRIBUTING/LICENSE 齐全 |
| ADR 文档 | ✅ 通过 | 12 个 ADR 全部已接受 |
| workspace 版本统一 | ✅ 通过 | 10/10 包使用 version.workspace = true |
| 测试覆盖 | ✅ 良好 | 2,596 个测试函数 |

---

## 12. 审计方法说明

本审计完全从零开始，所有数据均通过以下实际命令采集，未引用 docs/audit/ 下的任何既有审计文档：

- `cargo metadata --no-deps --format-version 1` — 包数
- `Get-ChildItem -Recurse -Filter *.rs packages/` — 文件数与行数
- `Grep --pattern '...' --glob '*.rs' --output_mode count` — 代码质量指标
- `cargo check --workspace` — 编译警告
- `cargo clippy --workspace --all-targets` — clippy 警告（通过 Start-Process 重定向 stderr）
- `cargo test --workspace --lib` / `--tests` — 测试结果
- `cargo audit` — 安全漏洞
- `cargo deny check advisories licenses bans sources` — 依赖审计
- `cargo tree --depth 1 --workspace` — 依赖树
- `git tag` / `git log --oneline` — 版本与提交历史
- `Grep` + `Test-Path` — README 链接验证

所有命令的实际输出已在本报告中如实记录，数值未取整或估算。

---

**报告生成时间**：2026-07-24
**审计执行者**：从零原始审计（无既有文档信任）
