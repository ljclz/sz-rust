> **中文** | [English](README.en.md)

# sz-rust-auth-facade

SZ-Rust 认证与网关层。包含 wechat、oauth、gateway 三大模块。

## 功能

| 模块 | 对齐 PHP | 说明 |
|------|---------|------|
| `wechat` | `EasyWeChat` | 公众号/小程序/开放平台/企业微信 SDK |
| `oauth` | Laravel Socialite | OAuth2Provider trait + GenericOAuth2Provider |
| `gateway` | GatewayWorker | WebSocket 客户端管理 + GatewayTransport trait |

## 用法

```rust
use sz_rust_auth_facade::wechat::WechatSdk;
use sz_rust_auth_facade::oauth::{OAuth2Provider, GenericOAuth2Provider};
use sz_rust_auth_facade::gateway::{Gateway, GatewayTransport};

// OAuth2 登录
let provider = GenericOAuth2Provider::new(config)?;
let user = provider.user_from_code(code).await?;
```

## 依赖

- `parking_lot`
- `serde` / `serde_json`
- `sha1` / `hex`
- `thiserror`

## 版本策略

与 `sz-rust-core` 保持同步。
