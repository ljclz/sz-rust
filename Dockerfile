# =============================================================================
# SZ-Rust distroless 多阶段构建 Dockerfile
#
# 设计目标：
#   - Stage 1 (builder): rust:1.82-slim 编译 release 二进制
#   - Stage 2 (runtime): gcr.io/distroless/cc-debian12 极简运行环境
#   - 最终镜像仅包含二进制 + 配置文件，无 shell/包管理器，攻击面最小
#   - 使用 nonroot 用户运行（distroless 内置 UID 65532）
# =============================================================================

# ---- Stage 1: 构建阶段 ----
# 使用 rust:1.82-slim 作为构建环境（slim 变体体积更小，仅需补充必要构建工具）
FROM rust:1.82-slim AS builder

# 安装构建所需系统依赖
#   - git: clone sz-orm 依赖（workspace 通过 path 引用 ../sz-orm）
#   - pkg-config / libssl-dev: 兜底处理潜在的原生依赖链接
#   - ca-certificates: 支持 git over HTTPS
RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# 复制项目源码
COPY . /build/sz-rust/

# 显式 clone sz-orm 依赖（避免依赖 build context 之外的目录）
RUN git clone https://github.com/ljclz/sz-orm.git /build/sz-orm

WORKDIR /build/sz-rust

# 编译 release 二进制（仅构建 sz300-server）
RUN cargo build --release -p sz-rust-sz300

# ---- Stage 2: 健康检查二进制构建 ----
# 编译一个极简的 TCP 健康检查工具（distroless 无 curl/wget，需自带）
FROM rust:1.82-slim AS healthcheck-builder
WORKDIR /hc
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY <<'EOF' /hc/main.rs
use std::net::TcpStream;
use std::process;
fn main() {
    let addr = std::env::var("HEALTHCHECK_ADDR").unwrap_or_else(|_| "127.0.0.1:8300".to_string());
    match TcpStream::connect(&addr) {
        Ok(_) => process::exit(0),
        Err(e) => { eprintln!("healthcheck failed: {e}"); process::exit(1) }
    }
}
EOF
RUN rustc --edition 2021 -O -o /hc/healthcheck /hc/main.rs

# ---- Stage 3: 运行阶段 ----
# 使用 distroless cc-debian12（含 glibc，适配动态链接的 Rust 二进制）
# 不含 shell / 包管理器，镜像体积与攻击面最小化
FROM gcr.io/distroless/cc-debian12

WORKDIR /app

# 仅复制编译产物与必要配置文件（最小化镜像体积与攻击面）
COPY --from=builder /build/sz-rust/target/release/sz300-server /app/sz300-server
COPY --from=builder /build/sz-rust/packages/sz-rust-sz300/config /app/config
COPY --from=healthcheck-builder /hc/healthcheck /app/healthcheck

# 使用 distroless 内置 nonroot 用户运行（UID/GID 65532），避免以 root 身份运行服务
USER nonroot:nonroot

# 暴露服务端口
EXPOSE 8300

# P1-CICD-02：健康检查（每 30s 探测，超时 3s，3 次失败后标记 unhealthy）
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD ["/app/healthcheck"]

ENTRYPOINT ["/app/sz300-server"]
