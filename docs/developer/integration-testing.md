# 集成测试指南

> **版本**：v1.4.0 T10
> **依赖**：docker-compose.yml（5 种数据库 + Prometheus）

## 1. 环境准备

### 1.1 前置条件

- Docker 24+ 及 Docker Compose v2
- Rust 1.81+（`rustup default stable`）
- 可选：sccache（编译缓存加速）

### 1.2 一键启动测试环境

```bash
cp .env.example .env
docker-compose up -d
docker-compose ps  # 等待全部 healthy
docker-compose down  # 停止并清理
```

### 1.3 自定义端口

编辑 `.env` 文件修改端口：

```bash
MYSQL_PORT=13306
POSTGRES_PORT=15432
ORACLE_PORT=11521
MSSQL_PORT=11433
SQLITE_PORT=18080
```

## 2. 五种数据库后端测试

### 2.1 MySQL

```bash
docker-compose exec mysql mysql -uroot -psz300_dev_pass -e "SELECT 1"
RUSTC_WRAPPER="" cargo test -p sz-rust-orm-facade --features backend-mysql -- --ignored
```

DSN: `mysql://root:sz300_dev_pass@localhost:3306/sz300`

### 2.2 PostgreSQL

```bash
docker-compose exec postgres psql -U postgres -c "SELECT 1"
RUSTC_WRAPPER="" cargo test -p sz-rust-orm-facade --features backend-postgres -- --ignored
```

DSN: `postgres://postgres:sz300_dev_pass@localhost:5432/sz300`

### 2.3 SQLite

```bash
RUSTC_WRAPPER="" cargo test -p sz-rust-orm-facade --features backend-sqlite -- --ignored
```

DSN: `sqlite://./data/sz300.db`

### 2.4 Oracle

```bash
docker-compose exec oracle sqlplus sz300/sz300_dev_pass@//localhost:1521/XEPDB1 -c "SELECT 1 FROM DUAL"
RUSTC_WRAPPER="" cargo test -p sz-rust-orm-facade --features backend-oracle -- --ignored
```

DSN: `oracle://sz300:sz300_dev_pass@localhost:1521/XEPDB1`

### 2.5 MSSQL

```bash
docker-compose exec mssql /opt/mssql-tools/bin/sqlcmd -S localhost -U sa -P "Sz300DevPass2026" -Q "SELECT 1"
RUSTC_WRAPPER="" cargo test -p sz-rust-orm-facade --features backend-mssql -- --ignored
```

DSN: `mssql://sa:Sz300DevPass2026@localhost:1433/sz300`

## 3. 全后端测试

```bash
docker-compose up -d
docker-compose ps  # 等待全部 healthy
RUSTC_WRAPPER="" cargo test -p sz-rust-orm-facade --features backend-all -- --ignored
RUSTC_WRAPPER="" cargo test -p sz-rust-sz300 --lib
```

## 4. CI 环境配置

```yaml
services:
  mysql:
    image: mysql:8.0
    env:
      MYSQL_ROOT_PASSWORD: test_pass
    ports: ["3306:3306"]
    options: >-
      --health-cmd "mysqladmin ping"
      --health-interval 10s
      --health-timeout 5s
      --health-retries 5

steps:
  - run: RUSTC_WRAPPER="" cargo test -p sz-rust-orm-facade --features backend-mysql -- --ignored
```

## 5. 故障排查

| 问题 | 解决方案 |
|------|---------|
| Oracle 容器启动慢 | 等待 30-60 秒，`docker-compose logs oracle` 查看进度 |
| MSSQL 连接失败 | 检查 SA 密码复杂度要求（需含大小写+数字） |
| 端口冲突 | 修改 `.env` 中对应 `*_PORT` 变量 |
| 测试超时 | 用 `--test-threads=1` 串行运行避免端口争抢 |
