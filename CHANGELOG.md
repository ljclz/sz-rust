# Changelog

All notable changes to SzRSQL are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(see [VERSIONING.md](VERSIONING.md) for SzRSQL-specific compatibility rules).

## [Unreleased]

### Added

- Phase 4.5.10：HTTP 端口配置支持（`--http-port`、`--http-host`、`--http-auth-token` CLI 参数）
- Phase 4.5.9：HTTP 管理端点（`/api/v1/sessions`、`/api/v1/cancel/{pid}`、`/api/v1/backup`、`/api/v1/config/reload`，含 Bearer token 鉴权）
- Phase 4.5.8：HTTP 管理服务器（`/healthz`、`/readyz`、`/metrics`，Prometheus 文本格式指标）
- Phase 4.13：进程守护化（`--daemon` Unix 双 fork + setsid）+ PID 文件 RAII 管理（`--pid-file`，重复启动检测、stale 清理、自动删除）
- Phase 4.12：信号处理（SIGTERM 优雅关闭 / SIGINT 立即关闭）+ Crash Handler（panic hook 写入崩溃日志含 backtrace + WAL LSN 占位）
- Phase 4.11：优雅关闭（Graceful Shutdown）— SIGTERM → Draining → 排空活跃连接 → Closed
- Phase 4.10：Rust sqlx 驱动验证（pgwire 协议兼容性）
- Phase 4.9：psql 互操作验证（简单查询、扩展查询、事务、COPY）
- Phase 4.8：COPY FROM/TO（CSV/TEXT 文件批量导入导出）
- Phase 4.7：元数据查询（pg_tables/pg_indexes + information_schema.tables/columns 等系统表）
- Phase 4.6：错误码 + 通知（LISTEN/NOTIFY/UNLISTEN 跨会话通知）
- Phase 4.5：TLS 1.3 加密（rustls 集成，支持 SSLRequest 协商）
- Phase 4.4：认证（trust 免密 + SCRAM-SHA-256 密码认证，RFC 5802 + RFC 7677）
- Phase 4.3：扩展查询协议（Parse/Bind/Execute/Describe/Close/Sync/Flush）
- Phase 4.2：简单查询协议（接入真实 SQL 执行器）
- Phase 4.1：pgwire 服务器启动（监听 5432 端口，启动消息握手）

### Changed

- 版本号集中管理：`Cargo.toml` 的 `workspace.package.version` 是唯一来源，所有子 crate 通过 `version.workspace = true` 继承

### Security

- SCRAM-SHA-256 认证使用恒定时间比较防止时序攻击
- TLS 1.3 默认拒绝客户端证书验证（`sslmode=verify-full` 时强制验证）

## [0.1.0] - 2026-07-20

### Added

- 项目骨架：14 个 workspace crate（types、storage、tx、cdc、sql、catalog、protocol、optimizer、ai、security、scheduler、replication、dist、pgcompat、bin）
- Phase 1-3：存储引擎（B+Tree、Buffer Pool、Page）、事务引擎（MVCC、WAL、Lock）、SQL 引擎（Parser、AST、Executor）
- Phase 4.1-4.13：pgwire 协议层完整实现
- 402 单元/集成测试通过（3 ignored）

### Notes

- Alpha 阶段版本，不承诺兼容性
- 预期 GA 版本 `1.0.0`（Phase 8 完成后，Jepsen + 7×24h 长稳测试通过）

[Unreleased]: https://github.com/szrsql/szrsql/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/szrsql/szrsql/releases/tag/v0.1.0
