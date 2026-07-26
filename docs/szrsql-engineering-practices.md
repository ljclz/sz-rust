# SzRSQL 工程化实践规范

> **项目名**：SzRSQL（自研关系型数据库，PG 协议兼容）
> **适用范围**：szrsql workspace 全部 15 个 crate（含 fuzz）
> **版本**：v1.0  日期：2026-07-20
> **当前 Phase**：Phase 3（枚举类型实现）
> **来源**：sz-orm v0.2.1 工程实践提炼 + szrsql Phase 1-3 开发经验 + sz-orm 全面审查 6 Critical + 5 High + 7 伪实现教训
> **核心定位**：针对 **数据库内核级 AI 开发** 场景强化的工程化规范——AI 生成代码有独特的缺陷模式（虚假实现、panic 路径、锁中毒、feature 遗漏），本规范从门禁、审查、测试三个维度系统性防御

---

## 目录

1. [标准 7 道门禁](#一标准-7-道门禁)
2. [SzRSQL 特殊强化门禁](#二szrsql-特殊强化门禁)
3. [五维代码审查增强](#三五维代码审查增强)
4. [测试体系](#四测试体系)
5. [CI/CD 与开发流程](#五cicd-与开发流程)
6. [附录：sz-orm 教训 → szrsql 防御](#六附录sz-orm-教训--szrsql-防御)

---

## 一、标准 7 道门禁

7 道门禁是每次提交/合入的**最低准入条件**，任何一道失败即整体阻断，禁止 `--force` 绕过。

| 顺序 | 关卡 | 命令 | 当前状态 | 阻断条件 |
|------|------|------|---------|---------|
| 1 | `fmt` | `cargo fmt --all -- --check` | ✅ CI 已有 | 任何文件未格式化 |
| 2 | `check` | `cargo check --workspace --all-targets` | ✅ CI 已有（clippy 中隐式含） | 编译错误 |
| 3 | `clippy` | `cargo clippy --workspace --all-targets -- -D warnings` | ✅ CI 已有 | 任何 warning |
| 4 | `test` | `cargo test --workspace --locked` | ✅ CI 已有（含 lib+doc+集成+fuzz） | 任何测试失败 |
| 5 | `doc` | `cargo doc --workspace --no-deps --document-private-items` | ✅ CI 已有（`continue-on-error: true`） | 文档构建失败 / rustdoc 警告 |
| 6 | `audit` | `cargo audit` + `cargo deny check` | ⬜ CI 缺少 | 已知漏洞 / 许可证违规 |
| 7 | `integration` | `cargo test --workspace --test '*' -- --test-threads=4` | ⬜ 需独立 Job | 跨模块集成测试失败 |

### 现状说明

- **门禁 1-5 已存在** `.github/workflows/ci.yml`（fmt / clippy / test / doc 四个 Job），但 doc job 配置了 `continue-on-error: true`，建议在实际 Phase 交付前改为严格模式
- **门禁 6 缺失**：需要安装 `cargo-audit` + `cargo-deny`，当前 Cargo.toml 无 `deny.toml`
- **门禁 7 缺失**：当前 test job 统一执行 `cargo test --workspace`，需要将集成测试从单元测试中分离

### 1.1 门禁脚本

`scripts/gate.ps1`（szrsql 基础版本，含标准 7 关）：

```powershell
[CmdletBinding()]
param(
    [switch]$SkipAudit,
    [switch]$SkipIntegration
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

function Invoke-Gate([string]$name, [scriptblock]$action) {
    Write-Host "==> [$name] start..." -ForegroundColor Cyan
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $action
    if ($LASTEXITCODE -ne 0) {
        Write-Host "==> [$name] FAILED (exit $LASTEXITCODE)" -ForegroundColor Red
        exit 1
    }
    $sw.Stop()
    Write-Host "==> [$name] OK ($($sw.Elapsed.TotalSeconds)s)" -ForegroundColor Green
}

Invoke-Gate 'fmt'    { cargo fmt --all -- --check }
Invoke-Gate 'check'  { cargo check --workspace --all-targets }
Invoke-Gate 'clippy' { cargo clippy --workspace --all-targets -- -D warnings }
Invoke-Gate 'test'   { cargo test --workspace --locked }

# 门禁 5：文档构建（严格模式）
Invoke-Gate 'doc'    { cargo doc --workspace --no-deps --document-private-items }

if (-not $SkipAudit) {
    Invoke-Gate 'audit'  { cargo audit; cargo deny check }
}
if (-not $SkipIntegration) {
    Invoke-Gate 'integration' { cargo test --workspace --test '*' -- --test-threads=4 }
}

Write-Host "All 7 standard gates passed." -ForegroundColor Green
```

### 1.2 CI 补充配置

将以下 Job 加入 `.github/workflows/ci.yml`：

```yaml
# Job 5: 安全审计
audit:
  name: Security Audit
  runs-on: ubuntu-latest
  timeout-minutes: 5
  steps:
    - uses: actions/checkout@v4
    - run: cargo install cargo-audit cargo-deny
    - name: Cargo audit
      run: cargo audit
      working-directory: rust/szrsql
    - name: Cargo deny check
      run: cargo deny check
      working-directory: rust/szrsql

# Job 6: 集成测试（与单元测试分离）
integration:
  name: Integration Tests
  runs-on: ubuntu-latest
  timeout-minutes: 15
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - uses: Swatinem/rust-cache@v2
      with:
        workspaces: rust/szrsql
    - name: Run integration tests
      working-directory: rust/szrsql
      run: cargo test --workspace --test '*' -- --test-threads=4
```

---

## 二、SzRSQL 特殊强化门禁

标准的 7 道门禁能阻挡"编译错误"和"测试失败"，但无法阻挡数据库内核开发中的几类致命问题。以下 3 道增强门禁是仅凭标准 7 关无法拦截的。

### 门禁 8：禁止占位实现检查

**来源**：sz-orm V-1~V-7 共 7 个虚假/伪实现。对于数据库内核，todo!() 或虚假实现可能导致数据损坏，比业务代码更危险。

```powershell
function Invoke-Gate-NoPlaceholders {
    Write-Host "==> [gate-8: no-placeholders] start..." -ForegroundColor Cyan
    $matches = Select-String -Path (Get-ChildItem -Recurse "*.rs" | Where-Object {
        $_.FullName -notmatch '\\target\\' -and $_.FullName -notmatch '\\fuzz\\target\\'
    }).FullName -Pattern '\b(todo!|unimplemented!|unreachable!)\b'
    # 过滤允许的例外行（// allow-placeholder）和测试代码
    $violations = $matches | Where-Object {
        $line = Get-Content -Path $_.Path -TotalCount $_.LineNumber | Select-Object -Last 1
        ($_.Path -notmatch '\\tests?\\') -and
        (-not $line.Contains('// allow-placeholder'))
    }
    if ($violations) {
        Write-Host "Found placeholder macros in non-test code:" -ForegroundColor Red
        $violations | ForEach-Object { Write-Host "  $($_.Path):$($_.LineNumber) $($_.Line.Trim())" }
        exit 1
    }
    Write-Host "==> [gate-8: no-placeholders] OK" -ForegroundColor Green
}
```

**规则**：
- `todo!()` / `unimplemented!()` / `unreachable!()` **禁止出现在任何非测试代码中**
- 唯一例外：显式注释 `// allow-placeholder` 的行可豁免（必须在 ADR 中记录原因和计划修复日期）
- 测试代码中允许 `unreachable!()`（表示测试逻辑不应该到达的分支）
- **数据库内核特殊规则**：`unreachable!()` 在存储引擎/B-Tree/WAL 路径中即使出现在测试代码也必须审查，因为数据库代码的路径复杂度高，"不可能到达"的断言可能隐含真实条件遗漏

### 门禁 9：Feature Flag 全组合编译

**来源**：sz-orm S-7：3 包 real-* feature 从未在 CI 编译/测试，数月不可编译未被发现。szrsql 当前虽无 feature flag，但 Phase 4 之后必然引入（TLS / 存储引擎后端 / CDC 等），**提前锁死规范**。

```powershell
function Invoke-Gate-AllFeatures {
    Write-Host "==> [gate-9: all-features] start..." -ForegroundColor Cyan
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    cargo check --workspace --all-targets --all-features
    if ($LASTEXITCODE -ne 0) {
        Write-Host "All-features build failed. A feature flag combination is broken." -ForegroundColor Red
        exit 1
    }
    $sw.Stop()
    Write-Host "==> [gate-9: all-features] OK ($($sw.Elapsed.TotalSeconds)s)" -ForegroundColor Green
}
```

**规则**：
- **当前（Phase 3）**：szrsql 尚无 feature flag，门禁 9 为通过状态，但必须每新增一个 feature 时立即激活此检查
- **Phase 4+**：每次提交必须对所有 feature 组合执行 `cargo check --all-features`
- 新加 feature 时必须确保所有组合都能编译
- CI 必须配置 matrix 覆盖 `--no-default-features` / `--all-features` / 关键 feature 子集
- 参考 CI matrix 配置：

```yaml
all-features:
  runs-on: ubuntu-latest
  strategy:
    matrix:
      features: [
        "--all-features",
        "--no-default-features",
      ]
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - run: cargo check --workspace --all-targets ${{ matrix.features }}
      working-directory: rust/szrsql
```

### 门禁 10：Phase Pin 检查

**来源**：szrsql 按 Phase 1/2/3/4/4.5 分阶段交付，缺乏 Phase 演进机制会导致未验证的依赖/代码被合入。

```powershell
function Invoke-Gate-PhasePin {
    Write-Host "==> [gate-10: phase-pin] start..." -ForegroundColor Cyan

    # 当前 Phase 允许的依赖白名单（按 Phase 维护）
    $phaseAllowList = @{
        "phase-3" = @(
            # Phase 1 核心依赖
            "tokio", "serde", "serde_json", "tracing", "tracing-subscriber",
            "anyhow", "thiserror", "bytes", "crc32c", "sha2", "hmac", "rand",
            "bincode", "chrono", "clap", "base64",
            # Phase 2 SQL 解析
            "sqlparser", "regex",
            # Phase 3 枚举类型
            "proptest",  # 仅 dev-dependencies
            # Phase 3 内部 crate
            "szrsql-types", "szrsql-storage", "szrsql-tx", "szrsql-cdc",
            "szrsql-sql", "szrsql-catalog", "szrsql-protocol", "szrsql-optimizer",
            "szrsql-ai", "szrsql-security", "szrsql-scheduler", "szrsql-replication",
            "szrsql-dist", "szrsql-pgcompat", "szrsql-bin"
        )
    }

    $currentPhase = "phase-3"
    $allowed = $phaseAllowList[$currentPhase]
    $violations = @()

    # 扫描所有 Cargo.toml 中的依赖
    $tomlFiles = Get-ChildItem -Recurse "Cargo.toml" | Where-Object {
        $_.FullName -notmatch '\\target\\' -and $_.FullName -notmatch '\\fuzz\\'
    }

    foreach ($toml in $tomlFiles) {
        $content = Get-Content $toml.FullName -Raw
        # 提取 [dependencies] 和 [dev-dependencies] 中的包名
        $deps = [regex]::Matches($content, '(?m)^([a-zA-Z][a-zA-Z0-9_-]*)\s*=') |
            ForEach-Object { $_.Groups[1].Value }
        foreach ($dep in $deps) {
            if ($dep -notin $allowed -and $dep -notin $violations -and $dep -ne "szrsql-fuzz") {
                $violations += $dep
                Write-Host "  [WARN] $($toml.FullName) 引用了 Phase 规划之外的依赖: $dep" -ForegroundColor Yellow
            }
        }
    }

    if ($violations.Count -gt 0) {
        Write-Host "当前 Phase ($currentPhase) 不允许的依赖: $($violations -join ', ')" -ForegroundColor Red
        Write-Host "请确认新依赖已在 ADR 中记录，并更新 phaseAllowList" -ForegroundColor Red
        # 注意：此门禁在 Phase 边界切换时由人工审核打开，日常开发仅 warning
        # exit 1  # 取消注释以启用严格模式
    }
    Write-Host "==> [gate-10: phase-pin] OK" -ForegroundColor Green
}
```

**规则**：
- szrsql 按 Phase 1→2→3→4→4.5 分阶段交付，当前 Phase 为 **Phase 3（枚举类型）**
- 每个 Phase 有明确的**依赖白名单**，不允许提前引入未来 Phase 的 crate
- `[features]` 中未来应明确声明每个 Phase 对应的 feature（当前尚无，建议 Phase 4 开始引入）
- 未到 Phase 的代码必须用 `#[cfg(feature = "phase-N")]` 或模块级 `#[cfg]` 隔离
- 特殊情况：`fuzz/Cargo.toml` 是独立 workspace，不受此限制

### 增强版 gate-full.ps1 完整骨架

```powershell
# scripts/gate-full.ps1 - 10 道门禁完整版本（7 标准 + 3 szrsql 特殊）
[CmdletBinding()]
param(
    [switch]$SkipAudit,
    [switch]$SkipIntegration
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

function Invoke-Gate([string]$name, [scriptblock]$action) {
    Write-Host "==> [$name] start..." -ForegroundColor Cyan
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $action
    if ($LASTEXITCODE -ne 0) {
        Write-Host "==> [$name] FAILED (exit $LASTEXITCODE)" -ForegroundColor Red
        exit 1
    }
    $sw.Stop()
    Write-Host "==> [$name] OK ($($sw.Elapsed.TotalSeconds)s)" -ForegroundColor Green
}

# 标准 7 关
Invoke-Gate 'fmt'               { cargo fmt --all -- --check }
Invoke-Gate 'check'             { cargo check --workspace --all-targets }
Invoke-Gate 'clippy'            { cargo clippy --workspace --all-targets -- -D warnings }
Invoke-Gate 'test'              { cargo test --workspace --locked }
Invoke-Gate 'doc'               { cargo doc --workspace --no-deps --document-private-items }
if (-not $SkipAudit) {
    Invoke-Gate 'audit'         { cargo audit; cargo deny check }
}
if (-not $SkipIntegration) {
    Invoke-Gate 'integration'   { cargo test --workspace --test '*' -- --test-threads=4 }
}

# 增强 3 关（szrsql 特殊）
Invoke-Gate 'no-placeholders'   { & "$PSScriptRoot/gate-no-placeholders.ps1" }
Invoke-Gate 'all-features'      { cargo check --workspace --all-targets --all-features }
Invoke-Gate 'phase-pin'         { & "$PSScriptRoot/gate-phase-pin.ps1" }

Write-Host "All 10 gates passed." -ForegroundColor Green
```

---

## 三、五维代码审查增强

除了 10 道门禁的自动化检查，每次合入前必须进行五维人工代码审查。每条审查意见必须标注问题编号。

| 维度 | 编号前缀 | 关注点 |
|------|---------|--------|
| 正确性 | C- | 功能正确、边界处理、竞态条件、数据完整性 |
| 可读性 | R- | 命名、结构、注释、复杂度 |
| 架构 | A- | 模块依赖、API 设计、职责划分 |
| 安全性 | S- | 注入、认证、数据保护、unsafe |
| 性能 | P- | 资源消耗、热点路径、内存管理 |

### 3.1 AI 生成代码特有审查项目

以下审查项目专门针对 **AI 生成的数据库内核 Rust 代码** 的常见缺陷模式：

#### 3.1.1 数据完整性与一致性

```rust
// ❌ AI 易犯错误：忽略 WAL 写入
fn insert_page(&mut self, page: Page) {
    self.pages.push(page);
    // WAL 未写入！crash 后数据丢失
}

// ✅ 正确：先写 WAL 再写数据页
fn insert_page(&mut self, page: Page) -> Result<(), StorageError> {
    let lsn = self.wal.write(WalRecord::Insert { /* ... */ })?;
    page.lsn = lsn;
    self.pages.push(page);
    Ok(())
}
```

- 审查要点：所有数据修改路径是否有对应的 WAL/redo log 记录
- szrsql 场景：storage/tx 模块的数据变更必须先写 WAL

#### 3.1.2 Lock Poisoned 降级

```rust
// ❌ AI 易犯错误：数据库锁 panic 会崩溃整个进程
let guard = self.lock_table.write().expect("lock table poisoned");

// ✅ 正确：使用 parking_lot（不会 poisoned）或降级处理
use parking_lot::RwLock;  // parking_lot 的锁不会 poisoned
let guard = self.lock_table.write();

// 若必须用 std::sync::Mutex，需 From<PoisonError> 降级
let guard = self.subscribers.read().map_err(|_| {
    tracing::error!("Lock poisoned, resetting");
    self.subscribers = RwLock::new(HashMap::new());
    // 重新获取
})?;
```

- 审查要点：所有 Mutex/RwLock 的 poisoned 处理
- szrsql 场景：事务锁表、BufferPool 的 page 锁、catalog 的 schema 锁
- **推荐方案**：szrsql 优先使用 `parking_lot::RwLock` / `parking_lot::Mutex`（不会 poisoned，性能更好）

#### 3.1.3 事务原子性与隔离性

```rust
// ❌ AI 易犯错误：事务中部分操作失败不回滚
fn execute_transaction(&self, ops: &[Operation]) {
    for op in ops {
        self.apply(op);  // op3 失败但 op1/op2 已提交
    }
}

// ✅ 正确：事务必须全有或全无
fn execute_transaction(&self, ops: &[Operation]) -> Result<(), TxError> {
    let tx = self.tx_mgr.begin()?;
    for op in ops {
        tx.apply(op).map_err(|e| {
            tx.rollback()?;  // 回滚已生效的操作
            e
        })?;
    }
    tx.commit()?;
    Ok(())
}
```

- 审查要点：事务提交路径是否有完整的原子性保障（commit 失败必须有 rollback）
- szrsql 场景：szrsql-tx crate 的 MVCC 事务管理

#### 3.1.4 Unsafe 审计 — 数据库内核特化

数据库内核是 Rust unsafe 密度最高的领域之一（Page 操作、指针偏移、零拷贝解析）。

```rust
// ❌ AI 易犯错误：unsafe 缺少安全说明
unsafe {
    let page_ptr = self.buffer.as_ptr().add(offset);
    std::ptr::read(page_ptr as *const PageHeader)
}

// ✅ 正确：每段 unsafe 必须有 // SAFETY: 注释
// SAFETY:
// - buffer 长度 >= offset + size_of::<PageHeader>()（调用方在 Page::read 中验证）
// - offset 是 8 字节对齐的（Page 布局保证，见 Page::layout 文档）
// - 当前线程持有 page 的读锁，无并发写
unsafe {
    let page_ptr = self.buffer.as_ptr().add(offset);
    std::ptr::read(page_ptr as *const PageHeader)
}
```

- 审查要点：每段 `unsafe` 必须有 `// SAFETY:` 注释，说明为什么安全
- szrsql 场景：szrsql-storage 的 Page 操作、BufferPool、B-Tree 节点序列化/反序列化
- AI 约束：AI 生成的任何 unsafe 代码必须同时生成 `// SAFETY:` 注释
- 审查必须：验证 SAFETY 注释的推理是否完备（光有注释但推理错误也不行）

#### 3.1.5 类型安全：`as` 转换精度丢失

```rust
// ❌ AI 易犯错误：数据库场景的数值精度丢失
let page_id = offset as u32;       // 如果文件 >4GB，offset 超 u32::MAX 静默截断
let lsn = timestamp as u64;        // 时间戳可能并非单调递增

// ✅ 正确：使用 TryFrom / 显式检查
use std::num::TryFromIntError;
let page_id = u32::try_from(offset).map_err(|_| StorageError::PageOverflow)?;
```

- 审查要点：`as u32` / `as u16` / `as usize` 等缩窄转换是否可能溢出
- szrsql 场景：Page ID（u32）、LSN（u64）、Checksum（u32）等数据库核心数值类型

#### 3.1.6 内存泄漏：循环引用 + Arc

```rust
// ❌ AI 易犯错误：BufferPool ↔ Page 循环引用
struct BufferPool {
    pages: HashMap<PageId, Arc<Mutex<Page>>>,
}
struct Page {
    pool: Arc<BufferPool>,  // pool → page → pool 循环引用
}

// ✅ 正确：使用 Weak 或索引
struct Page {
    pool_id: usize,  // 通过索引引用，而非 Arc
}
// 或
struct Page {
    pool: Weak<BufferPool>,
}
```

- 审查要点：双向 `Arc` 引用是否有 `Weak` 打断循环
- szrsql 场景：BufferPool ↔ Page、LockManager ↔ Transaction、Catalog ↔ Schema

#### 3.1.7 宏展开审查

```rust
// proc-macro 输出必须可读、无隐藏副作用
// ❌ AI 易犯错误：proc-macro 生成不易察觉的 panic 分支
// ✅ 正确：展开后的代码路径清晰可见
```

- 审查要点：`proc-macro` 的输出可以通过 `cargo expand` 查看
- 规则：`proc-macro` 输出不能有未在宏文档中声明的副作用

### 3.2 审查自动化

`scripts/review-checklist.ps1`：

```powershell
[CmdletBinding()]
param([string]$ModulePath)

Write-Host "========== SzRSQL Code Review Checklist ==========" -ForegroundColor Cyan

$rsFiles = Get-ChildItem -Recurse -Include "*.rs" -Path $ModulePath |
    Where-Object { $_.FullName -notmatch '\\target\\' }

Write-Host ""

# 1. 正确性
Write-Host "--- [C-] Correctness ---" -ForegroundColor Yellow
Write-Host "  [ ] 所有 unwrap()/expect() 已处理？"
Write-Host "  [ ] Lock poisoned 有降级处理？"
Write-Host "  [ ] 事务原子性：失败路径有 rollback？"
Write-Host "  [ ] 边界条件（空页、最大 Page ID、空事务）有测试？"
Write-Host "  [ ] WAL 写入顺序正确（先写 WAL 再写数据）？"

# 2. 可读性
Write-Host "--- [R-] Readability ---" -ForegroundColor Yellow
Write-Host "  [ ] 命名与 szrsql 代码风格一致？"
Write-Host "  [ ] 函数是否过长（>50 行建议拆分）？"
Write-Host "  [ ] 是否有残留的 dbg / println / eprintln？"

# 3. 架构
Write-Host "--- [A-] Architecture ---" -ForegroundColor Yellow
Write-Host "  [ ] 模块依赖方向正确？"
Write-Host "  [ ] 新增依赖已在 ADR 中记录？"
Write-Host "  [ ] 是否引入了当前 Phase 规划之外的依赖？（门禁 10）"

# 4. 安全性
Write-Host "--- [S-] Security ---" -ForegroundColor Yellow
Write-Host "  [ ] SQL 解析/执行路径全参数化（无字符串拼接）？"
Write-Host "  [ ] Unsafe 有 // SAFETY: 注释？"
Write-Host "  [ ] 认证/鉴权路径有 SQL 注入防御？"
Write-Host "  [ ] 敏感信息（密码/密钥）未在日志中输出？"

# 5. 性能
Write-Host "--- [P-] Performance ---" -ForegroundColor Yellow
Write-Host "  [ ] 热点路径避免不必要的 clone()？"
Write-Host "  [ ] 有 Arc 循环引用风险？"
Write-Host "  [ ] Page/Buffer 有内存池化而非频繁分配？"
Write-Host "  [ ] B-Tree 节点分裂/合并路径正确对齐？"

# 6. AI 特有检查
Write-Host "--- [AI-Specific] AI 生成代码特有检查 ---" -ForegroundColor Magenta
Write-Host "  [ ] todo!()/unimplemented!()/unreachable!() 已清除？（门禁 8）"
Write-Host "  [ ] 无 SQL 字符串拼接？"
Write-Host "  [ ] 无 as 缩窄转换导致精度丢失？"
Write-Host "  [ ] proc-macro 展开无隐藏副作用？"
Write-Host "  [ ] unsafe 块都有 // SAFETY: 注释？"
Write-Host "  [ ] 所有 trait 方法都有合理默认值（非占位实现）？"
Write-Host "  [ ] WAL/事务路径完整无遗漏？"

# 7. 文件统计
Write-Host "--- [Stats] ---" -ForegroundColor Cyan
Write-Host "  Files: $($rsFiles.Count)"
Write-Host "  Lines: $($rsFiles | ForEach-Object { (Get-Content $_.FullName | Measure-Object -Line).Lines } | Measure-Object -Sum | Select-Object -ExpandProperty Sum)"

Write-Host "=================================================="
```

### 3.3 审查标签与版本标记

- 审查通过的模块必须 `git tag` 标记版本：`szrsql-v0.1.0-<crate-name>`
- 已标记版本不允许重新修改（除非有新的 MAJOR 变更）
- 每个 tag 关联对应的审查报告（ADR 或 PR 描述）

---

## 四、测试体系

### 4.1 测试金字塔

```
                         ▲
                         │   ┌─────────────────────────────┐
                         │   │ T4 压力测试                   │  高并发查询/长稳/内存泄漏
                         │   │   （待开发完成后补充）         │
                         │   └─────────────────────────────┘
                       ┌─┴─────────────────────────────────┐
                       │  T3 Fuzz + Property-Based         │  随机 SQL 语句/协议消息
                       │  （szrsql-sql 已有 15 fuzz tests）│
                       └─┬─────────────────────────────────┘
                     ┌───┴───────────────────────────────────┐
                     │  T2 集成测试                           │  跨模块交互/PG 协议
                     │  （szrsql-protocol + szrsql-catalog）  │
                     └───┴───────────────────────────────────┘
                   ┌─────┴─────────────────────────────────────┐
                   │  T1 单元测试                              │  每个函数级 cargo test
                   │  （14 crates，已有大量 unit tests）       │
                   └───────────────────────────────────────────┘
```

| 层级 | 名称 | 工具 | 当前状态 | 覆盖目标 |
|------|------|------|---------|---------|
| T1 | 单元测试 | `cargo test --workspace` | ✅ 已有 | 每个函数逻辑 |
| T2 | 集成测试 | `cargo test --test '*'` | ✅ 部分已有 | 跨模块交互 / PG 协议消息 |
| T3 | Fuzz + Property-Based | `proptest` + `libfuzzer-sys` | ✅ 已有（15 个 fuzz tests） | 随机 SQL 语句 / B-Tree 编解码 |
| T4 | 压力测试 | `tokio::spawn` + 长稳监控 | ⬜ 待补充 | 高并发查询 / 内存泄漏 |

### 4.2 T2 集成测试

szrsql 当前的集成测试分布在以下位置：

| 位置 | 测试内容 | 执行方式 |
|------|---------|---------|
| `crates/szrsql-protocol/tests/` | PG wire 协议、认证、通知、TLS | `cargo test -p szrsql-protocol --test '*'` |
| `crates/szrsql-sql/tests/integration/` | SQL 完整执行链路 | `cargo test -p szrsql-sql --test '*'` |
| `crates/szrsql-catalog/tests/integration/` | RBAC/RLS 权限、多租户 | `cargo test -p szrsql-catalog --test '*'` |

**规范**：
- 所有集成测试放在 `tests/` 目录（而非 `tests/integration/` 子目录），每个 crate 一个 `tests/` 入口
- 集成测试**不允许依赖未在当前 crate `[dev-dependencies]` 中声明的外部 crate**
- 跨多个 crate 的集成测试放在 `tests/` 目录下（workspace 级），当前尚未有此设置
- 需要外部资源（如文件系统、端口）的测试用 `#[ignore]` 标记

### 4.3 T3 Fuzz + Property-Based Testing

szrsql 的 fuzz 测试分布在两个位置：

| 位置 | Fuzz 目标 | 说明 |
|------|----------|------|
| `fuzz/fuzz_targets/btree_fuzz.rs` | B-Tree 操作随机序列 | 基于 `libfuzzer-sys` |
| `fuzz/fuzz_targets/btree_encode_decode_fuzz.rs` | B-Tree 编解码一致性 | 编码→解码→验证 |
| `crates/szrsql-sql/tests/fuzz/sql_fuzz.rs` | SQL 随机语句解析 | 基于 `proptest` |
| `crates/szrsql-catalog/tests/fuzz/auth_fuzz.rs` | 认证/鉴权随机输入 | 基于 `proptest` |

**覆盖要求**：
- SQL 解析器必须对 sqlparser 支持的全部 SQL 语法做 fuzz
- B-Tree 操作的任意序列不能 panic（插入→查询→分裂→删除→合并）
- 编码/解码往返一致性（encode→decode 必须等于原值）
- 认证/鉴权输入随机化（用户名/密码/权限组合）

**规范**：
```rust
// crates/szrsql-sql/tests/fuzz/sql_fuzz.rs — 示例规范
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_sql_parse_does_not_panic(
        sql in "[a-zA-Z0-9_\\s\\\\(\\),;'\"=<>!+\\-*/%.]{0,200}",
    ) {
        // 任意 SQL 字符串不能导致 parser panic
        let _ = szrsql_sql::parse(&sql);
    }

    #[test]
    fn prop_btree_encode_decode(
        keys in proptest::collection::vec(any::<u64>(), 0..100),
        values in proptest::collection::vec(any::<[u8; 32]>(), 0..100),
    ) {
        let tree = BTree::from_entries(keys.iter().copied().zip(values.iter()));
        let encoded = tree.encode();
        let decoded = BTree::decode(&encoded).unwrap();
        prop_assert_eq!(tree, decoded);
    }
}
```

### 4.4 T4 压力测试（待补充）

当前 szrsql 处于 Phase 3，尚未覆盖压力测试。**预计 Phase 4 完成后补充**。

压力测试要求：
- 并发连接数 ≥ 100
- 混合读写负载（80% 读 + 20% 写）
- 运行时长时间监控（至少 1 小时无内存增长）
- 监控指标：RSS 内存、文件句柄数、QPS、P50/P95/P99 延迟

### 4.5 测试编写规范

1. **测试名遵循 `<场景>_<预期>`**：`btree_split_leaf_rebalance`
2. **每个测试 Arrange-Act-Assert 三段式**
3. **不依赖外部资源的测试默认运行**，依赖外部资源的用 `#[ignore]` 标记
4. **测试代码也要通过 clippy（`--all-targets`）和格式化（`cargo fmt --all`）**
5. **Property-Based Testing 的拒绝率必须 <10%**（用 strategy 范围限制而非 `prop_assume!`）
6. **Fuzz 测试必须至少跑 10,000 次随机输入无 panic** 才能合入
7. **数据库测试的特殊要求**：每次测试前后必须清理环境（临时文件、端口释放），避免测试间的状态污染

---

## 五、CI/CD 与开发流程

### 5.1 本地开发流程

```
修改代码
  │
  ▼
cargo check                 # 快速验证编译（比 build 快 2-3x）
  │
  ▼
cargo test -p <affected>    # 只运行受影响的 crate 测试
  │
  ▼
cargo clippy -p <affected>  # 只运行受影响 crate 的 lint
  │
  ▼
.\scripts\gate-full.ps1     # 完整 10 道门禁（push 前必须）
  │
  ▼
git push
```

**本地开发原则**：
- 每次修改后先 `cargo check`（比 `cargo build` 快 2-3 倍）
- 只运行受影响 crate 的测试，而非全量（`cargo test -p szrsql-storage`）
- push 前必须运行 `gate-full.ps1`（可在 `pre-push` hook 中自动触发）
- 数据库开发特别注意：修改存储格式（Page 布局、B-Tree 节点结构）后必须全量运行 T2+T3 测试

### 5.2 CI 配置（推荐最终版）

```yaml
# .github/workflows/ci.yml — szrsql 完整 CI
name: SzRSQL CI
on:
  push:
    branches: [main, master]
  pull_request:

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  # 门禁 Job
  gate:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: rust/szrsql
      - name: Run 10 Gates
        run: pwsh ./scripts/gate-full.ps1
        working-directory: rust/szrsql

  # Feature 全组合编译（Phase 4+ 启用）
  all-features:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: rust/szrsql
      - run: cargo check --workspace --all-targets --all-features
        working-directory: rust/szrsql

  # Fuzz 测试（CI 上做短迭代验证）
  fuzz:
    runs-on: ubuntu-latest
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: rust/szrsql
      - name: Run property-based tests
        run: cargo test --workspace --test fuzz -- --nocapture
        working-directory: rust/szrsql

  # 安全审计
  security:
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@v4
      - run: cargo install cargo-audit cargo-deny
      - run: cargo audit
        working-directory: rust/szrsql
      - run: cargo deny check
        working-directory: rust/szrsql
```

### 5.3 部署前检查清单

> szrsql 是自研数据库，部署前检查比 Web 应用更严格

- [ ] **门禁检查**
  - [ ] 10 道门禁全部通过（含增强门禁 8-10）
  - [ ] 所有 feature 组合编译通过

- [ ] **测试检查**
  - [ ] `cargo test --workspace` 100% 通过
  - [ ] Fuzz 测试至少 10,000 次随机输入无 panic
  - [ ] B-Tree 编解码往返一致性验证全部通过

- [ ] **审查检查**
  - [ ] 五维审查全部通过（正确性/可读性/架构/安全/性能）
  - [ ] 无残留的占位宏（todo!/unimplemented!/unreachable!）
  - [ ] 所有 unsafe 有 `// SAFETY:` 注释
  - [ ] WAL/事务路径完整性审查
  - [ ] 无 SQL 拼接（SQL 解析/执行路径全参数化）

- [ ] **Phase 合规检查**
  - [ ] 未引入 Phase 规划之外的外部依赖
  - [ ] 当前 Phase 功能全部实现并通过测试
  - [ ] 下一 Phase 功能的代码已用 feature flag 隔离（如有）

### 5.4 AI 辅助开发约束

针对 szrsql 的"AI 主导数据库内核开发"场景，以下约束必须遵守：

| 约束编号 | 约束内容 | 出处 |
|---------|---------|------|
| AI-1 | AI 生成的任何 unsafe 代码必须有 `// SAFETY:` 注释说明为什么安全 | szrsql 存储引擎 |
| AI-2 | AI 不允许生成 `todo!()` / `unimplemented!()` / `unreachable!()` | sz-orm V-1~V-7 |
| AI-3 | AI 生成的 SQL 语句必须通过参数化接口（不允许 format! 拼接）— 数据库本身更危险 | sz-orm C-1~C-6 |
| AI-4 | AI 生成的 proc-macro 必须展开后可读（`cargo expand` 验证） | 通用 |
| AI-5 | 每次 AI 提交必须有对应的测试代码（单元测试 + fuzz 测试） | szrsql 工程化规范 |
| AI-6 | AI 生成的新 trait 实现必须全部方法都有合理默认值（非占位实现） | sz-orm V-1~V-7 |
| AI-7 | AI 生成的 `as` 类型转换必须确认无精度丢失（Page ID / LSN 等数据库核心数值） | 数据库安全 |
| AI-8 | AI 生成的双向 `Arc` 引用必须用 `Weak` 打断循环（BufferPool/Page 等） | 内存泄漏防御 |
| AI-9 | AI 生成的事务代码必须有完整的 commit/rollback 路径 | 事务原子性 |
| AI-10 | AI 生成的数据修改路径必须先写 WAL 再写数据页 | WAL 预写日志原则 |
| AI-11 | AI 不允许使用 `std::sync::Mutex` / `std::sync::RwLock` 替代 `parking_lot` | 锁中毒防御 |
| AI-12 | AI 生成的 Page/Buffer 操作代码必须包含对齐和越界检查 | 内存安全 |

---

## 六、附录：sz-orm 教训 → szrsql 防御

### 6.1 教训映射表

| 教训 | sz-orm 问题 | szrsql 风险 | 防御措施 |
|------|-----------|------------|---------|
| 占位实现 | 7 个虚假实现（RealPg st_union 不执行 SQL、SloMonitor 仅 2 窗口） | 数据库功能假实现可能导致数据损坏或静默写入丢失 | **门禁 8**：占位宏检查 + **审查 AI-6**：trait 实现完整性 |
| Feature 隔离失败 | RealPg/RealTimescale 数月未编译，CI 从未检查 | 新存储引擎 backend 或 TLS feature 可能无人测试，合入后不可编译 | **门禁 9**：Feature Flag 全组合编译 + CI matrix |
| 依赖泛滥 | 引入未在 ADR 中 Record 的依赖 | 提前引入 Phase 5+ 依赖（如分布式共识库），增加编译时间和攻击面 | **门禁 10**：Phase Pin 依赖白名单 |
| Lock Poisoned | 13 处 `expect("lock poisoned")`，panic 直接崩溃 | 数据库事务锁 panic 会崩溃整个进程，导致数据丢失 | **强制 parking_lot** + **审查 C-**：From<PoisonError> 降级 |
| SQL 拼接 | 6 Critical 注入（PostGIS EWKT 拼接、FOREIGN KEY 拼接） | 数据库本身执行 SQL，拼接更危险——攻击者可通过 SQL 注入执行任意查询/修改 | **全参数化查询** + **审查 S-**：SQL 路径审计 |
| 测试数据点 < 10 | 测试仅覆盖 happy path，边界条件无覆盖 | B-Tree 边界情况（根分裂、叶子合并、空树）未经充分测试 | **T3 Fuzz + Property**：至少 10,000 次随机输入 |
| Unsafe 无 SAFETY | AI 生成 unsafe 无安全说明 | Page 指针操作/Buffer 偏移错误导致段错误或数据损坏 | **审查 AI-1**：unsafe 必须有 SAFETY 注释 |
| 名实不符 | "倒排索引"实为线性扫描 | 存储引擎/B-Tree 的实现与文档不一致 | **ADR 记录**：每个实现必须明确声明实际算法 |
| 密码/密钥硬编码 | 配置文件中硬编码密钥 | 数据库认证密码硬编码风险更高 | **审查 S-**：敏感信息配置化 |
| 夸大对比 | 对比数据缺少可复现来源 | 性能基准数据不透明 | **criterion 基准**：每次合入前运行 bench |
| WAL 写入顺序 | (sz-orm 未涉及，但 szrsql 特有) | 先写数据后写 WAL，crash 后数据页已修改但 WAL 无记录 → 数据损坏 | **审查 AI-10**：WAL 先写原则 |

### 6.2 szrsql 独有的防御重点

相比通用 Rust 项目（如 sz-orm / sz-rust），szrsql 作为 **数据库内核** 有以下额外的防御重点：

| 风险领域 | 具体风险 | 防御层级 |
|---------|---------|---------|
| **数据完整性** | WAL 写入顺序错误、事务部分提交、Page 写入未落盘 | T2 集成测试 + 审查 AI-9/10 |
| **并发安全** | B-Tree 并发遍历/分裂/合并、MVCC 可见性判断 | T3 Fuzz + 审查 C- |
| **内存安全** | Page 指针偏移越界、零拷贝解析越界、BufferPool Use-After-Free | 审查 S- + AI-12 |
| **格式兼容** | Page 布局变更导致旧数据不可读、B-Tree 节点格式不兼容 | T3 编解码 Fuzz + 版本标记 |
| **性能退化** | B-Tree 分裂策略不当导致树失衡、BufferPool 淘汰策略劣化 | cargo bench + T4 压力测试 |
| **认证安全** | PG 协议认证实现漏洞、密码传输未加密 | T2 集成测试 + 审查 S- |

### 6.3 必须执行的追溯

每个 Phase 交割时，必须逐项检查本附录的教训映射表，确认每一条教训在当前 Phase 代码中都有对应的防御措施且已落实。

| Phase | 交割检查重点 |
|-------|------------|
| Phase 1（基础存储） | B-Tree 占位实现检查、Page 操作 unsafe 审计、WAL 写入顺序验证 |
| Phase 2（SQL 解析） | SQL 拼接扫描、Parser fuzz 覆盖率、类型转换精度检查 |
| Phase 3（枚举类型） | 类型系统扩展的 feature gate、新类型的内存布局对齐检查 |
| Phase 4（PG 协议） | 认证路径安全性审计、协议解析 fuzz、TLS 集成测试 |
| Phase 4.5（生产就绪） | 压力测试基线、内存泄漏 24h soak、全量审计门禁 |

---

## 七、与其他规范的关系

- 本规范定义 **szrsql 工程化的全局规范**，是 szrsql 项目所有 crate 必须遵守的约定；
- [`szrsql-architecture.md`](szrsql-architecture.md)（若有）定义架构决策与设计说明；
- `docs/adr/` 目录记录每个重大架构决策；
- 通用 [`rust-engineering-practices/`](../rust-engineering-practices/) 规范（7 道门禁、契约审计、测试金字塔等）是本规范的基础，szrsql 在此基础上增加了数据库内核开发场景的特殊强化。
