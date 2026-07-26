# syntax=docker/dockerfile:1.7
# =============================================================================
# Phase 4.5.5 — SzRSQL Dockerfile
# 多阶段构建：cargo build --release → debian:bookworm-slim 运行时镜像
# 目标：镜像 < 100MB，启动 < 1s，端口 5432 可连接
# =============================================================================
#
# 构建命令：
#   docker build -t szrsql:0.1.0 .
#
# 运行命令：
#   docker run -d --name szrsql \
#     -p 5432:5432 \
#     -v szrsql-data:/var/lib/szrsql \
#     szrsql:0.1.0
#
# 启用 HTTP 管理端点（可选）：
#   docker run -d --name szrsql \
#     -p 5432:5432 -p 8080:8080 \
#     -v szrsql-data:/var/lib/szrsql \
#     szrsql:0.1.0 --http-port 8080 --http-host 0.0.0.0
#
# 验证：
#   docker exec szrsql szrsql --version          # 输出版本号
#   docker exec szrsql ss -lntp | grep 5432      # 端口监听检查
# =============================================================================

# -----------------------------------------------------------------------------
# Stage 1: Builder — 编译 release 二进制
# -----------------------------------------------------------------------------
FROM rust:1-bookworm AS builder

# 安装构建所需系统依赖
# ca-certificates：cargo crates HTTPS 下载
# libc6：C 运行时（ring、crc32c 等 native crate 依赖）
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        libc6 \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# 先复制 Cargo.toml / Cargo.lock 与各 crate 的 Cargo.toml 以利用 Docker 层缓存
# 仅当依赖变化时才重新下载/编译依赖
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/

# 编译 release 二进制（szrsql 服务 + szrsql-health 健康检查）
# --locked：严格使用 Cargo.lock，CI 可复现
RUN cargo build --release --locked --bin szrsql --bin szrsql-health \
 && strip target/release/szrsql \
 && strip target/release/szrsql-health

# -----------------------------------------------------------------------------
# Stage 2: Runtime — 最小化运行时镜像
# -----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# 安装运行时所需最小依赖
# ca-certificates：rustls TLS 证书校验
# libgcc-s1：libstd 依赖的 unwind / __divti3 等 compiler-rt 符号
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates \
        libgcc-s1 \
 && rm -rf /var/lib/apt/lists/*

# 创建非 root 用户运行数据库服务（安全最佳实践）
# UID/GID 1000 是常见约定，避免与宿主机 root 冲突
RUN groupadd --system --gid 1000 szrsql \
 && useradd --system --uid 1000 --gid szrsql \
            --home-dir /var/lib/szrsql --shell /usr/sbin/nologin szrsql \
 && mkdir -p /var/lib/szrsql /var/run/szrsql /var/log/szrsql \
 && chown -R szrsql:szrsql /var/lib/szrsql /var/run/szrsql /var/log/szrsql

# 复制编译产物
COPY --from=builder /build/target/release/szrsql /usr/local/bin/szrsql
# Phase 7d.18：复制健康检查二进制（用于 Docker HEALTHCHECK）
COPY --from=builder /build/target/release/szrsql-health /usr/local/bin/szrsql-health

# 设置运行时环境变量
# RUST_LOG：tracing 日志级别（可被 -e RUST_LOG=debug 覆盖）
# RUST_BACKTRACE=1：panic 时输出 backtrace
# SZRSQL_HOST=0.0.0.0：默认监听所有接口（容器场景必需）
# SZRSQL_PORT=5432：默认 PG 端口
ENV RUST_LOG=info \
    RUST_BACKTRACE=1 \
    SZRSQL_HOST=0.0.0.0 \
    SZRSQL_PORT=5432

# 暴露 pgwire 端口
EXPOSE 5432

# 数据卷：数据库文件、PID 文件、崩溃日志
VOLUME ["/var/lib/szrsql"]

# 切换非 root 用户
USER szrsql

WORKDIR /var/lib/szrsql

# Phase 7d.18：健康检查
# 使用 szrsql-health 二进制进行 TCP 探针，验证 pgwire 端口 5432 可连接
# - 停止数据库 → TCP 连接失败 → exit 1 → unhealthy
# - 启动数据库 → TCP 连接成功 → exit 0 → healthy
# 生产环境可启用 --http-port 8080 + 使用 `szrsql-health --http --port 8080` 进行 HTTP 探针
HEALTHCHECK --interval=30s --timeout=5s --start-period=3s --retries=3 \
    CMD szrsql-health --host 127.0.0.1 --port 5432 --timeout 3 || exit 1

# 启动命令
# 默认监听 0.0.0.0:5432（容器场景必须 0.0.0.0，否则外部不可访问）
# CMD 可被 `docker run` 末尾的参数覆盖
ENTRYPOINT ["szrsql"]
CMD ["--host", "0.0.0.0", "--port", "5432"]
