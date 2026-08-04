# sz-rust-infra-facade

SZ-Rust 基础设施层。包含 config、validate、static_files、upload、debug_page 五大模块。

## 功能

| 模块 | 对齐 PHP | 说明 |
|------|---------|------|
| `config` | `config/app.php` | YAML 加载 + 环境变量覆盖 + 默认值 |
| `validate` | `think\Validate` | 数据验证器（规则引擎 + 场景 + 消息） |
| `static_files` | 静态文件路由 | `tower-http::ServeDir` 封装 |
| `upload` | `think\File` | 文件上传 + 图片处理 + 云存储驱动 |
| `debug_page` | Whoops | 开发环境调试页 + 生产环境简洁页 |

## 用法

```rust
use sz_rust_infra_facade::config::Config;
use sz_rust_infra_facade::validate::{Validator, rule};

// 配置加载
let config = Config::load("config/app.yml")?;

// 数据验证
let v = Validator::new(data);
v.rule("email", "required|email");
```

## 依赖

- `axum`
- `serde_yml`
- `image` / `ab_glyph`（图片处理）
- `sz-rust-orm-facade`（云存储驱动，统一通过 orm-facade 访问）
- `tower-http`
- `tokio` / `tokio-util`

## 版本策略

与 `sz-rust-core` 保持同步。
