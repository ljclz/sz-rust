---
name: sz-rust-security-audit
description: AI 辅助安全审计，检测 SQL 注入、XSS、CSRF、敏感信息泄露、认证授权等漏洞。提交前自动触发，也可手动触发全量审计。
agentMode: auto
---

# sz-rust 安全审计 Skill

## 触发条件

- 修改涉及用户输入处理的代码
- 新增认证/授权逻辑
- 修改数据库查询
- 提交前自动触发（preCommitCheck）
- 手动触发：`@sz-rust-security-audit 执行全量安全审计`

## 审计范围

### 1. SQL 注入检测

```rust
// ❌ 错误：字符串拼接 SQL（P0-SEC-01）
let sql = format!("SELECT * FROM users WHERE name = '{}'", name);

// ✅ 正确：参数化查询
let sql = "SELECT * FROM users WHERE name = ?";
conn.query_with_params(sql, (name,)).await?;
```

**检查点**：
- [ ] 所有 SQL 查询使用参数化绑定（`?` 占位符）
- [ ] 无 `format!` / `+` 拼接 SQL 字符串
- [ ] WHERE 条件经过 `sz_orm_query` 参数化验证

### 2. XSS 检测

```rust
// ❌ 错误：直接输出用户输入
resp.body(user_input);

// ✅ 正确：模板引擎自动转义
ctrl.render_success("ok", json!({"name": escape_html(user_input)}));
```

**检查点**：
- [ ] 用户输入输出经过转义
- [ ] 模板引擎启用自动转义
- [ ] Content-Type 正确设置（`application/json` 或 `text/html; charset=utf-8`）

### 3. CSRF 检测

```rust
// ❌ 错误：状态修改操作无 CSRF 保护
POST /api/users/delete

// ✅ 正确：携带 CSRF token（双重提交 Cookie 模式）
POST /api/users/delete
Headers: X-CSRF-Token: <token>
Cookie: csrf_token=<token>; Secure; SameSite=Strict
```

**检查点**：
- [ ] 状态修改操作（POST/PUT/DELETE）有 CSRF 保护
- [ ] CSRF Cookie 包含 `Secure` + `SameSite=Strict` 标志
- [ ] CSRF Token 与 JWT 过期时间对齐（24h）

### 4. 敏感信息泄露检测

```rust
// ❌ 错误：日志输出密码（P1-SEC-12）
tracing::info!("login attempt: user={}, password={}", user, password);

// ✅ 正确：脱敏处理
tracing::info!("login attempt: user={}, password=***", user);

// ❌ 错误：响应包含敏感字段
json!({"password": user.password, "token": token})

// ✅ 正确：敏感字段脱敏
#[serde(skip_serializing)]
pub password: String,
```

**检查点**：
- [ ] 日志中无密码、token、密钥等敏感信息
- [ ] 响应中敏感字段有 `#[serde(skip_serializing)]`
- [ ] `JwtConfig` 手动实现 `Debug`（secret 脱敏为 `[REDACTED]`）
- [ ] 错误信息不包含内部实现细节（堆栈、路径、SQL）

### 5. 认证授权检测

```rust
// ❌ 错误：敏感接口无鉴权
pub async fn delete_user(State(state): State<AppState>, req: Request<Body>) -> Response {
    // 无 Auth 中间件
}

// ✅ 正确：鉴权中间件覆盖
pub async fn delete_user(
    State(state): State<AppState>,
    req: Request<Body>,
    _user: AuthenticatedUser, // Auth 中间件注入
) -> Response {
    // 已认证
}
```

**检查点**：
- [ ] 敏感接口有 Auth 中间件覆盖
- [ ] JWT 密钥非空时启动（`validate_jwt_config()`）
- [ ] JWT audience 验证（生产环境配置 `SZ_JWT_AUDIENCE`）
- [ ] JWT 黑名单支持（登出后 token 失效）
- [ ] 权限粒度合理（管理员/普通用户分离）

### 6. 文件上传检测

```rust
// ❌ 错误：无文件类型/大小限制
let files = handle_multipart(req).await?;

// ✅ 正确：白名单 + 大小限制
let files = handle_multipart_with_config(req, MultipartConfig {
    max_file_size: 10 * 1024 * 1024, // 10MB
    allowed_types: vec!["image/jpeg", "image/png", "image/gif"],
    allowed_extensions: vec![".jpg", ".jpeg", ".png", ".gif"],
}).await?;
```

**检查点**：
- [ ] 文件类型白名单（MIME + 扩展名）
- [ ] 文件大小限制
- [ ] 文件路径遍历防护（`../` 过滤）
- [ ] 文件名 sanitization（去除特殊字符）
- [ ] 存储目录不可直接访问

### 7. 密码安全检测

```rust
// ❌ 错误：明文存储 / 弱哈希
password: password, // 明文
md5(password),      // 弱哈希

// ✅ 正确：强哈希（bcrypt/argon2）
use sz_orm_auth::hash_password;
let hashed = hash_password(&password, None)?; // 自动 salt
```

**检查点**：
- [ ] 密码使用 bcrypt/argon2 哈希
- [ ] 盐值自动生成（不手动管理）
- [ ] 密码强度验证（长度 >= 8，复杂度）
- [ ] 密码不记录日志

### 8. Rate Limiting 检测

```rust
// ✅ 已实现：登录限流（防暴力破解）
let limiter = SlidingWindowRateLimiter::new(5, 300); // 5 次/5 分钟
```

**检查点**：
- [ ] 登录接口限流（<= 5 次/5min）
- [ ] 短信接口限流（<= 3 次/小时）
- [ ] 全站限流（防 DDoS）

## 通过标准

| 级别 | 要求 |
|------|------|
| **Critical** | 零 Critical 级别漏洞（SQL 注入、RCE、认证绕过） |
| **High** | 零 High 级别漏洞（XSS、CSRF、敏感信息泄露） |
| **Medium** | Medium 级别漏洞有明确的修复计划（记录在 CHANGELOG） |
| **Low** | forbid(unsafe_code) 全覆盖 |

## 执行命令

```bash
# 依赖漏洞扫描
cargo audit

# unsafe code 检测
cargo geiger --include-tests --all-features

# 模糊测试（CI 自动运行）
cargo test -p sz-rust-core --test fuzz

# AI 辅助审计（本 Skill）
@sz-rust-security-audit 执行全量安全审计
```

## 审计输出格式

```markdown
## 🔒 安全审计报告

| 检查项 | 状态 | 详情 |
|--------|------|------|
| SQL 注入 | ✅ | 所有查询参数化绑定 |
| XSS | ✅ | 模板自动转义 |
| CSRF | ✅ | 双重提交 Cookie + Secure + SameSite |
| 敏感信息 | ✅ | skip_serializing + Debug 脱敏 |
| 认证授权 | ✅ | Auth 中间件覆盖 |
| 文件上传 | ✅ | 白名单 + 大小限制 |
| 密码安全 | ✅ | bcrypt + 自动 salt |
| Rate Limit | ✅ | 登录 5/5min + 短信 3/hour |
| unsafe_code | ✅ | forbid 全覆盖 |
| 依赖漏洞 | ⚠️ | 无已知漏洞 |

**结论**：通过 ✅ / 不通过 ❌
**修复建议**：...
```
