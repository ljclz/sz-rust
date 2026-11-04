# sccache 编译缓存使用指南

> **版本**：v1.4.0（T01）
> **配置文件**：`.cargo/config.toml`
> **统计脚本**：`scripts/sccache-stats.sh`

## 1. 概述

sccache（shared compilation cache）是 Rust/C/C++ 的编译缓存工具，通过缓存编译产物加速重复编译。sz-rust 项目在 v1.4.0 集成 sccache，目标命中率 ≥ 70%。

## 2. 安装

### 2.1 Rust 工具链安装（推荐）

```bash
cargo install sccache
```

### 2.2 平台包管理器安装

| 平台 | 命令 |
|------|------|
| macOS | `brew install sccache` |
| Ubuntu/Debian | `apt install sccache` |
| Windows (Scoop) | `scoop install sccache` |
| Windows (Cargo) | `cargo install sccache` |

### 2.3 验证安装

```bash
sccache --version
```

## 3. 本地缓存配置

项目已在 `.cargo/config.toml` 中配置 sccache wrapper：

```toml
[build]
rustc-wrapper = "sccache"

[env]
SCCACHE_CACHE_SIZE = "10G"
```

安装 sccache 后，`cargo build` 自动经过缓存层，无需额外操作。

### 3.1 自定义缓存目录

取消 `.cargo/config.toml` 中 `SCCACHE_DIR` 的注释并修改路径：

```toml
[env]
SCCACHE_DIR = { value = "/custom/path/sccache", relative = false }
SCCACHE_CACHE_SIZE = "10G"
```

### 3.2 缓存大小调整

修改 `SCCACHE_CACHE_SIZE` 值（支持 `K`/`M`/`G`/`T` 后缀）：

```toml
SCCACHE_CACHE_SIZE = "20G"  # 20GB 缓存
```

缓存空间不足时，sccache 自动按 LRU 策略淘汰旧缓存。

## 4. 远程缓存配置

团队共享缓存可显著提升 CI 和新成员首次编译速度。

### 4.1 启用远程缓存

取消 `.cargo/config.toml` 中 `SCCACHE_REMOTE_SERVICE` 的注释：

```toml
[env]
SCCACHE_REMOTE_SERVICE = { value = "https://sccache.example.com", force = false }
```

### 4.2 TLS 加密配置

远程缓存启用 TLS 加密确保传输安全：

```bash
# 设置 TLS 证书（环境变量方式）
export SCCACHE_REMOTE_SERVICE="https://sccache.example.com"
export SCCACHE_REMOTE_TLS_CA_CERT="/path/to/ca-cert.pem"
export SCCACHE_REMOTE_TLS_CLIENT_CERT="/path/to/client-cert.pem"
export SCCACHE_REMOTE_TLS_CLIENT_KEY="/path/to/client-key.pem"
```

或使用 JWT 认证：

```bash
export SCCACHE_REMOTE_SERVICE="https://sccache.example.com"
export SCCACHE_REMOTE_TOKEN="your-jwt-token"
```

### 4.3 支持的远程后端

| 后端 | 环境变量 | 说明 |
|------|---------|------|
| S3 | `SCCACHE_REMOTE_SERVICE="s3://bucket"` | AWS S3 兼容存储 |
| GCS | `SCCACHE_REMOTE_SERVICE="gcs://bucket"` | Google Cloud Storage |
| Azure | `SCCACHE_REMOTE_SERVICE="azure://container"` | Azure Blob Storage |
| Redis | `SCCACHE_REMOTE_SERVICE="redis://host:port"` | Redis 缓存 |

## 5. 降级策略

### 5.1 sccache 未安装

`cargo build` 会因找不到 wrapper 而报错。临时禁用方式：

```bash
RUSTC_WRAPPER="" cargo build
```

或在 `.cargo/config.toml` 中注释掉 `rustc-wrapper` 行。

### 5.2 sccache 不可达（远程缓存）

sccache 自动降级为本地缓存或直接编译，不影响构建正确性。日志中会出现 warning 提示。

### 5.3 缓存空间不足

sccache 按 LRU 策略淘汰最久未使用的缓存条目，继续写入新缓存，无需手动干预。

## 6. 缓存统计

### 6.1 查看统计

```bash
bash scripts/sccache-stats.sh
```

输出示例：

```
┌─────────────────────────────────────┐
│       sccache 缓存统计报告          │
├─────────────────────────────────────┤
│  总请求数      :        150        │
│  缓存命中      :        120        │
│  缓存未命中    :         30        │
│  命中率        :     80.0%       │
└─────────────────────────────────────┘
✅ 命中率 80.0% ≥ 70%（达标）
```

### 6.2 JSON 格式输出（CI 用）

```bash
bash scripts/sccache-stats.sh --json
```

### 6.3 重置统计

```bash
bash scripts/sccache-stats.sh --reset
```

### 6.4 验证命中率

连续两次全量编译，第二次应命中缓存：

```bash
sccache --zero-stats
cargo clean && cargo build    # 第一次：全部未命中
cargo clean && cargo build    # 第二次：应 ≥ 70% 命中
bash scripts/sccache-stats.sh
```

## 7. CI 环境配置

### 7.1 GitHub Actions

```yaml
- name: Install sccache
  run: cargo install sccache

- name: Configure sccache
  run: |
    echo "SCCACHE_CACHE_SIZE=10G" >> $GITHUB_ENV

- name: Build with sccache
  run: cargo build --profile ci

- name: Show sccache stats
  run: bash scripts/sccache-stats.sh
```

### 7.2 Rust 环境变量

CI 环境中建议设置：

```bash
export RUSTC_WRAPPER=sccache
export SCCACHE_CACHE_SIZE=10G
```

## 8. 故障排查

| 问题 | 原因 | 解决方案 |
|------|------|---------|
| `cargo build` 报错找不到 sccache | sccache 未安装 | 安装 sccache 或 `RUSTC_WRAPPER="" cargo build` |
| 命中率始终为 0% | 缓存目录无写入权限 | 检查 `SCCACHE_DIR` 权限或使用默认目录 |
| 远程缓存连接超时 | 网络问题或服务不可达 | 检查 `SCCACHE_REMOTE_SERVICE` URL 和 TLS 证书 |
| 编译速度未提升 | 首次编译无缓存可命中 | 需第二次编译才能体现缓存效果 |