# sz-rust 技能触发命令速查

> **目录**：`.trae/skills/`
> **项目根目录**：`e:\vue\test\鲜视达\rust\sz-rust`

---

## 一、全面排查（一次性触发所有技能）

### 对话框输入

```
运行全面排查，执行所有5个技能
```

### AI 执行顺序

| 步骤 | 技能 | 路径 | 产出 |
|------|------|------|------|
| 1 | 路由变异测试 | `.trae/skills/sz-rust-framework-routing/SKILL.md` | 变异存活报告 |
| 2 | 中间件混沌测试 | `.trae/skills/sz-rust-framework-middleware/SKILL.md` | 混沌测试日志 |
| 3 | DI 循环依赖检测 | `.trae/skills/sz-rust-framework-di/SKILL.md` | 依赖分析报告 |
| 4 | 配置审计 | `.trae/skills/sz-rust-framework-config/SKILL.md` | 审计报告 |
| 5 | 负载与内存基线 | `.trae/skills/sz-rust-framework-load/SKILL.md` | 基准测试结果 |

---

## 二、单项技能触发

### 1. 路由变异测试

| 操作 | 对话框输入 |
|------|-----------|
| 执行路由变异测试 | `执行 sz-rust-framework-routing 路由变异测试` |
| 指定文件变异 | `对 packages/sz-rust-core/src/router.rs 执行路由变异` |

**命令行**（也可直接跑）：

```bash
cargo mutants -p sz-rust-core --mutate "src/router/**/*.rs" -- --test-threads=1
```

**攻击目标**：将 `/user/:id` 变异为 `/user/:id/`（尾部斜杠），删除 `(\d+)` 约束，交换 `/user/me` 和 `/user/:id` 顺序。

**通过标准**：路由歧义时返回明确错误（非随机匹配）。

### 2. 中间件混沌测试

| 操作 | 对话框输入 |
|------|-----------|
| 执行中间件混沌测试 | `执行 sz-rust-framework-middleware 中间件混沌攻击` |
| 中间件 Panic 测试 | `测试中间件 panic 时 CatchPanicLayer 是否返回 500` |
| 超时测试 | `测试中间件 sleep(10s) 时全局超时层是否返回 408` |
| 上下文篡改测试 | `测试篡改 extensions.UserId 是否类型安全` |

**命令行**：

```bash
cargo test -p sz-rust-core --test middleware_chaos -- --nocapture
```

**攻击目标**：
1. 中间件故意 panic，验证 CatchPanicLayer 返回 500 而非崩溃
2. sleep(10s)，验证全局超时层返回 408
3. 篡改 `extensions` 中的 `UserId`，确保类型安全

**通过标准**：进程永不崩溃，连接数无泄漏。

### 3. DI 循环依赖检测

| 操作 | 对话框输入 |
|------|-----------|
| 执行 DI 检测 | `执行 sz-rust-framework-di 循环依赖检测` |
| 深度依赖测试 | `测试 10 层深度依赖，验证递归上限返回 Err` |

**命令行**：

```bash
cargo test -p sz-rust-core --test di_deadlock -- --nocapture
```

**攻击目标**：
- A 依赖 B，B 依赖 A，验证返回 `CyclicDependency`
- 10 层深度依赖，验证递归上限（16）返回 `Err`

**通过标准**：无 Panic，无死锁。

### 4. 配置审计

| 操作 | 对话框输入 |
|------|-----------|
| 执行配置审计 | `执行 sz-rust-framework-config 配置脱敏审计` |
| 敏感字段检查 | `扫描所有 struct，检查 password/secret 是否已脱敏` |
| 目录遍历检查 | `检查静态文件服务是否拦截 .. 路径` |

**审查范围**：

| 审查项 | 强制要求 |
|--------|---------|
| 敏感字段 | 含 `password`/`secret`/`token`/`api_key` 的 struct 必须加 `#[serde(skip_serializing)]` |
| 路径遍历 | 静态文件服务必须拦截 `..`，返回 403 |

### 5. 负载与内存基线

| 操作 | 对话框输入 |
|------|-----------|
| 执行负载测试 | `执行 sz-rust-framework-load 负载与内存基线测试` |
| 10 万次并发 | `用 criterion 压测 10 万次请求，验证内存增长` |

**命令行**：

```bash
cargo bench -p sz-rust-core
```

**通过标准**：

| 指标 | 上限 |
|------|------|
| 空载 RSS | ≤ 20MB |
| 简单路由 P99 | ≤ 5ms |
| 10 万次请求内存增长 | < 1MB |

---

## 三、日常快速验证

### 3.1 运行全部单元测试

**对话框输入**：

```
运行全部测试，确认无回归
```

**命令行**：

```bash
cargo test --workspace --lib --jobs 2
```

### 3.2 运行特定 package 测试

```bash
cargo test -p sz-rust-core          # 框架核心
cargo test -p sz-rust-sz300         # sz300 业务
cargo test -p sz-rust-addons-operate # 插件
cargo test -p sz-rust-cli           # 命令行
```

### 3.3 编译检查

```bash
cargo check --workspace --all-targets
```

### 3.4 Clippy 静态检查

```bash
cargo clippy --workspace -- -D warnings
```

---

## 四、环境准备

### 4.1 初次安装（仅一次）

```bash
# 变异测试 CLI 工具（Skill 1 依赖）
cargo install cargo-mutants
```

### 4.2 前置依赖（Cargo.toml 添加）

在 `packages/sz-rust-core/Cargo.toml` 的 `[dev-dependencies]` 中添加：

```toml
proptest = "1.4"
fake = { version = "2.9", features = ["chrono", "uuid"] }
rand = "0.8"
rstest = "0.19"
tokio-test = "0.4"
criterion = { version = "0.5", features = ["html_reports"] }
mockall = "0.12"
loom = "0.7"
regex = "1.10"
```

### 4.3 铁律自检清单（AI 每次修改前检查）

- [ ] 是否读取了 `.trae/rules/project_rules.md`？（12 条生死线）
- [ ] 本次修改涉及的技能是否已加载？
- [ ] 修改 router 时是否执行了路由变异测试（Skill 1）？
- [ ] 新增中间件时是否执行了混沌测试（Skill 2）？
- [ ] 修改 container 时是否执行了 DI 检测（Skill 3）？
- [ ] 修改 config/static 时是否执行了配置审计（Skill 4）？
- [ ] 是否运行了 `cargo check --workspace`？
- [ ] 是否检查了代码中是否残留 `unwrap`/`expect`？
- [ ] 是否检查了异步函数中是否存在 `std::thread::sleep`？

---

## 附录：文件索引

| 文件 | 路径 |
|------|------|
| 项目铁律（12 条） | `.trae/rules/project_rules.md` |
| Trae 设置 | `.trae/settings.json` |
| Agent 指南 | `AGENTS.md` |
| Skill 1：路由变异测试 | `.trae/skills/sz-rust-framework-routing/SKILL.md` |
| Skill 2：中间件混沌测试 | `.trae/skills/sz-rust-framework-middleware/SKILL.md` |
| Skill 3：DI 循环依赖检测 | `.trae/skills/sz-rust-framework-di/SKILL.md` |
| Skill 4：配置审计 | `.trae/skills/sz-rust-framework-config/SKILL.md` |
| Skill 5：负载与内存基线 | `.trae/skills/sz-rust-framework-load/SKILL.md` |
