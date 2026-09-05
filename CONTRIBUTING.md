# 贡献指南

感谢您对 SZ-Rust 项目的关注！本文档指导您如何参与贡献。

## 开发环境

- Rust stable（≥ 1.75，需要 `async fn in trait` 支持）
- Node.js 20+（仅前端业务迁移需要）
- MySQL 8.0+ 或 PostgreSQL 14+（仅集成测试需要）

## 快速开始

```bash
# 克隆仓库
git clone https://github.com/ljclz/sz-rust.git
cd sz-rust

# 编译检查
cargo check --workspace --all-targets

# 运行测试
cargo test --workspace

# 运行 clippy（必须 0 警告）
cargo clippy --workspace --all-targets -- -D warnings

# 格式检查（必须 0 差异）
cargo fmt --all -- --check
```

## 10 道工程化门禁

所有 PR 必须通过以下 10 道门禁（详见 [《SZ-Rust 工程化实践规范》](docs/sz-rust-engineering-practices.md)）：

| 门禁 | 命令 | 要求 |
|------|------|------|
| 1. fmt | `cargo fmt --all -- --check` | 0 差异 |
| 2. check | `cargo check --workspace --all-targets` | 0 错误 |
| 3. clippy | `cargo clippy --workspace --all-targets -- -D warnings` | 0 警告 |
| 4. test | `cargo test --workspace` | 全部通过 |
| 5. doc | `cargo doc --workspace --no-deps --all-features` | 构建成功 |
| 6. audit | `cargo audit` | 无已知漏洞 |
| 7. integration | `cargo test --workspace -- --ignored` | 集成测试通过 |
| 8. 占位检查 | 扫描 `todo!()/unimplemented!()/panic!("not implemented")` | 0 处占位 |
| 9. 安全扫描 | 扫描 SQL 注入/XSS/CSRF/路径遍历 | 0 处漏洞 |
| 10. feature 全组合 | `cargo check --workspace --all-targets --all-features` | 编译通过 |

## 提交规范

### 提交信息格式

遵循 [Conventional Commits](https://www.conventionalcommits.org/)：

```
<type>(<scope>): <subject>

<body>

<footer>
```

**type** 取值：
- `feat`：新功能
- `fix`：Bug 修复
- `docs`：文档变更
- `style`：代码格式（不影响功能）
- `refactor`：重构（ neither新功能 nor Bug修复）
- `perf`：性能优化
- `test`：测试相关
- `chore`：构建/工具/依赖变更

**scope** 取值（可选）：
- `core`：sz-rust-core
- `addons`：sz-rust-addons-*
- `macros`：sz-rust-macros
- `cli`：sz-rust-cli
- `examples`：sz-rust-examples
- `docs`：文档
- `ci`：CI 配置

**示例**：
```
feat(core): 新增 Guard 守卫模块，支持 AuthGuard/PermissionGuard/GuardChain

- 借鉴 NestJS Guard + Spring Security 设计
- 鉴权决策与 Middleware 分离（二元决策 vs 横切关注点）
- 支持 AND 语义的 Guard 链
```

### 分支策略

- `main`：稳定分支，只接受 PR 合入
- `feat/*`：功能分支
- `fix/*`：修复分支
- `docs/*`：文档分支

## 架构决策记录（ADR）

所有影响公共 API 表面或性能特性的决策必须新增 ADR：

1. 复制 `docs/adr/ADR-NNN-<short-title>.md` 模板
2. 填写背景/决策/后果/注意事项/Bug 定位提示
3. 更新 `docs/adr/README.md` 索引
4. 在 PR 中引用该 ADR

详见 [《ADR 与生产 Bug 定位规范》](docs/ADR与生产Bug定位规范.md)。

## 测试规范

### 测试金字塔

| 层级 | 占比 | 工具 | 说明 |
|------|------|------|------|
| T1 单元测试 | 70% | `#[test]` | 纯函数 / 无副作用逻辑 |
| T2 集成测试 | 20% | `#[test]` + `tokio::test` | 模块间交互 |
| T3 端到端测试 | 5% | axum::Router::oneshot | HTTP 请求→响应 |
| T4 PHP 行为对比测试 | 3% | 自定义对比 | PHP/Rust 行为一致性 |
| T5 性能基准测试 | 1% | criterion | 性能回归 |
| T6 soak 测试 | 1% | 自定义 | 长时间运行退化检测 |

### 测试红线规则

- **禁止 mock 自己编写的代码**：只 mock 外部依赖（DB/HTTP/文件系统）
- **禁止删除已通过的测试**：除非该测试对应的代码已删除
- **禁止 `#[ignore]` 生产代码测试**：`#[ignore]` 仅用于 soak/长时间测试
- **每个 Bug 修复必须新增回归测试**

## PHP 迁移规范

SZ-Rust 对标 ThinkPHP 8，PHP 迁移需遵循：

1. **1:1 复刻 PHP 行为**：包括 PHP 源码 bug（必须有注释说明）
2. **控制器方法无参数**：主键从 `$data` 获取（如 `$data['good_id']`）
3. **不使用 GET 请求分支**：禁止 `if ($this->request->isGet())`
4. **标准响应格式**：`{code, msg, data}`，字段顺序严格一致
5. **错误码对齐**：1/0/-1/-2/-3 + 403/404/422/500

## 代码审查清单（五维）

每个 PR 必须通过五维审查：

| 维度 | 检查项 |
|------|--------|
| 正确性 | 逻辑正确？边界条件处理？PHP 行为对齐？ |
| 可读性 | 命名清晰？注释充分？结构合理？ |
| 架构 | 分层正确？依赖方向？ADR 遵守？ |
| 安全性 | 输入校验？SQL 注入？路径遍历？权限检查？ |
| 性能 | 避免重复计算？合理数据结构？异步无阻塞？ |

## 许可证

贡献的代码遵循 [MIT License](LICENSE)。
