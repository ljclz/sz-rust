# sz-rust-cli

SZ-Rust 命令行工具 — 项目脚手架生成、插件模板、代码审查。

## 功能

### make:plugin — 插件模板生成

支持 4 种模板类型：

- **plugin-crud** — CRUD 插件（Model + Controller + Service + Repository + Migration + Routes + Manifest + Tests）
- **plugin-master-slave** — 主从插件（Master/Slave Model + Cascade + DataSource）
- **plugin-report** — 报表插件
- **plugin-workflow** — 工作流插件

```bash
# 生成 CRUD 插件
sz-rust-cli make:plugin --name my-plugin --type crud --fields "name:string,price:decimal"

# 生成主从插件
sz-rust-cli make:plugin --name order-plugin --type master-slave --master Order --slave OrderItem
```

### 其他命令

- `make:controller` — 生成控制器
- `make:model` — 生成模型
- `make:middleware` — 生成中间件
- `check` — 代码审查与铁律合规检查

## 模板引擎

使用 Tera 模板引擎，模板文件位于 `templates/` 目录。每个模板类型包含：

- `template.toml` — 模板元数据
- `*.tera` — Tera 模板文件

## 自定义模板

在 `templates/` 下创建新目录，包含 `template.toml` 和 `.tera` 文件即可。模板支持变量插值、条件、循环等 Tera 语法。

## 测试

```bash
cargo test -p sz-rust-cli
```

354 单元测试 + 10 集成测试，全部通过。