---
name: sz-rust-ci-cd
description: CI/CD 流水线检查 — 确保提交符合 CI 要求、Docker 构建可复现。修改 CI 配置或 Dockerfile 时触发。
tools: [docker, cargo]
agentMode: auto
---

# CI/CD 流水线检查（sz-rust）

## 触发条件

- 修改 `.github/workflows/` 中的 CI 配置
- 修改 `Dockerfile` 或 `docker-compose.yml`
- 新增 workspace 成员

## 检查步骤

### CI 配置
1. 确认 `cargo fmt --check` 通过
2. 确认 `cargo clippy --workspace` 无警告
3. 确认 `cargo test --workspace` 通过

### Docker
1. 确认 Dockerfile 使用多阶段构建
2. 确认基础镜像版本固定（非 `latest`）
3. 确认镜像可复现构建

## 通过标准

- CI 所有 job 通过
- Docker 镜像构建时间 <= 基线 + 20%
- 镜像层数 <= 20
- 无硬编码的密钥或 Token

## Dockerfile 最佳实践

```dockerfile
# 多阶段构建
FROM rust:1.81 AS builder
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/app /usr/local/bin/
```
