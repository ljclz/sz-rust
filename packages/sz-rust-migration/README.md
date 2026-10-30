# sz-rust-migration

SZ-Rust AI 辅助迁移工具 — TP6（ThinkPHP 6）→ sz-rust 端到端迁移流水线。

## 功能

- **TP6 代码分析**：解析路由、Model、Controller、Middleware，构建依赖图，评估迁移复杂度
- **sz-rust 代码生成**：基于分析结果，通过 LLM 生成等价的 sz-rust Rust 代码
- **增量验证**：对比 TP6 与 sz-rust 服务在相同 HTTP 请求下的响应一致性
- **迁移报告**：汇总分析结果、生成结果、验证结果、风险项和迁移建议
- **迁移案例文档**：基于典型接口生成迁移前后代码对比文档

## 架构

```
TP6 项目 → 分析(Analysis) → 生成(Generation) → 验证(Validation) → 报告(Reporting) → 案例(CaseDoc)
```

五阶段编排引擎 `MigrationOrchestrator` 顺序执行各阶段，每阶段产出可机器解析的文档。

## 使用

```rust
use sz_rust_migration::{MigrationOrchestrator, MigrationStartRequest};

let orchestrator = MigrationOrchestrator::new();
let request = MigrationStartRequest {
    tp6_project_path: "/path/to/tp6/project".into(),
    config_path: "/path/to/migration.toml".into(),
    output_dir: "/path/to/output".into(),
    validation_samples: vec![],
    tenant_id: "my-tenant".into(),
};
let handle = orchestrator.start(request).await?;
```

## 配置

参考 `migration.toml.example` 创建配置文件。

## 复用

- `sz-rust-ai-facade`：LLM 统一抽象
- `sz-rust-sdd`：SDD Agent 编排引擎
- `sz-rust-capability`：能力守卫

## 测试

```bash
cargo test -p sz-rust-migration
```

53 单元测试 + 2 集成测试 + 1 E2E 测试，全部通过。