# sz-rust-capability

SZ-Rust Capability Registry — 统一能力注册表，将 Skills（AI 内置能力）与 Plugins（业务插件）抽象为统一的 Capability 接口。

## 核心组件

- `Capability` trait — 统一能力抽象
- `CapabilityRegistry` — 中心注册表（注册/发现/调用）
- `Cap` facade — 静态 API（OnceLock 全局实例）
- `CapabilitySource` — 能力来源枚举（Skill/Plugin/Service）
- `CapError` — 错误类型

## 使用示例

```rust
use sz_rust_capability::{Cap, Capability, CapabilitySource};

// 注册能力
Cap::init();
Cap::register(Arc::new(MyCapability));

// 发现能力
let caps = Cap::find_by_tags(&["crm", "read"], None)?;

// 调用能力
let result = Cap::call("search_customer", args).await?;
```