---
name: sz-rust-error-handling
description: 错误处理检查 — 确保错误信息不泄露敏感数据、有正确的 HTTP 状态码。
tools: [cargo-clippy]
agentMode: auto
---

# 错误处理检查（sz-rust）

## 触发条件

- 新增或修改 `Result` 返回的函数
- 修改错误响应格式

## 检查步骤

1. 检查错误信息是否包含内部路径、SQL 语句等敏感信息
2. 确认 HTTP 状态码正确（4xx 客户端错误，5xx 服务端错误）
3. 确认 `unwrap()` 仅在测试代码中使用

## 通过标准

- 生产错误响应不包含内部实现细节
- 数据库错误转换为通用 500 响应
- 验证失败返回 400 + 结构化错误信息
- 未授权返回 401，无权限返回 403
- 无生产代码中的 `unwrap()` / `expect()`

## 错误响应格式

```json
{
  "code": 400,
  "msg": "参数验证失败",
  "data": {
    "fields": {
      "email": "格式不正确"
    }
  }
}
```
