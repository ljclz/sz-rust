# 安全策略

## 报告漏洞

如果您发现安全漏洞，请**不要**在 GitHub Issue 中公开报告。

请通过以下方式私密报告：
1. 发送邮件至安全团队
2. 在邮件中描述漏洞细节、影响范围和复现步骤
3. 如果可能，提供修复建议

## 响应 SLA

| 级别 | 响应时间 | 修复时间 |
|------|---------|---------|
| 严重（RCE / SQL 注入 / 认证绕过） | 24 小时 | 7 天 |
| 高（路径遍历 / 信息泄露 / 权限提升） | 48 小时 | 14 天 |
| 中（配置错误 / 缺少审计日志） | 72 小时 | 30 天 |
| 低（最佳实践建议） | 1 周 | 下个迭代 |

## 安全措施

- 全代码库 `#![forbid(unsafe_code)]`
- 密钥从环境变量读取，不硬编码
- JWT 使用 HS256 + constant-time 比较
- 密码使用 bcrypt 哈希
- cargo-deny 依赖审计（advisories + licenses + bans + sources）
- cargo-audit 安全漏洞扫描
- OpenSSL 强制禁止（走 rustls 路线）

## 已知忽略项

以下 RUSTSEC advisory 被忽略（均为 unmaintained/archived，非可利用漏洞）：

| Advisory | 包 | 原因 |
|----------|---|------|
| RUSTSEC-2026-0192 | ttf-parser | unmaintained，通过 ab_glyph 间接依赖 |
| RUSTSEC-2025-0134 | rustls-pemfile | archived，功能已合入 rustls |
| RUSTSEC-2024-0436 | paste | archived，无替代品 |
