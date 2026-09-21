# SZ-Rust 插件市场 (`sz-rust-marketplace`)

> P2-2 生态件：插件清单管理、Ed25519 签名校验、对象存储抽象、审核流程、Web API、CLI 客户端。

## 架构

```
sz-rust-marketplace/
├── src/
│   ├── lib.rs          # 模块导出 (#![forbid(unsafe_code)])
│   ├── error.rs        # MarketplaceError 14 变体 + thiserror
│   ├── manifest.rs     # MarketplaceManifest + serde flatten + 三格式解析
│   ├── signature.rs    # Ed25519 签名/校验 + SHA256 + pubkey fingerprint
│   ├── storage.rs      # ObjectStore trait + LocalObjectStore
│   ├── repository.rs   # 5 实体 + 5 Repository CRUD (sqlx)
│   ├── lockfile.rs     # LockfileManager read/write/update/remove
│   ├── service.rs      # MarketplaceService publish/search/install/review
│   ├── web.rs          # axum 路由 + JWT 鉴权 + OpenAPI
│   └── client.rs       # MarketplaceClient (reqwest, 7 方法)
├── migrations/         # 5 SQL 迁移脚本
├── docker-compose.yml  # 三服务编排 (web + postgres + minio)
├── Dockerfile          # 多阶段构建 (distroless)
└── tests/
    └── e2e_flow.sh     # 端到端闭环测试
```

## API 端点

| 方法 | 路径 | 鉴权 | 说明 |
|------|------|------|------|
| GET  | `/api/v1/plugins/search` | 公开 | 搜索插件（keyword/tag/limit/offset） |
| GET  | `/api/v1/plugins/:name` | 公开 | 获取插件详情 |
| GET  | `/api/v1/plugins/:name/:version/download` | 公开 | 下载插件归档 |
| POST | `/api/v1/plugins/publish` | Bearer | 发布插件（multipart） |
| GET  | `/api/v1/admin/reviews/pending` | Bearer+审核员 | 待审核列表 |
| POST | `/api/v1/admin/reviews/:version_id/approve` | Bearer+审核员 | 批准版本 |
| POST | `/api/v1/admin/reviews/:version_id/reject` | Bearer+审核员 | 拒绝版本 |
| POST | `/api/v1/auth/login` | 公开 | 登录获取 JWT |
| GET  | `/api/v1/health` | 公开 | 健康检查 |
| GET  | `/api/v1/openapi.json` | 公开 | OpenAPI 3.0 文档 |

## CLI 用法

```bash
# 搜索插件
sz-rust plugin search crm --tags business

# 登录市场
sz-rust plugin login --token <JWT> --url http://localhost:8080

# 安装插件
sz-rust plugin install crm@1.0.0

# 发布插件
sz-rust plugin publish -p ./my-plugin.tar.gz -s ./key.pem

# 列出已安装
sz-rust plugin list

# 更新插件
sz-rust plugin update crm

# 卸载插件
sz-rust plugin uninstall crm
```

## 部署

### Docker Compose

```bash
# 启动
docker compose -f packages/sz-rust-marketplace/docker-compose.yml up -d

# 验证
curl http://localhost:8080/api/v1/health

# 端到端测试
bash packages/sz-rust-marketplace/tests/e2e_flow.sh

# 清理
docker compose -f packages/sz-rust-marketplace/docker-compose.yml down -v
```

### 环境变量

| 变量 | 说明 | 默认 |
|------|------|------|
| `DATABASE_URL` | PostgreSQL 连接串 | `postgres://szrust:szrust@postgres-14:5432/szrust_marketplace` |
| `OBJECT_STORE_ENDPOINT` | S3 兼容存储端点 | `http://minio:9000` |
| `JWT_SECRET` | JWT 签名密钥 | — |
| `ED25519_PUBLIC_KEY` | Ed25519 公钥（Base64） | — |
| `RUST_LOG` | 日志级别 | `info` |

## 清单 Schema

插件清单支持 JSON / TOML / PHP 三种格式，核心字段：

```json
{
  "name": "crm",
  "identifier": "com.szrust.crm",
  "title": "CRM 插件",
  "version": "1.0.0",
  "author": "alice",
  "license": "Apache-2.0",
  "description": "客户关系管理插件",
  "tags": ["business", "crm"],
  "price": 0.0,
  "homepage": "https://github.com/szrust/crm",
  "signature": "<Ed25519 Base64>",
  "review_status": "pending",
  "capabilities": ["database.read", "database.write"],
  "dependencies": [
    { "name": "orm", "version": ">=6.0.0" }
  ]
}
```

## 测试

```bash
# 单元 + 集成测试
cargo test -p sz-rust-marketplace
# 34 passed; 0 failed

# Clippy
cargo clippy -p sz-rust-marketplace -- -D warnings
```

## 设计约束

- `#![forbid(unsafe_code)]`
- 所有 `async fn` 必须 `Send + 'static`
- 禁止 `std::fs`，统一 `tokio::fs`
- SQL 显式列投影，禁止 `SELECT *`
- 所有 WHERE 条件参数化绑定
- Ed25519 签名 + SHA256 校验和双重验证
- SemVer 严格递增（新版本 > 最新版本）
- 审核流程：append-only 审核记录 + 自审禁止 + 仅 pending 可审核