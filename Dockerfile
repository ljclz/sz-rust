# ---- 构建阶段 ----
# 基础镜像 Rust 版本对齐 workspace rust-version = "1.81"
FROM rust:1.81-bookworm AS builder

WORKDIR /build

# 复制项目源码（sz-orm 由 release.yml 在 docker build 之前 clone 到 ../sz-orm，
# 并通过 build context 一并送入；此处 COPY 路径与 release.yml 的 context 配合）
COPY . /build/sz-rust/

# 显式 clone sz-orm 依赖（避免依赖 build context 之外的目录）
RUN git clone https://github.com/ljclz/sz-orm.git /build/sz-orm

WORKDIR /build/sz-rust

# 构建 release 二进制
RUN cargo build --release -p sz-rust-sz300

# ---- 运行阶段 ----
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app

COPY --from=builder /build/sz-rust/target/release/sz300-server /app/sz300-server

EXPOSE 8300

ENTRYPOINT ["/app/sz300-server"]
