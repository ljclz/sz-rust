# JWT 密钥轮换运维指南

## 概述

sz300 v1.0 支持 JWT 签名密钥自动轮换，无需停机即可更新密钥。

## 环境变量

| 变量 | 说明 | 默认 |
|------|------|------|
| `SZ300_JWT_SECRET` | 当前签名密钥（必填） | - |
| `SZ300_JWT_ROTATION_INTERVAL` | 轮换间隔（秒） | 86400（24h） |
| `SZ300_JWT_GRACE_PERIOD` | 旧密钥宽限期（秒） | 3600（1h） |

## 工作原理

1. **签发**：始终使用 `current` 密钥签发新 token
2. **验证**：先用 `current` 密钥验证，失败则遍历 `previous` 列表
3. **轮换**：定时生成新密钥，旧 `current` 移入 `previous`（带过期时间）
4. **清理**：超过 `max_previous`（默认 3）或过期的旧密钥自动删除

## Grace Period

轮换后，旧密钥在 `grace_period` 内仍可验证 token。这确保：
- 轮换时已签发的 token 不会立即失效
- 客户端有足够时间获取新 token

## 密钥指纹

审计日志使用 SHA256 前 8 位指纹标识密钥，不泄露原值：

```
JWT_KEY_ROTATED: old_fingerprint=a1b2c3d4, new_fingerprint=e5f6g7h8
```

## 轮换失败处理

密钥生成失败时：
- 保留当前密钥，不中断服务
- 记录 `JWT_KEY_ROTATION_FAILED` 错误日志
- 下个轮换周期自动重试

## 配置示例

```bash
# 每 12 小时轮换，旧密钥保留 2 小时
export SZ300_JWT_SECRET="your-initial-secret"
export SZ300_JWT_ROTATION_INTERVAL=43200
export SZ300_JWT_GRACE_PERIOD=7200
```