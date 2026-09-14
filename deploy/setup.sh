#!/bin/bash
# sz300-server 可观测性栈 - 一键部署脚本
# 用法: bash setup.sh
set -e

echo "================================================"
echo " sz300-server 可观测性栈部署"
echo "================================================"

# 检查 Docker
if ! command -v docker &>/dev/null; then
  echo "❌ Docker 未安装。请先安装 Docker："
  echo "   curl -fsSL https://get.docker.com | bash"
  exit 1
fi

# 检查 docker compose
if ! docker compose version &>/dev/null && ! docker-compose version &>/dev/null; then
  echo "❌ docker compose 不可用。请安装 Docker Compose v2："
  echo "   apt install docker-compose-plugin"
  exit 1
fi
echo "✅ Docker 已安装"

# 检查 sz300-server（提示开机启动）
echo ""
echo "⚠️  确保 sz300-server 已启动并监听 8300 端口"
echo "   启动命令示例："
echo "     cd ~/sz-rust && cargo run --release -p sz-rust-sz300"
echo "     或使用 systemd 服务："
echo "     sudo systemctl start sz300-server"
echo ""

# 创建监控配置目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# 检查 Grafana 面板文件
if [ -f "grafana/sz300-server-overview.json" ]; then
  echo "✅ Grafana 面板文件存在"
else
  echo "⚠️  Grafana 面板文件不存在，跳过导入"
fi

# 启动
echo ""
echo "================================================"
echo " 启动 Prometheus + Grafana + Alertmanager..."
echo "================================================"

docker compose up -d

echo ""
echo "================================================"
echo " 部署完成！"
echo "================================================"
echo ""
echo "访问地址："
echo "  Grafana:    http://$(curl -s ifconfig.me):3000  (admin/admin)"
echo "  Prometheus: http://$(curl -s ifconfig.me):9090"
echo "  Alertmanager: http://$(curl -s ifconfig.me):9093"
echo ""
echo "首次使用："
echo "  1. 打开 Grafana → Dashboards → sz300-server-overview"
echo "  2. 如未自动加载，手动 Import: Grafana → Dashboards → New → Import"
echo "     → Upload sz300-server-overview.json"
echo ""
echo "常用命令："
echo "  make logs     # 查看日志"
echo "  make ps       # 查看状态"
echo "  make down     # 停止"
echo "  make restart  # 重启"
echo ""
