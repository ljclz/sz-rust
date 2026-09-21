# sz-rust-state-facade

> **中文** | [English](README.en.md)

SZ-Rust application state management. Contains seven modules: session, cookie, env, event, i18n, mail, notify.

## Features

| Module | PHP Alignment | Description |
|--------|---------------|-------------|
| `session` | `think\facade\Session` | SessionStore trait + MemorySessionStore |
| `cookie` | `think\Cookie` | CookieJar + CookieOptions |
| `env` | `think\facade\Env` | Environment variable management (`Env::get`) |
| `event` | `think\Event` | Listener/Subscriber/Observer event system |
| `i18n` | `think\facade\Lang` | Multi-language internationalization |
| `mail` | `think\facade\Mail` | Mailer trait + MemoryMailer |
| `notify` | `think\facade\Notify` | Notifier trait + SlackNotifier |

## Usage

```rust
use sz_rust_state_facade::session::SessionStore;
use sz_rust_state_facade::event::{EventDispatcher, Listener};
use sz_rust_state_facade::env::Env;

// Environment variable
let db_url = Env::get("DATABASE_URL");
```

## Dependencies

- `axum`
- `chrono`
- `parking_lot`
- `serde` / `serde_json`
- `thiserror`

## Version Policy

Keeps in sync with `sz-rust-core`.