# Prometheus 抓取配置

## Bearer Token 鉴权

sz300 v1.0 metrics 端点支持 Bearer token 鉴权。

### 服务端配置

```bash
# 环境变量
export SZ300_METRICS_BEARER_TOKEN="your-secret-token"
export SZ300_METRICS_AUTH_ENABLED=true
```

### Prometheus 配置

```yaml
scrape_configs:
  - job_name: 'sz300'
    metrics_path: /metrics
    bearer_token: "your-secret-token"
    static_configs:
      - targets: ['sz300-service:80']
```

## IP 白名单鉴权

```bash
# 环境变量
export SZ300_METRICS_ALLOWED_IPS="10.0.0.1,10.0.0.2,192.168.1.0/24"
```

### Prometheus 配置（IP 白名单）

```yaml
scrape_configs:
  - job_name: 'sz300'
    metrics_path: /metrics
    static_configs:
      - targets: ['sz300-service:80']
        labels:
          cluster: 'production'
```

## 双重鉴权

同时配置 Bearer token + IP 白名单，任一通过即放行。

## 生产环境强制鉴权

`SZ_ENV=production` 时，未配置任何鉴权 → 拒绝暴露 metrics 端点。