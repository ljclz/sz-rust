# sz300 K8s 部署指南

## Helm 部署（推荐）

### 安装

```bash
# 默认配置
helm install sz300 deploy/k8s/helm/sz300/

# 生产环境
helm install sz300 deploy/k8s/helm/sz300/ -f deploy/k8s/helm/sz300/values-prod.yaml
```

### 更新

```bash
helm upgrade sz300 deploy/k8s/helm/sz300/ -f deploy/k8s/helm/sz300/values-prod.yaml
```

### 卸载

```bash
helm uninstall sz300
```

## 前置准备

### 创建 Secret

```bash
kubectl create secret generic sz300-secrets \
  --from-literal=jwt-secret=<your-jwt-secret> \
  --from-literal=db-password=<your-db-password>
```

### 创建 ConfigMap（可选）

```bash
kubectl create configmap sz300-server-config \
  --from-file=app.yaml=config/app.yml
```

## 探针配置

| 探针 | 端点 | initialDelay | period | timeout | failure |
|------|------|-------------|--------|---------|---------|
| liveness | /health | 10s | 30s | 3s | 3 |
| readiness | /health/ready | 5s | 10s | 2s | 3 |
| startup | /health/startup | - | 5s | - | 30 |

- liveness 仅检查进程存活，不检查依赖
- readiness 检查 DB（可配置 redis/mqtt）
- startup 允许 150s 启动时间（period × failure = 5 × 30）

## RUST_LOG 配置

| 环境 | RUST_LOG |
|------|----------|
| 开发 | `warn,sz_rust_sz300=info` |
| 生产 | `warn,sz_rust_sz300=info` |

生产环境禁止使用 `debug`/`trace` 级别。

## 资源限制

| 环境 | requests | limits |
|------|----------|--------|
| 默认 | 256Mi / 250m | 512Mi / 500m |
| 生产 | 512Mi / 500m | 1Gi / 1000m |

基于 v0.7.0 RSS 5~6MB 基线留余量。

## Prometheus 抓取

metrics 端点需鉴权（Bearer token 或 IP 白名单）：

```yaml
# Prometheus scrape config
scrape_configs:
  - job_name: 'sz300'
    bearer_token: '<your-metrics-bearer-token>'
    static_configs:
      - targets: ['sz300-service:80']
```