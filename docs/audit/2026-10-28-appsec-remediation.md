# 2026-10-28 AppSec/SecOps 只读评测整改报告

> 触发：`docs/audit/appsec-eval-prompt.md` 只读安全评测（10 维评分 5.2/10）。
> 整改范围：本会话内完成仓库侧全部可修复项；生产凭据轮换需人工执行（见文末）。
> 验证基线：`cargo test -p sz-rust-sz300 --lib` = **87 passed / 0 failed**（含 26 项 security_gauntlet）；`cargo test -p sz-rust-key-rotation` = 8 passed；`cargo check -p sz-rust-sz300` = 0 warning。

---

## 1. 凭据泄漏（P0-严重）— 已完成仓库侧脱敏，待人工轮换

**问题**：`scripts/` 下 68 个脚本内嵌真实 SSH 私钥；4 个脚本含生产 `SZ300_JWT_SECRET` / `SZ300_DB_PASSWORD` 明文；`deploy/monitoring/prometheus.yml` 硬编码 `prometheus-scrape-token`；`scripts/ssh_key_temp` 为私钥文件。

**修复**：
- 全部 68 个文件中的私钥块替换为 `process.env.SSH_PRIVATE_KEY`（环境变量注入，缺失即连接失败——失败关闭）。
- `scripts/verify_monitoring_deploy.js` 中的生产 JWT/DB 密码/metrics 令牌改为 `\${SZ300_JWT_SECRET:?}` / `\${SZ300_DB_PASSWORD:?}` / `\${PROMETHEUS_SCRAPE_TOKEN:?}`。
- `deploy/monitoring/prometheus.yml` 的 `bearer_token` 改为 `"${PROMETHEUS_SCRAPE_TOKEN}"`。
- 删除 `scripts/ssh_key_temp`。
- `.dockerignore` 新增 `.env*`、`**/*.key`、`**/*.pem`、`secrets.json`、`credentials.json`、`scripts/deploy/`、`scripts/ssh_key_temp`、`deploy/monitoring/`、`test_oracle_*.js`、`tmp_*.js`。
- `.gitignore` 新增 `ssh_key_temp`、`scripts/ssh_key_temp`。

**验证**：`grep -rl "BEGIN .*PRIVATE KEY" scripts/` = 0；`grep -rl "liujieclz_test_secret_key\|2sJ8BcTdSxWNN4fp\|prometheus-scrape-token" scripts/ deploy/` = 0；`grep -rl "process.env.SSH_PRIVATE_KEY" scripts/` = 68。

> ⚠️ **人工必办**：上述私钥与生产凭据已泄露于仓库历史/工作目录，必须在生产环境**立即轮换**：SSH 主机密钥对、MySQL 密码、JWT 签名密钥、metrics 抓取令牌。仓库侧删除不能撤销泄露。

## 2. key-rotation RNG（P0-高危）— 已修复

**问题**：`packages/sz-rust-key-rotation/src/manager.rs:78-93` 的 `RandomKeyGenerator` 使用时间种子 LCG（线性同余），JWT 签名密钥可被预测/暴力枚举；且被 sz300 生产初始化直接使用。

**修复**：`manager.rs` 改为 `rand::rngs::OsRng`（操作系统 CSPRNG）填充 32 字节；`Cargo.toml` 增加 `rand = { workspace = true }`；测试新增「两次生成密钥互不相同」。

**验证**：`cargo test -p sz-rust-key-rotation` = 8 passed / 0 failed。

## 3. IDOR / 水平越权（P0-高危）— 已修复

**问题**：`merchant::list/info`、`product::list/info`、`order::list/info`、`device::list/info` 等端点无数据归属过滤，任意登录用户可读/写其他商户数据。

**修复**：
- 新增 `CurrentUser` 请求上下文（`middleware/auth_middleware.rs`），JWT 校验通过后经 `try_get_user_info_by_id`（不 panic）注入 `user_id / username / merchant_id / is_admin`。
- `controllers/common.rs` 新增 `effective_merchant_scope_*` 与 `can_access_merchant_resource_*` 辅助函数。
- 商户/商品/订单/设备的 list 强制 `merchant_id` 归属过滤；info/update/delete/bind/unbind/OTA 校验资源归属；create 强制归属当前商户（非管理员）。管理员放行全部。

**验证**：`cargo check -p sz-rust-sz300` 通过；`cargo test -p sz-rust-sz300 --lib` 87 passed（含 gauntlet）。

