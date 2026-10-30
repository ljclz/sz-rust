# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2024-2026 SZ-Rust Team
#
# SZ-Rust 插件市场多阶段构建 Dockerfile
#
# 阶段 1：builder — 编译 Rust 二进制
# 阶段 2：runtime — distroless 基础镜像

# ── 阶段 1：编译 ──
FROM rust:1.81-bookworm AS builder

WORKDIR /build

# 复制 workspace 根 Cargo.toml + Cargo.lock
COPY Cargo.toml Cargo.lock ./

# 复制所有 crate 的 Cargo.toml（依赖图）
COPY packages/ packages/

# 创建空 lib.rs 占位（避免 cargo build 下载依赖时失败）
RUN find packages/ -name "Cargo.toml" -execdir sh -c ' \
    if [ -f "Cargo.toml" ]; then \
        lib_name=$(grep "^name" Cargo.toml | head -1 | sed "s/.*= *\"//;s/\".*//"); \
        if [ ! -f "src/lib.rs" ] && [ ! -f "src/main.rs" ]; then \
            mkdir -p src && echo "" > src/lib.rs; \
        fi; \
    fi' \; 2>/dev/null || true

# 编译 marketplace crate
RUN cargo build --release -p sz-rust-marketplace || true

# 复制实际源码
COPY packages/ packages/

# 重新编译（使用实际源码）
RUN cargo build --release -p sz-rust-marketplace

# ── 阶段 2：运行时 ──
FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=builder /build/target/release/sz_rust_marketplace /app/marketplace

EXPOSE 8080

ENTRYPOINT ["/app/marketplace"]