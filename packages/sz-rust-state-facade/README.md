# sz-rust-state-facade

SZ-Rust 应用状态管理。包含 session、cookie、env、event、i18n、mail、notify 七大模块。

## 功能

| 模块 | 对齐 PHP | 说明 |
|------|---------|------|
| `session` | `think\facade\Session` | SessionStore trait + MemorySessionStore |
| `cookie` | `think\Cookie` | CookieJar + CookieOptions |
| `env` | `think\facade\Env` | 环境变量管理（`Env::get`） |
| `event` | `think\Event` | Listener/Subscriber/Observer 事件系统 |
| `i18n` | `think\facade\Lang` | 多语言国际化 |
| `mail` | `think\facade\Mail` | Mailer trait + MemoryMailer |
| `notify` | `think\facade\Notify` | Notifier trait + SlackNotifier |

## 用法

```rust
use sz_rust_state_facade::session::SessionStore;
use sz_rust_state_facade::event::{EventDispatcher, Listener};
use sz_rust_state_facade::env::Env;

// 环境变量
let db_url = Env::get("DATABASE_URL");
```

## 依赖

- `axum`
- `chrono`
- `parking_lot`
- `serde` / `serde_json`
- `thiserror`

## 版本策略

与 `sz-rust-core` 保持同步。