## 4. refresh/logout 桩实现（P1-高）— 已修复

**问题**：`/api/v1/auth/refresh` 与 `/api/v1/auth/logout` 原为桩实现（不轮换、不吊销）。

**修复**：
- `auth_service::refresh_access_token`：调用 sz-orm 的 `JwtAuthenticator::refresh_token`（严格校验 `token_use=refresh`，防类型混淆），换发新 access token。
- `auth_service::revoke_token` / `is_token_revoked`：服务端内存黑名单（SHA-256 哈希存储 + 过期惰性清理）；`verify_token` 与 `verify_token_with_audience` 均检查黑名单。
- 登录响应新增 `refresh_token`；`logout` 解析 Authorization 头并吊销 token + 清除 CSRF Cookie。

## 5. WS/SSE 未认证（P1-高）— 已修复

**问题**：`/ws` 与 `/events` 在公开白名单且无连接认证。

**修复**：`ws_handler` / `sse_handler` 在升级/订阅前校验 JWT（`Authorization: Bearer` 或 `?token=` 查询参数），无效返回 401。

## 6. 模板 XSS（P1-中）— 已修复

**问题**：`/page/{template}` 将用户可控路径直接插入 HTML 且默认不转义。

**修复**：模板变量改为 `{$title|htmlspecialchars}` / `{$template|htmlspecialchars}`；错误页输出经 `html_escape` 转义。

## 7. 安全配置默认值（P1-高）— 已修复

- `panic_guard` 挂载生产路由最外层（`router.rs`）。
- JWT 密钥统一 `SZ300_JWT_SECRET`（启动强度校验 ≥32 字节，并同步 `SZ_JWT_SECRET` 供 addon 控制器）。
- `SZ300_COOKIE_SECURE` 环境变量可配置（默认 true）。
- sz300 `default` features 默认启用 `v18-security-headers`、`v18-data-mask`、`v18-audit-chain`、`v18-key-rotation`、`v19-jwt-audience`。
- `v19-jwt-audience` 默认开启：登录签发带 `aud=sz300-api` 的令牌；`deny_no_audience=false` 兼容历史无 aud 令牌（30 天宽限）。

## 8. 供应链治理（P1-中）— 已修复

- `deny.toml`：修正 protobuf（非 sz-orm 残留，真实经 prometheus 引入）、webpki-roots（非不存在，真实传递依赖）、zip（8.x 已在树中）三条不实理由。
- `.cargo/audit.toml`：修正 rkyv「幻影依赖」虚假理由；删除 Cargo.lock 中不存在的 serde_yml/libyml 无效 ignore；登记 ci.yml 额外忽略的 RUSTSEC-2026-0258/0370。
- `.github/workflows/security.yml`：unmaintained 检查补 clone `../sz-orm` 路径依赖；geiger 注释与行为对齐（非阻塞汇总）。
- `README.md` / `README.en.md`：miri（现为阻塞门禁）与 geiger 描述修正。

## 9. SecOps（P1-中）— 已修复

- `mqtt_service.rs` 落库 `operate_log.detail` 前调用 `redact_sensitive`（手机号/身份证掩码）。
- 生产部署强制 `SZ300_METRICS_TOKEN`（`docker-compose.prod.yml` 以 `:?` 要求）。

## 10. 部署与基础设施（P1-中）— 已修复

- `docker-compose.prod.yml`：数据库端口仅绑定 127.0.0.1；强制 `SZ300_METRICS_TOKEN` / `SZ300_CORS_ORIGIN`；注释明确 TLS 由前置反向代理终止。
- k8s 清单：统一命名空间（sz300）与服务名（sz300-service）；Deployment 显式设置 `SZ300_COOKIE_SECURE=true`。
- 监控栈：Grafana 管理员密码、Alertmanager SMTP/钉钉令牌、marketplace compose 凭据改为环境变量注入。
- Dockerfile 增加固定镜像 digest 的 TODO 注释。

---

## 遗留人工事项

1. **轮换生产凭据**（最高优先）：SSH 私钥、MySQL 密码、JWT 密钥、metrics 令牌、Grafana/Alertmanager 令牌。
2. `sz-rust-key-rotation` 密钥现存储于内存（进程重启后失效），多实例部署建议接入共享存储（Redis/KMS）——当前单实例示例可接受。
3. logout 黑名单为进程内存实现，多实例部署需替换为 Redis 等共享存储。
4. 上传端点仍无 magic-byte 内容校验（扩展名白名单 + 随机文件名已降低风险，可选增强）。