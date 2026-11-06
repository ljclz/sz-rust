# =============================================================================
# SZ-Rust distroless 多阶段构建 Dockerfile
#
# 设计目标：
#   - Stage 1 (builder): rust:1.98-slim 编译 release 二进制
#   - Stage 2 (runtime): gcr.io/distroless/cc-debian12 极简运行环境
#   - 最终镜像仅包含二进制 + 配置文件，无 shell/包管理器，攻击面最小
#   - 使用 nonroot 用户运行（distroless 内置 UID 65532）
# =============================================================================
#
# 安全说明（2026-10-28 整改）：
#   - 基础镜像使用浮动 tag，生产构建应固定为具体 digest（sha256:...），
#     避免上游镜像更新导致构建不可复现 / 供应链漂移。具体 digest 需在发布时
#     通过 `docker pull` 后 `docker inspect --format='{{index .RepoDigests 0}}'`
#     或 registry API 确认，再替换下方 TODO。
#   - 本文件使用 `COPY . /build/sz-rust/` 复制整个构建上下文，敏感文件
#     （.env*、scripts/deploy/、scripts/ssh_key_temp、deploy/monitoring/ 等）
#     依赖根目录 .dockerignore 过滤。请勿显式 COPY 这些敏感路径，并保持
#     .dockerignore 与敏感路径清单同步（2026-10-28 已更新）。

# ---- Stage 1: 构建阶段 ----
# 使用 rust:1.98-slim 作为构建环境（slim 变体体积更小，仅需补充必要构建工具）
# TODO: 固定为具体 digest，例如 rust:1.98-slim@sha256:<digest>
FROM rust:1.98-slim AS builder

# 安装构建所需系统依赖
#   - ca-certificates: 支持 git over HTTPS
# 2026-11-06：改用 USTC 镜像源——deb.debian.org 直连不稳定（404/慢速，
# 本机 staging 实测 7 分钟才 530kB 且 trixie 索引 404）
RUN sed -i 's|deb.debian.org|mirrors.ustc.edu.cn|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# 复制项目源码
COPY . /build/sz-rust/

# sz-orm 依赖经 BuildKit additional context 引入（compose: additional_contexts.sz-orm），
# 不再容器内 git clone：① 依赖 GitHub 直连（受限网络不可复现）；② clone 的
# main HEAD 不受 .sz-orm-commit 钉住契约约束（可复现性门禁 P0-1）。使用方需
# 保证宿主机 ../sz-orm 检出在钉住提交上（dependency-reproducibility.py 门禁校验）。
COPY --from=sz-orm / /build/sz-orm

WORKDIR /build/sz-rust

# 生产安全 feature 构建参数（2026-10-25 架构评测修复）：
# 默认构建不启用 v1.8+ 安全 feature（与 Cargo.toml default=[] 一致）；
# 生产构建应传入 SZ300_FEATURES，例如：
#   docker build --build-arg SZ300_FEATURES="v18-security-headers,v18-data-mask,v18-rbac,v18-key-rotation,v18-audit-chain" .
ARG SZ300_FEATURES=""

# 编译 release 二进制（仅构建 sz300-server；传入 SZ300_FEATURES 时启用安全 feature）
RUN if [ -n "$SZ300_FEATURES" ]; then \
        cargo build --release -p sz-rust-sz300 --features "$SZ300_FEATURES"; \
    else \
        cargo build --release -p sz-rust-sz300; \
    fi

# ---- Stage 2: 健康检查二进制构建 ----
# 编译一个极简的 TCP 健康检查工具（distroless 无 curl/wget，需自带）
# TODO: 固定为具体 digest，例如 rust:1.98-slim@sha256:<digest>
FROM rust:1.98-slim AS healthcheck-builder
WORKDIR /hc
RUN sed -i 's|deb.debian.org|mirrors.ustc.edu.cn|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# 2026-11-05 修复（本机 staging 部署实测发现）：distroless 运行阶段 /app 归 root，
# nonroot(65532) 进程启动时 FileService::init() 的 create_dir_all("./uploads")
# 报 EACCES 直接退出——prod 模板此前从未真正启动过。在带 shell 的构建阶段
# 预建 uploads 目录并赋予 nonroot 属主，运行阶段 COPY --chown 带入。
# COPY --chown 在 distroless 内无需 shell，属主信息随层带入。
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
RUN rustc --edition 2021 -O -o /hc/healthcheck /hc/main.rs \
    && mkdir -p /hc/uploads && chown -R 65532:65532 /hc/uploads

# ---- Stage 3: 运行阶段 ----
# 使用 distroless cc-debian12（含 glibc，适配动态链接的 Rust 二进制）
# 不含 shell / 包管理器，镜像体积与攻击面最小化
# TODO: 固定为具体 digest，例如 gcr.io/distroless/cc-debian12@sha256:<digest>
FROM gcr.io/distroless/cc-debian12

WORKDIR /app

# 仅复制编译产物与必要配置文件（最小化镜像体积与攻击面）
COPY --from=builder /build/sz-rust/target/release/sz300-server /app/sz300-server
COPY --from=builder /build/sz-rust/packages/sz-rust-sz300/config /app/config
COPY --from=healthcheck-builder /hc/healthcheck /app/healthcheck
# 2026-11-05：nonroot 可写的 uploads 目录（FileService::init 启动期要求）；
# compose 侧挂 uploads_data 命名卷，首挂载继承此处属主
COPY --from=healthcheck-builder --chown=65532:65532 /hc/uploads /app/uploads

# 使用 distroless 内置 nonroot 用户运行（UID/GID 65532），避免以 root 身份运行服务
USER nonroot:nonroot

# 暴露服务端口
EXPOSE 8300

# P1-CICD-02：健康检查（每 30s 探测，超时 3s，3 次失败后标记 unhealthy）
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD ["/app/healthcheck"]

ENTRYPOINT ["/app/sz300-server"]
