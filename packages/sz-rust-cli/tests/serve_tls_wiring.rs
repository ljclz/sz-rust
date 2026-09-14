// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! serve 命令 TLS 接线验证测试
//!
//! 验证：
//! - CLI 参数 `--tls-cert` / `--tls-key` 正确解析到 ServeArgs
//! - `--tls-cert` 和 `--tls-key` 必须同时提供或同时缺失（validate 校验）
//! - TLS 证书文件不存在时 load_tls_config 返回错误（复用 sz-rust-core 测试）

use clap::Parser;
use sz_rust_cli::cmd::serve::ServeArgs;
use sz_rust_cli::{Cli, CliCommand};

/// `serve --tls-cert cert.pem --tls-key key.pem` 解析正确
#[test]
fn test_parse_serve_tls_args() {
    let cli = Cli::parse_from([
        "sz-rust",
        "serve",
        "--tls-cert",
        "cert.pem",
        "--tls-key",
        "key.pem",
    ]);
    match cli.command {
        Some(CliCommand::Serve {
            tls_cert, tls_key, ..
        }) => {
            assert_eq!(tls_cert.as_deref(), Some(std::path::Path::new("cert.pem")));
            assert_eq!(tls_key.as_deref(), Some(std::path::Path::new("key.pem")));
        }
        _ => panic!("expected Serve command"),
    }
}

/// `serve` 默认不提供 TLS 参数
#[test]
fn test_parse_serve_no_tls_default() {
    let cli = Cli::parse_from(["sz-rust", "serve"]);
    match cli.command {
        Some(CliCommand::Serve {
            tls_cert, tls_key, ..
        }) => {
            assert!(tls_cert.is_none());
            assert!(tls_key.is_none());
        }
        _ => panic!("expected Serve command"),
    }
}

/// ServeArgs validate：tls_cert 有值但 tls_key 无值时拒绝
#[test]
fn test_validate_tls_cert_without_key_rejected() {
    let args = ServeArgs {
        with_admin: false,
        addr: "0.0.0.0:8080".to_string(),
        watch_config: false,
        workers: None,
        grace_timeout: None,
        tls_cert: Some(std::path::PathBuf::from("cert.pem")),
        tls_key: None,
        access_log: false,
        health: true,
    };
    assert!(args.validate().is_err());
}

/// ServeArgs validate：tls_cert 和 tls_key 都有值时通过
#[test]
fn test_validate_tls_cert_and_key_ok() {
    let args = ServeArgs {
        with_admin: false,
        addr: "0.0.0.0:8080".to_string(),
        watch_config: false,
        workers: None,
        grace_timeout: None,
        tls_cert: Some(std::path::PathBuf::from("cert.pem")),
        tls_key: Some(std::path::PathBuf::from("key.pem")),
        access_log: false,
        health: true,
    };
    assert!(args.validate().is_ok());
}

/// ServeArgs validate：tls_cert 和 tls_key 都无值时通过
#[test]
fn test_validate_no_tls_ok() {
    let args = ServeArgs {
        with_admin: false,
        addr: "0.0.0.0:8080".to_string(),
        watch_config: false,
        workers: None,
        grace_timeout: None,
        tls_cert: None,
        tls_key: None,
        access_log: false,
        health: true,
    };
    assert!(args.validate().is_ok());
}

/// ServeArgs validate：workers=0 拒绝
#[test]
fn test_validate_workers_zero_rejected() {
    let args = ServeArgs {
        with_admin: false,
        addr: "0.0.0.0:8080".to_string(),
        watch_config: false,
        workers: Some(0),
        grace_timeout: None,
        tls_cert: None,
        tls_key: None,
        access_log: false,
        health: true,
    };
    assert!(args.validate().is_err());
}

/// ServeArgs validate：workers>1024 拒绝
#[test]
fn test_validate_workers_over_limit_rejected() {
    let args = ServeArgs {
        with_admin: false,
        addr: "0.0.0.0:8080".to_string(),
        watch_config: false,
        workers: Some(1025),
        grace_timeout: None,
        tls_cert: None,
        tls_key: None,
        access_log: false,
        health: true,
    };
    assert!(args.validate().is_err());
}

/// ServeArgs validate：grace_timeout>300 拒绝
#[test]
fn test_validate_grace_timeout_over_limit_rejected() {
    let args = ServeArgs {
        with_admin: false,
        addr: "0.0.0.0:8080".to_string(),
        watch_config: false,
        workers: None,
        grace_timeout: Some(301),
        tls_cert: None,
        tls_key: None,
        access_log: false,
        health: true,
    };
    assert!(args.validate().is_err());
}
