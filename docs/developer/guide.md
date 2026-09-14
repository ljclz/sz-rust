# 插件开发指南

## 1. 创建插件清单

插件清单支持 JSON 和 TOML 两种格式。

### JSON 格式（`manifest.json`）

```json
{
    "name": "search_customer",
    "title": "客户搜索插件",
    "identifier": "crm.search_customer",
    "version": "1.0.0",
    "author": "your-username",
    "description": "提供客户搜索功能",
    "tags": ["crm", "search"],
    "source": "plugin",
    "license": "MIT",
    "capabilities": [
        {
            "name": "crm.search_customer",
            "description": "搜索客户",
            "schema": {},
            "tags": ["search"],
            "requires_confirmation": false
        }
    ],
    "dependencies": [],
    "permissions": []
}
```

## 2. 实现 Capability

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use sz_rust_capability::{Capability, CapabilitySource, CapResult};

struct SearchCustomerCapability;

#[async_trait]
impl Capability for SearchCustomerCapability {
    fn name(&self) -> &'static str { "crm.search_customer" }
    fn description(&self) -> &'static str { "搜索客户" }
    fn schema(&self) -> Value { json!({}) }
    fn tags(&self) -> &[&'static str] { &["search"] }
    fn source(&self) -> CapabilitySource { CapabilitySource::Plugin }
    async fn call(&self, args: Value) -> CapResult<Value> {
        Ok(json!({ "results": [] }))
    }
}
```

## 3. 生成 Ed25519 签名密钥

```rust
use ed25519_dalek::SigningKey;
use base64::Engine;

let key = SigningKey::generate(&mut rand::rngs::OsRng);
let pub_key = key.verifying_key();
println!("private: {}", base64::engine::general_purpose::STANDARD.encode(key.to_bytes()));
println!("public: {}", base64::engine::general_purpose::STANDARD.encode(pub_key.to_bytes()));
```

## 4. 发布插件

```bash
# 打包插件
tar czf my-plugin.tar.gz manifest.json src/

# 签名
sz-rust plugin publish --path my-plugin.tar.gz --sign private_key.pem
```

## 5. 安装插件

```bash
sz-rust plugin install crm.search_customer
```

安装后插件自动注册到 Capability Registry。