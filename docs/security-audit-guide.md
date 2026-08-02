# AI 辅助安全审计指南

> **目标**：通过 AI 静态分析 + 工具链扫描 + 模糊测试，达到接近第三方安全审计的效果  
> **适用阶段**：CI/CD 自动审计 + 提交前手动审计  
> **审计频率**：每次提交自动扫描 + 每周全量审计

---

## 一、审计架构

```
┌─────────────────────────────────────────────────────────────┐
│                    AI 安全审计流水线                          │
├─────────────────────────────────────────────────────────────┤
│  Stage 1: 依赖安全                                           │
│  ├── cargo-audit（RustSec Advisory Database）               │
│  └── cargo-deny（许可证 + 来源审计）                          │
├─────────────────────────────────────────────────────────────┤
│  Stage 2: 代码安全                                           │
│  ├── cargo-geiger（unsafe code 统计）                        │
│  ├── AI 规则引擎（SQL 注入/XSS/CSRF 模式匹配）                │
│  └── clippy（安全相关 lint）                                  │
├─────────────────────────────────────────────────────────────┤
│  Stage 3: 模糊测试                                           │
│  ├── cargo-fuzz（基于 libfuzzer 的覆盖引导模糊测试）           │
│  └── AI 生成边界用例（空值/超长/特殊字符/Unicode）             │
├─────────────────────────────────────────────────────────────┤
│  Stage 4: 报告生成                                           │
│  ├── 风险等级分类（Critical/High/Medium/Low）                 │
│  └── 修复建议 + 代码定位                                      │
└─────────────────────────────────────────────────────────────┘
```

---

## 二、Stage 1：依赖安全

### 2.1 cargo-audit

扫描 `Cargo.lock` 中的已知漏洞（基于 RustSec Advisory Database）。

```bash
# 安装
cargo install cargo-audit

# 扫描
cargo audit

# 输出示例
#     Fetching advisory database from RustSec...
#     vulnerability found!
#     ID: RUSTSEC-2024-0XXX
#     Package: some-crate
#     Title: Buffer overflow in ...
#     Date: 2024-01-01
#     URL: https://rustsec.org/advisories/RUSTSEC-2024-0XXX
# Solution: Upgrade to version >= 1.2.3
```

### 2.2 CI 集成

```yaml
# .github/workflows/security.yml（已配置）
- name: Run cargo-audit
  run: cargo audit --deny-warnings
```

### 2.3 修复策略

| 漏洞级别 | 响应时间 | 操作 |
|---------|---------|------|
| Critical | 24h | 立即修复或降级 |
| High | 72h | 尽快升级依赖 |
| Medium | 1 week | 计划升级 |
| Low | 1 month | 跟踪观察 |

---

## 三、Stage 2：代码安全

### 3.1 cargo-geiger

统计项目中 unsafe code 使用情况。

```bash
# 安装
cargo install cargo-geiger

# 扫描
cargo geiger --include-tests --all-features

# 输出示例
# cargo-geiger statistics
# ========================
# Unsafe code found: 0.00% (0/148000 lines)
# 
# Files with unsafe code:
# (none — forbid(unsafe_code) enforced)
```

### 3.2 AI 规则引擎

通过 `.trae/skills/sz-rust-security-audit/SKILL.md` 定义的安全规则进行模式匹配。

**触发方式**：
```bash
# 提交前自动触发（preCommitCheck）
@sz-rust-security-audit

# 手动触发
@sz-rust-security-audit 执行全量安全审计
```

### 3.3 审计规则清单

| 规则 ID | 规则名称 | 检测方法 | 严重级别 |
|---------|---------|---------|---------|
| SEC-01 | SQL 注入 | `format!` + SQL 关键词模式匹配 | Critical |
| SEC-02 | XSS | 用户输入直接输出检测 | High |
| SEC-03 | CSRF | 状态修改操作无 token 检测 | High |
| SEC-04 | 敏感信息泄露 | 日志/响应中敏感字段检测 | High |
| SEC-05 | 认证绕过 | 敏感接口无 Auth 中间件检测 | Critical |
| SEC-06 | 文件上传漏洞 | 无类型/大小限制检测 | High |
| SEC-07 | 密码安全 | 弱哈希/明文检测 | Critical |
| SEC-08 | Rate Limit | 关键接口无限流检测 | Medium |
| SEC-09 | unsafe code | 非 forbid 的 unsafe 块 | High |

---

## 四、Stage 3：模糊测试

### 4.1 现有 Fuzz 测试

项目已有 fuzz 测试（`.github/workflows/fuzz.yml`），覆盖：

- `parse_path`：路由解析
- `HandlerRef`：处理器引用
- `route_config`：路由配置
- `ApiResponse`：响应序列化
- `ErrorCode`：错误码
- `AppConfig`：配置解析
- `Validate`：验证器

### 4.2 Fuzz 覆盖增强

