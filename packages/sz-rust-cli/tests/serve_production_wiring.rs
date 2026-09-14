// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! serve 命令生产化集成测试
//!
//! 覆盖所有生产化特性的 CLI 参数解析验证 + 向后兼容回归测试。
//! 端到端测试使用 build_tcp_listener 绑定随机端口避免冲突。

use clap::Parser;
use sz_rust_cli::{Cli, CliCommand};

// ========== CLI 参数解析验证 ==========

/// `serve` 默认参数：所有新字段为默认值
#[test]
fn test_serve_production_defaults() {
    let cli = Cli::parse_from(["sz-rust", "serve"]);
    match cli.command {
        Some(CliCommand::Serve {
            with_admin,
            addr,
            watch_config,
            workers,
            grace_timeout,
            tls_cert,
            tls_key,
            access_log,
            health,
            no_health,
        }) => {
            assert!(!with_admin, "默认不加载 admin");
            assert_eq!(addr, "0.0.0.0:8080", "默认地址");
            assert!(!watch_config, "默认不启用配置热重载");
            assert!(workers.is_none(), "默认 workers 为 None");
            assert!(grace_timeout.is_none(), "默认 grace_timeout 为 None");
            assert!(tls_cert.is_none(), "默认无 TLS 证书");
            assert!(tls_key.is_none(), "默认无 TLS 私钥");
            assert!(!access_log, "默认不启用访问日志");
            assert!(health, "默认启用健康检查");
            assert!(!no_health, "默认不禁用健康检查");
        }
        _ => panic!("expected Serve command"),
    }
}

/// `serve --with-admin --workers 4 --grace-timeout 30 --health --access-log` 全参数
#[test]
fn test_serve_production_all_args() {
    let cli = Cli::parse_from([
        "sz-rust",
        "serve",
        "--with-admin",
        "--workers",
        "4",
        "--grace-timeout",
        "30",
        "--health",
        "--access-log",
        "--watch-config",
    ]);
    match cli.command {
        Some(CliCommand::Serve {
            with_admin,
            workers,
            grace_timeout,
            access_log,
            watch_config,
            ..
        }) => {
            assert!(with_admin);
            assert_eq!(workers, Some(4));
            assert_eq!(grace_timeout, Some(30));
            assert!(access_log);
            assert!(watch_config);
        }
        _ => panic!("expected Serve command"),
    }
}

/// `serve --no-health` 禁用健康检查
#[test]
fn test_serve_production_no_health() {
    let cli = Cli::parse_from(["sz-rust", "serve", "--no-health"]);
    match cli.command {
        Some(CliCommand::Serve { no_health, .. }) => {
            assert!(no_health);
        }
        _ => panic!("expected Serve command"),
    }
}

/// `serve --tls-cert cert.pem --tls-key key.pem --addr 0.0.0.0:443` TLS 配置
#[test]
fn test_serve_production_tls() {
    let cli = Cli::parse_from([
        "sz-rust",
        "serve",
        "--tls-cert",
        "/etc/ssl/cert.pem",
        "--tls-key",
        "/etc/ssl/key.pem",
        "--addr",
        "0.0.0.0:443",
    ]);
    match cli.command {
        Some(CliCommand::Serve {
            tls_cert,
            tls_key,
            addr,
            ..
        }) => {
            assert_eq!(
                tls_cert.as_deref(),
                Some(std::path::Path::new("/etc/ssl/cert.pem"))
            );
            assert_eq!(
                tls_key.as_deref(),
                Some(std::path::Path::new("/etc/ssl/key.pem"))
            );
            assert_eq!(addr, "0.0.0.0:443");
        }
        _ => panic!("expected Serve command"),
    }
}

// ========== 向后兼容回归测试 ==========

/// `serve` 不传任何新参数时与 commit 12d0695 行为一致
#[test]
fn test_serve_backward_compatible_default() {
    let cli = Cli::parse_from(["sz-rust", "serve"]);
    match cli.command {
        Some(CliCommand::Serve {
            with_admin,
            addr,
            health,
            ..
        }) => {
            // 与 commit 12d0695 一致：with_admin=false, addr=0.0.0.0:8080
            assert!(!with_admin);
            assert_eq!(addr, "0.0.0.0:8080");
            // health 默认 true（新增特性，不影响向后兼容）
            assert!(health);
        }
        _ => panic!("expected Serve command"),
    }
}

/// `serve --with-admin --addr 127.0.0.1:0` 与 commit 12d0695 一致
#[test]
fn test_serve_backward_compatible_with_admin() {
    let cli = Cli::parse_from(["sz-rust", "serve", "--with-admin", "--addr", "127.0.0.1:0"]);
    match cli.command {
        Some(CliCommand::Serve {
            with_admin, addr, ..
        }) => {
            assert!(with_admin);
            assert_eq!(addr, "127.0.0.1:0");
        }
        _ => panic!("expected Serve command"),
    }
}

// ========== Windows CLI 子命令验证 ==========

/// `serve:reload` 子命令解析
#[test]
fn test_parse_serve_reload() {
    let cli = Cli::parse_from(["sz-rust", "serve:reload"]);
    assert!(matches!(cli.command, Some(CliCommand::ServeReload)));
}

/// `serve:log-level` 子命令解析
#[test]
fn test_parse_serve_log_level() {
    let cli = Cli::parse_from(["sz-rust", "serve:log-level"]);
    assert!(matches!(cli.command, Some(CliCommand::ServeLogLevel)));
}

// ========== ServeArgs validate 验证 ==========

/// ServeArgs validate：合法参数通过
#[test]
fn test_serve_args_validate_ok() {
    let args = sz_rust_cli::cmd::serve::ServeArgs {
        with_admin: true,
        addr: "0.0.0.0:8080".to_string(),
        watch_config: true,
        workers: Some(4),
        grace_timeout: Some(30),
        tls_cert: None,
        tls_key: None,
        access_log: true,
        health: true,
    };
    assert!(args.validate().is_ok());
}

/// ServeArgs validate：workers=0 拒绝
#[test]
fn test_serve_args_validate_workers_zero() {
    let args = sz_rust_cli::cmd::serve::ServeArgs {
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

/// ServeArgs validate：TLS 证书和私钥必须同时提供
#[test]
fn test_serve_args_validate_tls_pair() {
    let args = sz_rust_cli::cmd::serve::ServeArgs {
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
