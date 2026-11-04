#!/bin/bash
set -euo pipefail

TARGET_VERSION="${1:-1.81.0}"
CHANNEL="${2:-stable}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "$SCRIPT_DIR/config-defaults.sh" 2>/dev/null || true
BACKUP_DIR="${DEFAULT_WORK_DIR:-/www/rust/sz-rust-soak}"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)

mkdir -p "$BACKUP_DIR"

echo "=== Rust 工具链升级脚本 ==="
echo "目标版本: $TARGET_VERSION"
echo "频道: $CHANNEL"
echo "备份目录: $BACKUP_DIR"

# 备份当前工具链信息
echo "--- 备份当前工具链 ---"
{
    echo "=== rustc/cargo 版本 ==="
    rustc --version 2>&1 || true
    cargo --version 2>&1 || true
    echo "=== rustup 状态 ==="
    rustup show 2>&1 || echo "rustup 未安装"
    echo "=== which ==="
    which rustc 2>&1 || true
    which cargo 2>&1 || true
    which rustup 2>&1 || true
} > "$BACKUP_DIR/rust-backup-$TIMESTAMP.txt"
echo "备份保存到: $BACKUP_DIR/rust-backup-$TIMESTAMP.txt"

# 检查并安装 rustup
if ! command -v rustup &>/dev/null; then
    echo "--- rustup 未安装，开始安装 rustup ---"
    # 安装依赖
    apt-get update -qq
    apt-get install -y -qq curl build-essential pkg-config libssl-dev

    # 通过 rustup 脚本安装
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none

    # 加载环境变量
    source "$HOME/.cargo/env"
    export PATH="$HOME/.cargo/bin:$PATH"

    if ! command -v rustup &>/dev/null; then
        echo "❌ rustup 安装失败"
        exit 20
    fi
    echo "✅ rustup 安装成功: $(rustup --version)"
fi

# 执行升级
echo "--- 开始安装 $CHANNEL 工具链 ---"
if rustup install "$CHANNEL"; then
    rustup default "$CHANNEL"
    ACTUAL_VERSION=$(rustc --version | grep -oP '\d+\.\d+\.\d+' | head -1)
    echo "实际安装版本: $ACTUAL_VERSION"

    # 版本比较
    ACTUAL_MAJOR=$(echo "$ACTUAL_VERSION" | cut -d. -f1)
    ACTUAL_MINOR=$(echo "$ACTUAL_VERSION" | cut -d. -f2)
    TARGET_MAJOR=$(echo "$TARGET_VERSION" | cut -d. -f1)
    TARGET_MINOR=$(echo "$TARGET_VERSION" | cut -d. -f2)

    if [ "$ACTUAL_MAJOR" -gt "$TARGET_MAJOR" ] || \
       ([ "$ACTUAL_MAJOR" -eq "$TARGET_MAJOR" ] && [ "$ACTUAL_MINOR" -ge "$TARGET_MINOR" ]); then
        echo "✅ 升级成功: rustc $ACTUAL_VERSION >= $TARGET_VERSION"
        rustc --version
        cargo --version
        exit 0
    else
        echo "❌ 升级失败: rustc $ACTUAL_VERSION < $TARGET_VERSION"
        exit 20
    fi
else
    echo "❌ rustup install 失败"
    exit 20
fi
