# Phase -1.1：SZ-ORM 依赖清单

> 验证日期：2026-07-20
> 验证结果：✅ 全部通过

## 验证概要

| 维度 | 结果 |
|------|------|
| Workspace 成员 | 36 个 sz-orm 包 + cli + examples = 38 个 |
| 核心版本 | sz-orm-core v0.2.0，resolver = "2"，edition = "2021" |
| cargo check | ✅ 编译通过（1.72s，0 warnings） |
| 关键依赖 | tokio 1.40+ / async-trait 0.1 / thiserror 2 / serde 1 / chrono 0.4 / parking_lot 0.12 |

## 36 个 sz-orm 包清单

### 核心层（6）
| 包名 | 功能 | 备注 |
|------|------|------|
| sz-orm-core | ORM 核心引擎 | 分层+插件化，15 模块 |
| sz-orm-query-builder | 链式查询构建器 | 11 方言 |
| sz-orm-sql-validator | SQL 编译时+运行时双层校验 | 安全 |
| sz-orm-macros | 派生宏（ModelExt 等） | 编译期生成 |
| sz-orm-config | 配置系统 | 多源合并 |
| sz-orm-lc | 生命周期管理 | 优雅关闭 |

### 存储层（4）
| 包名 | 功能 | 备注 |
|------|------|------|
| sz-orm-sqlx | sqlx 适配器 | 真实 MySQL/PG/SQLite/Oracle |
| sz-orm-mig | 迁移引擎 | SchemaBuilder + 各语言方言 |
| sz-orm-storage | 文件存储抽象 | S3/阿里/腾讯/华为/七牛/又拍/本地 |
| sz-orm-back | 灾备（Backup/Restore） | 全量/增量 |

### 安全层（3）
| 包名 | 功能 | 备注 |
|------|------|------|
| sz-orm-auth | JWT(HS256) + 认证 | 替代 firebase/php-jwt + lcobucci/jwt |
| sz-orm-crypto | AES-256-GCM + PBKDF2 | 替代 phpseclib/ext-openssl |
| sz-orm-masking | 数据脱敏 | 身份证/手机号/邮箱掩码 |

### 并发与事务（4）
| 包名 | 功能 | 备注 |
|------|------|------|
| sz-orm-dtx | 分布式事务 | TCC + Saga + CrossShard |
| sz-orm-rw | 读写分离 | 主从路由 |
| sz-orm-sharding | 分片 | 分库分表 |
| sz-orm-limit | 限流 | 令牌桶/漏桶 |

### 通信层（5）
| 包名 | 功能 | 备注 |
|------|------|------|
| sz-orm-mqtt | MQTT 客户端 | 替代 workerman/mqtt |
| sz-orm-websocket | WebSocket | 原生异步 |
| sz-orm-queue | 队列抽象 | RabbitMQ/Kafka/NATS 等 7 种 |
| sz-orm-grpc | gRPC | tonic 包装 |
| sz-orm-graphql | GraphQL | async-graphql 包装 |

### 可观测层（6）
| 包名 | 功能 | 备注 |
|------|------|------|
| sz-orm-tracing | OpenTelemetry 追踪 | OTLP exporter |
| sz-orm-logger | 结构化日志 | 替代 think-log |
| sz-orm-health | 健康检查 | 端口+自定义探针 |
| sz-orm-audit | 操作审计 | 入库审计日志 |
| sz-orm-swagger | API 文档 | OpenAPI 自动生成 |
| sz-orm-observability | Prometheus + SLO | 多窗口燃烧率 |

### 数据扩展层（4）
| 包名 | 功能 | 备注 |
|------|------|------|
| sz-orm-postgis | PostGIS 地理空间 | EWKT/EWKB |
| sz-orm-timeseries | 时序数据 | InfluxDB/TimescaleDB |
| sz-orm-search | 全文搜索 | Elasticsearch/Meilisearch |
| sz-orm-es | Elasticsearch 客户端 | 原生异步 |

### AI 与 WASM（2）
| 包名 | 功能 | 备注 |
|------|------|------|
| sz-orm-ai | AI 集成 | Embedding/RAG/Vector |
| sz-orm-wasm | WASM 支持 | 浏览器端运行 |

### 工具链（3）
| 包名 | 功能 | 备注 |
|------|------|------|
| sz-orm-scheduler | Cron 调度器 | 秒级支持 |
| sz-orm-batch | 批量操作 | 批量插入/更新 |
| sz-orm-cli | CLI 工具 | （cli 目录） |
| sz-orm-examples | 示例集 | （examples 目录） |

## SZ-Rust 直接复用策略

| 层 | 直接复用（不重复造轮子） | 需要包装适配 |
|----|------------------------|------------|
| 数据层 | sz-orm-core / query-builder / sqlx / mig | sz-orm-macros 扩展 SZ-Rust Model 派生宏 |
| 安全层 | sz-orm-auth / crypto / masking | 封装为 Guard + AuthService trait |
| 存储 | sz-orm-storage / mqtt / websocket / queue | 封装为 SZ-Rust Service Provider |
| 可观测 | sz-orm-tracing / logger / health / audit | 封装为中间件 + 全局 Handler |
| 调度 | sz-orm-scheduler | 封装为 CLI 命令 `sz schedule:run` |
| 分布式 | sz-orm-dtx / rw / sharding / limit | 预留接口，Phase 10+ 再集成 |
