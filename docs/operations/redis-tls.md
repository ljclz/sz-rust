# Redis TLS 配置指南

## 概述

sz300 v1.0 支持 Redis TLS 加密连接，生产环境强制启用。

## 环境变量

| 变量 | 说明 | 默认 |
|------|------|------|
| `SZ300_REDIS_CA_CERT_PATH` | CA 证书路径（PEM） | - |
| `SZ300_REDIS_CLIENT_CERT_PATH` | 客户端证书（mTLS） | - |
| `SZ300_REDIS_CLIENT_KEY_PATH` | 客户端私钥（mTLS） | - |
| `SZ300_REDIS_SNI` | SNI 主机名 | - |
| `SZ300_REDIS_ACCEPT_INVALID_CERT` | 接受无效证书 | false |

## URL 协议

- `redis://` — 明文连接
- `rediss://` — TLS 连接（自动启用）

## 生产环境校验

`SZ_ENV=production` + 明文连接 → 拒绝启动（`RedisTlsRequired`）

## 证书生成

```bash
# 生成 CA 私钥和证书
openssl genrsa -out ca-key.pem 4096
openssl req -new -x509 -key ca-key.pem -out ca-cert.pem -days 3650

# 生成 Redis 服务端证书
openssl genrsa -out redis-key.pem 2048
openssl req -new -key redis-key.pem -out redis.csr
openssl x509 -req -in redis.csr -CA ca-cert.pem -CAkey ca-key.pem -CAcreateserial -out redis-cert.pem -days 365
```

## 配置示例

```bash
export SZ300_REDIS_CA_CERT_PATH="/etc/ssl/redis/ca-cert.pem"
export SZ300_REDIS_SNI="redis.example.com"
```