```rust
// packages/sz-rust-core/tests/fuzz.rs

/// Fuzz: 请求参数解析（边界值 + 特殊字符）
#[test]
fn fuzz_fetch_post_data() {
    let iterations = std::env::var("FUZZ_ITERATIONS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1000);

    for i in 0..iterations {
        // 生成随机输入
        let input = generate_fuzz_input(i);
        
        // 执行解析（不应 panic）
        let _ = parse_post_data(&input);
    }
}

fn generate_fuzz_input(seed: u64) -> Vec<u8> {
    // AI 生成的边界用例
    let cases = [
        vec![],                    // 空输入
        vec![0; 1024 * 1024],     // 超大输入（1MB）
        b"{}\".to_vec(),          // 不完整 JSON
        b"\"\x00\"".to_vec(),     // 空字节
        b"\"\u{1F600}\"".to_vec(), // Emoji
        b"<script>alert(1)</script>".to_vec(), // XSS payload
        b"' OR 1=1 --".to_vec(),  // SQL 注入
    ];
    cases[(seed as usize) % cases.len()].to_vec()
}
```

### 4.3 AI 生成边界用例

```python
# scripts/fuzz_generator.py
"""AI 辅助生成模糊测试边界用例"""

BOUNDARY_CASES = {
    "empty": b"",
    "null_byte": b"\x00",
    "unicode": "😀🎉你好".encode("utf-8"),
    "sql_injection": [
        b"' OR 1=1 --",
        b"'; DROP TABLE users; --",
        b"1' AND '1'='1",
    ],
    "xss": [
        b"<script>alert(1)</script>",
        b"<img src=x onerror=alert(1)>",
        b"javascript:alert(1)",
    ],
    "path_traversal": [
        b"../../../etc/passwd",
        b"..\\..\\..\\windows\\system32\\config\\sam",
    ],
    "overflow": [
        b"A" * 10000,
        b"0" * 1000000,
    ],
}
```

---

## 五、Stage 4：报告生成

### 5.1 报告模板

```markdown
# 安全审计报告

**审计日期**：2026-08-02  
**审计范围**：全量代码 + 依赖 + 模糊测试  
**审计工具**：cargo-audit + cargo-geiger + AI 规则引擎 + cargo-fuzz

## 执行摘要

| 指标 | 值 |
|------|-----|
| 代码行数 | 148,000+ |
| 依赖数量 | 150+ |
| 审计用例 | 7 fuzz cases |
| 审计时长 | ~5 min |

## 漏洞统计

| 级别 | 数量 | 状态 |
|------|------|------|
| Critical | 0 | ✅ |
| High | 0 | ✅ |
| Medium | 0 | ✅ |
| Low | 0 | ✅ |

## 详细结果

### 依赖安全（cargo-audit）

| 漏洞 ID | 包名 | 级别 | 状态 |
|---------|------|------|------|
| — | — | — | ✅ 无已知漏洞 |

### 代码安全（AI 规则引擎）

| 规则 | 检查结果 | 详情 |
|------|---------|------|
| SEC-01 SQL 注入 | ✅ | 所有查询参数化绑定 |
| SEC-02 XSS | ✅ | 模板自动转义 |
| SEC-03 CSRF | ✅ | 双重提交 Cookie |
| SEC-04 敏感信息 | ✅ | skip_serializing + Debug 脱敏 |
| SEC-05 认证绕过 | ✅ | Auth 中间件覆盖 |
| SEC-06 文件上传 | ✅ | 白名单 + 大小限制 |
| SEC-07 密码安全 | ✅ | bcrypt + 自动 salt |
| SEC-08 Rate Limit | ✅ | 登录 5/5min + 短信 3/hour |
| SEC-09 unsafe code | ✅ | forbid 全覆盖 |

### 模糊测试（cargo-fuzz）

| 用例 | 迭代次数 | 崩溃数 | 状态 |
|------|---------|--------|------|
| parse_path | 10,000 | 0 | ✅ |
| fetch_post_data | 10,000 | 0 | ✅ |
| ApiResponse | 10,000 | 0 | ✅ |
| Validate | 10,000 | 0 | ✅ |

## 结论

**审计结果**：✅ 通过

**建议**：
1. 持续监控依赖漏洞（每周自动审计）
2. 新增代码需通过安全审计 Skill
3. 定期（每季度）进行第三方渗透测试

## 附录

- [cargo-audit 文档](https://docs.rs/cargo-audit)
- [RustSec Advisory Database](https://rustsec.org)
- [OWASP Top 10](https://owasp.org/www-project-top-ten/)
```

---

## 六、与第三方审计的对比

| 维度 | AI 辅助审计 | 第三方审计 |
|------|------------|-----------|
| **成本** | 低（自动化） | 高（5-20 万/次） |
| **频率** | 每次提交 | 年度/季度 |
| **覆盖范围** | 静态分析 + 模式匹配 | 静态 + 动态 + 渗透 |
| **业务逻辑** | 有限 | 深入 |
| **0-day 漏洞** | 无法检测 | 可能发现 |
| **合规认可** | 内部参考 | 外部认可 |

**建议**：AI 辅助审计作为日常防护手段，每年进行 1 次第三方审计作为合规背书。

---

## 七、快速开始

```bash
# 1. 安装工具
cargo install cargo-audit cargo-geiger

# 2. 运行审计
cargo audit          # 依赖漏洞
cargo geiger --include-tests  # unsafe code
@sz-rust-security-audit  # AI 规则引擎

# 3. 查看报告
cat target/security-report.md
```
