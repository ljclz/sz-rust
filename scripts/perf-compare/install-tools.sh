#!/bin/bash
set -euo pipefail

TOOLS="${1:-both}"
K6_VERSION="v0.54.0"

echo "=== 压测工具安装脚本 ==="
echo "安装工具: $TOOLS"

INSTALL_WRK() {
    echo "--- 安装 wrk ---"
    if command -v wrk &>/dev/null; then
        echo "wrk 已安装: $(wrk --version)"
        return 0
    fi

    # 安装依赖
    apt-get update -qq
    apt-get install -y -qq build-essential libssl-dev git

    # 源码编译
    WRK_DIR="/tmp/wrk-build"
    rm -rf "$WRK_DIR"
    git clone --depth=1 https://github.com/wg/wrk.git "$WRK_DIR"
    cd "$WRK_DIR"
    make -j$(nproc)
    cp wrk /usr/local/bin/wrk
    cd /
    rm -rf "$WRK_DIR"

    # 验证
    if command -v wrk &>/dev/null; then
        echo "✅ wrk 安装成功: $(wrk --version)"
        return 0
    else
        echo "❌ wrk 源码编译失败，尝试 apt 安装..."
        apt-get install -y -qq wrk 2>/dev/null || true
        if command -v wrk &>/dev/null; then
            echo "✅ wrk apt 安装成功"
            return 0
        fi
        return 100
    fi
}

INSTALL_K6() {
    echo "--- 安装 k6 ---"
    if command -v k6 &>/dev/null; then
        echo "k6 已安装: $(k6 version)"
        return 0
    fi

    # GitHub release 二进制下载
    K6_URL="https://github.com/grafana/k6/releases/download/${K6_VERSION}/k6-${K6_VERSION}-linux-amd64.tar.gz"
    K6_TMP="/tmp/k6-download.tar.gz"
    
    if curl -sL "$K6_URL" -o "$K6_TMP"; then
        tar -xzf "$K6_TMP" -C /tmp/
        cp /tmp/k6-${K6_VERSION}-linux-amd64/k6 /usr/local/bin/k6
        rm -f "$K6_TMP"
        rm -rf /tmp/k6-${K6_VERSION}-linux-amd64
    else
        echo "二进制下载失败，尝试 docker 方式..."
        if command -v docker &>/dev/null; then
            # 创建 k6 wrapper 脚本
            cat > /usr/local/bin/k6 << 'EOF'
#!/bin/bash
docker run --rm -i grafana/k6:0.54.0 "$@"
EOF
            chmod +x /usr/local/bin/k6
        else
            echo "❌ k6 安装失败：无 docker 也无法下载二进制"
            return 100
        fi
    fi

    # 验证
    if command -v k6 &>/dev/null; then
        echo "✅ k6 安装成功: $(k6 version)"
        return 0
    else
        return 100
    fi
}

# 执行安装
RESULT=0

case "$TOOLS" in
    wrk)
        INSTALL_WRK || RESULT=100
        ;;
    k6)
        INSTALL_K6 || RESULT=100
        ;;
    both)
        INSTALL_WRK || RESULT=100
        INSTALL_K6 || RESULT=100
        ;;
    *)
        echo "❌ 未知工具: $TOOLS (支持: wrk/k6/both)"
        exit 100
        ;;
esac

echo "=== 安装完成 (退出码: $RESULT) ==="
exit $RESULT