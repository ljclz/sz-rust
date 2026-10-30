# Admin Monitor API 测试指南

## 端点一览

| 端点 | 方法 | 鉴权 | 说明 |
|------|------|------|------|
| `/api/admin/server/info` | GET | `admin` 角色 | 服务器 CPU/内存/磁盘/负载/Rust版本/主机名 |
| `/api/admin/db/pool` | GET | `admin` 角色 | 数据库连接池 active/idle/max/usage |
| `/api/admin/redis/info` | GET | `admin` 角色 | Redis 版本/模式/连接数/内存/角色（无连接时降级） |

## 快速测试

```bash
# 1. 启动 MySQL（Docker）
echo "SZ300_JWT_SECRET=test-secret-2026" > .env
echo "SZ300_DB_PASSWORD=test-secret-2026" >> .env
docker compose up -d mysql

# 2. 启动服务器
SZ300_JWT_SECRET=test-secret-2026 \
SZ300_DB_HOST=127.0.0.1 \
SZ300_DB_PASSWORD=test-secret-2026 \
cargo run -p sz-rust-sz300 --features admin --bin sz300-server

# 3. 运行测试脚本（另一终端）
bash scripts/test-admin-api.sh
```

## 手动 curl 测试

```bash
SECRET="test-secret-2026"
BASE="http://127.0.0.1:8300"

# 生成 admin 令牌（需要 pyjwt: pip install pyjwt）
TOKEN=$(python3 -c "
import jwt, time
print(jwt.encode({
    'sub': 'admin',
    'exp': int(time.time()) + 3600,
    'iat': int(time.time()),
    'iss': 'sz300',
    'roles': ['admin'],
    'user_id': 1
}, '$SECRET', algorithm='HS256'))
")

# 1. 服务器信息
curl -sS -H "Authorization: Bearer $TOKEN" "$BASE/api/admin/server/info" | python3 -m json.tool

# 2. 数据库连接池
curl -sS -H "Authorization: Bearer $TOKEN" "$BASE/api/admin/db/pool" | python3 -m json.tool

# 3. Redis 信息（无 ADMIN_REDIS_URL 时返回 connected=false）
curl -sS -H "Authorization: Bearer $TOKEN" "$BASE/api/admin/redis/info" | python3 -m json.tool

# 4. 无令牌 → 401
curl -sS -w "\nHTTP %{http_code}\n" "$BASE/api/admin/server/info"

# 5. 无 admin 角色 → 403
TOKEN_USER=$(python3 -c "
import jwt, time
print(jwt.encode({
    'sub': 'user',
    'exp': int(time.time()) + 3600,
    'iat': int(time.time()),
    'iss': 'sz300',
    'roles': ['user'],
    'user_id': 2
}, '$SECRET', algorithm='HS256'))
")
curl -sS -w "\nHTTP %{http_code}\n" -H "Authorization: Bearer $TOKEN_USER" "$BASE/api/admin/server/info"
```

## 预期响应示例

### `GET /api/admin/server/info` (200)

```json
{
  "cpu_usage_percent": 12.5,
  "memory_total_bytes": 17179869184,
  "memory_used_bytes": 8589934592,
  "memory_usage_percent": 50.0,
  "disk_total_bytes": 500000000000,
  "disk_used_bytes": 250000000000,
  "load_avg": { "one": 1.2, "five": 0.8, "fifteen": 0.5 },
  "process_start_time": 1723300000,
  "rust_version": "rustc 1.81.0",
  "hostname": "my-server"
}
```

### `GET /api/admin/db/pool` (200)

```json
{
  "active": 3,
  "idle": 7,
  "max": 10,
  "usage_percent": 30.0
}
```

### `GET /api/admin/redis/info` (200，已连接)

```json
{
  "connected": true,
  "redis_version": "7.2.3",
  "redis_mode": "standalone",
  "connected_clients": 12,
  "used_memory_human": "1.20M",
  "uptime_in_seconds": 86400,
  "role": "master"
}
```

### `GET /api/admin/redis/info` (200，降级 — 未配置 ADMIN_REDIS_URL)

```json
{
  "connected": false,
  "redis_version": "",
  "redis_mode": "",
  "connected_clients": 0,
  "used_memory_human": "",
  "uptime_in_seconds": 0,
  "role": ""
}
```

### 无令牌 (401)

```
未提供认证令牌
```

### 无 admin 角色 (403)

```
需要 admin 角色才能访问此资源
```

## 单元测试

```bash
# observability admin 模块（16 个测试）
cargo test -p sz-rust-observability --features admin --lib admin

# RoleGuard 中间件（4 个测试）
cargo test -p sz-rust-sz300 --features admin --lib role_guard
```

## 编译验证

```bash
# admin feature 启用
cargo check -p sz-rust-sz300 --features admin

# 默认（admin 关闭，端点不编译进二进制）
cargo check -p sz-rust-sz300
```
