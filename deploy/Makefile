# sz300-server 部署快捷命令
.PHONY: up down restart logs ps prometheus grafana alertmanager loadtest

# 启动可观测性栈
up:
	docker compose up -d

# 停止
down:
	docker compose down

# 重启
restart: down up

# 查看日志
logs:
	docker compose logs -f

# 查看状态
ps:
	docker compose ps

# 打开 Prometheus 页面
prometheus:
	@echo "Prometheus: http://localhost:9090"

# 打开 Grafana 页面
grafana:
	@echo "Grafana: http://localhost:3000 (admin/admin)"

# 打开 Alertmanager 页面
alertmanager:
	@echo "Alertmanager: http://localhost:9093"

# 生产负载测试（需要先安装 k6）
loadtest:
	k6 run loadtest/loadtest.js
