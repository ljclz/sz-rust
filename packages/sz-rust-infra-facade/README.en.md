# sz-rust-infra-facade

> **中文** | [English](README.en.md)

SZ-Rust infrastructure layer. Contains five modules: config, validate, static_files, upload, debug_page.

## Features

| Module | PHP Alignment | Description |
|--------|---------------|-------------|
| `config` | `config/app.php` | YAML loading + env override + defaults |
| `validate` | `think\Validate` | Data validator (rule engine + scenarios + messages) |
| `static_files` | Static file routing | `tower-http::ServeDir` wrapper |
| `upload` | `think\File` | File upload + image processing + cloud storage drivers |
| `debug_page` | Whoops | Dev debug page + production concise page |

## Usage

```rust
use sz_rust_infra_facade::config::Config;
use sz_rust_infra_facade::validate::{Validator, rule};

// Config loading
let config = Config::load("config/app.yml")?;

// Data validation
let v = Validator::new(data);
v.rule("email", "required|email");
```

## Dependencies

- `axum`
- `serde_yml`
- `image` / `ab_glyph` (image processing)
- `sz-rust-orm-facade` (cloud storage drivers, via orm-facade)
- `tower-http`
- `tokio` / `tokio-util`

## Version Policy

Keeps in sync with `sz-rust-core`.