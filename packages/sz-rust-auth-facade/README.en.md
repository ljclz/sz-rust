# sz-rust-auth-facade

> **中文** | [English](README.en.md)

SZ-Rust authentication and gateway layer. Contains three major modules: wechat, oauth, gateway.

## Features

| Module | PHP Alignment | Description |
|--------|---------------|-------------|
| `wechat` | `EasyWeChat` | Official Account / Mini Program / Open Platform / Enterprise WeChat SDK |
| `oauth` | Laravel Socialite | OAuth2Provider trait + GenericOAuth2Provider |
| `gateway` | GatewayWorker | WebSocket client management + GatewayTransport trait |

## Usage

```rust
use sz_rust_auth_facade::wechat::WechatSdk;
use sz_rust_auth_facade::oauth::{OAuth2Provider, GenericOAuth2Provider};
use sz_rust_auth_facade::gateway::{Gateway, GatewayTransport};

// OAuth2 login
let provider = GenericOAuth2Provider::new(config)?;
let user = provider.user_from_code(code).await?;
```

## Dependencies

- `parking_lot`
- `serde` / `serde_json`
- `sha1` / `hex`
- `thiserror`

## Version Policy

Keeps in sync with `sz-rust-core`.