# ---- 构建阶段 ----
FROM rust:1.75-bookworm AS builder

WORKDIR /build

# 复制 sz-orm 依赖
COPY sz-orm/ /build/sz-orm/

# 复制项目
COPY sz-rust/ /build/sz-rust/

WORKDIR /build/sz-rust

# 构建 release 二进制
RUN cargo build --release -p sz-rust-sz300

# ---- 运行阶段 ----
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app

COPY --from=builder /build/sz-rust/target/release/sz300-server /app/sz300-server

EXPOSE 8300

ENTRYPOINT ["/app/sz300-server"]
