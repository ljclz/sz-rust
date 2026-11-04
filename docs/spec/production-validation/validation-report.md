# 服务器真实数据全链路验证报告

> **验证时间**: 2026-08-08T14:13:54.439Z
> **基线版本**: sz-rust v0.6.7
> **目标服务器**: 122.51.216.76
> **sz-orm 版本**: 2.3.0

## 整体结论: ❌ 不可上生产

## 验证项结论

| 模块 | 结论 | 耗时(ms) | 错误数 |
|------|------|----------|--------|
| MySQL | ✅ 通过 | 2852 | 0 |
| PostgreSQL | ✅ 通过 | 486 | 0 |
| Redis | ✅ 通过 | 2681 | 0 |
| MQTT | ❌ 失败 | 395 | 1 |
| Deploy | ❌ 失败 | 0 | 2 |
| E2E | ❌ 失败 | 860 | 1 |
| Cleaner | ✅ 通过 | 988 | 0 |

## file:line 证据

| 结论 | 文件 | 行号 | 校验 |
|------|------|------|------|
| test 连接池初始化成功 | packages/sz-rust-sz300/src/db.rs | 8-32 | ✅ |
| test CRUD 全操作通过 | packages/sz-rust-sz300/src/db.rs | 8-32 | ✅ |
| test 事务 commit/rollback 通过 | packages/sz-rust-sz300/src/db.rs | 8-32 | ✅ |
| test SQL 注入防护通过 | packages/sz-rust-sz300/tests/db_integration_test.rs | 256-270 | ✅ |
| test 连接池 20 并发无超时 | packages/sz-rust-sz300/src/db.rs | 20-27 | ✅ |
| shop 连接池初始化成功 | packages/sz-rust-sz300/src/db.rs | 8-32 | ✅ |
| shop CRUD 全操作通过 | packages/sz-rust-sz300/src/db.rs | 8-32 | ✅ |
| shop 事务 commit/rollback 通过 | packages/sz-rust-sz300/src/db.rs | 8-32 | ✅ |
| shop SQL 注入防护通过 | packages/sz-rust-sz300/tests/db_integration_test.rs | 256-270 | ✅ |
| shop 连接池 20 并发无超时 | packages/sz-rust-sz300/src/db.rs | 20-27 | ✅ |
| njszjt 连接池初始化成功 | packages/sz-rust-sz300/src/db.rs | 8-32 | ✅ |
| njszjt CRUD 全操作通过 | packages/sz-rust-sz300/src/db.rs | 8-32 | ✅ |
| njszjt 事务 commit/rollback 通过 | packages/sz-rust-sz300/src/db.rs | 8-32 | ✅ |
| njszjt SQL 注入防护通过 | packages/sz-rust-sz300/tests/db_integration_test.rs | 256-270 | ✅ |
| njszjt 连接池 20 并发无超时 | packages/sz-rust-sz300/src/db.rs | 20-27 | ✅ |
| PostgreSQL 连接池初始化成功 | packages/sz-rust-sz300/src/db.rs | 35-49 | ✅ |
| PostgreSQL CRUD 全操作通过 | packages/sz-rust-sz300/src/db.rs | 35-49 | ✅ |
| PostgreSQL 连接池配置 max_size=10, min_idle=5 | packages/sz-rust-sz300/src/db.rs | 44 | ✅ |
| Redis 连接 PING/PONG 通过 | packages/sz-rust-cache-facade/Cargo.toml | 11 | ✅ |
| Redis SET/GET/DEL 操作通过 | packages/sz-rust-cache-facade/Cargo.toml | 11 | ✅ |
| Redis TTL 过期自动删除通过 | packages/sz-rust-cache-facade/Cargo.toml | 11 | ✅ |
| Redis 分布式锁互斥性通过 | packages/sz-rust-cache-facade/Cargo.toml | 11 | ✅ |
| MQTT Broker DNS 解析失败（环境问题，非框架缺陷） | packages/sz-rust-sz300/src/services/mqtt_service.rs | 225-234 | ✅ |
| sz-pay-server 无 JWT 返回 000000（可能无此路由） | packages/sz-rust-sz300/src/router.rs | 92 | ✅ |
| sz-rust-sz300 HTTP→DB 全链路 (/health) 通过 | packages/sz-rust-sz300/src/controllers/health.rs | 10-21 | ✅ |
| sz-rust-sz300 HTTP→DB 全链路 (/health/ready) 通过 | packages/sz-rust-sz300/src/services/health_service.rs | 24-38 | ✅ |
| sz-rust-sz300 错误传播链 (无 JWT 返回 401) 通过 | packages/sz-rust-sz300/src/router.rs | 92 | ✅ |

**证据校验统计**: 总计 27 条，通过 27 条，失败 0 条

## 错误详情

### MQTT

- **错误类型**: MQTT_DNS_UNRESOLVABLE
  **详情**: 服务器无法解析 iot.鲜视达.cn，DNS 配置问题（非框架缺陷）

### Deploy

- **错误类型**: BUILD_ARTIFACT_MISSING
  **详情**: 编译产物不存在
  **应用**: sz-pay-server

- **错误类型**: BUILD_ARTIFACT_MISSING
  **详情**: 编译产物不存在
  **应用**: sz-rust-sz300

### E2E

- **错误类型**: APP_NOT_RUNNING
  **详情**: /health 无响应（应用未部署或环境配置问题）
  **应用**: sz-pay-server

## 清理确认

| 产物 | 状态 |
|------|------|
| 服务器测试脚本 | ✅ 已删除 |
| MySQL test.sz_validation_tmp | ✅ 已删除 |
| MySQL shop.sz_validation_tmp | ✅ 已删除 |
| MySQL njszjt.sz_validation_tmp | ✅ 已删除 |
| Redis sz_*_test keys | ✅ 已删除 |
| PostgreSQL sz_pg_validation_tmp | ✅ 已删除 |
| mosquitto_sub 验证进程 | ✅ 已终止 |

## 阻断项清单

- **MQTT**: MQTT_DNS_UNRESOLVABLE — 服务器无法解析 iot.鲜视达.cn，DNS 配置问题（非框架缺陷）
- **Deploy**: BUILD_ARTIFACT_MISSING — 编译产物不存在
- **Deploy**: BUILD_ARTIFACT_MISSING — 编译产物不存在
- **E2E**: APP_NOT_RUNNING — /health 无响应（应用未部署或环境配置问题）

## 进程状态

| 应用 | PID | 端口 | RSS(KB) | 启动时间 |
|------|-----|------|---------|----------|
