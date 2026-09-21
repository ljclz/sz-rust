# sz300 覆盖率提升 phase2 — 交付记录

> 交付时间：2026-09-06（更新：2026-09-07）
> 交付范围：组6-9（lib 覆盖率度量 + 未覆盖清单 + 补充测试 + CI 配置 + 交付审查）
> 仓库：企业版 `E:\www\rust\sz-rust-enterprise`（sz300 业务应用）

## 1. 新增/修改文件清单

### 新增文件
| 文件 | 说明 |
|------|------|
| `packages/sz-rust-sz300/tests/cov_supplement_test.rs` | 40 个覆盖率补充测试用例 |
| `docs/audit/sz300-cov-phase2-uncovered.md` | 未覆盖路径清单 + 分类 |
| `docs/audit/2026-09-06-sz300-cov-phase2-delivery.md` | 本交付记录 |

### 修改文件
| 文件 | 说明 |
|------|------|
| `packages/sz-rust-sz300/tests/common/db_fixture.rs` | 修复 MySQL 容器等待条件、建表 SQL 注释跳过 bug、随机端口映射、连接重试 |
| `packages/sz-rust-sz300/tests/common/request.rs` | 新增 `make_authed_json_request_uri` 函数（JWT Bearer token + CSRF cookie） |
| `packages/sz-rust-sz300/tests/success_path_test.rs` | 修复请求 URI（使用正确路由路径） |

## 2. 测试用例统计

| 测试文件 | 用例数 | 状态 |
|----------|--------|------|
| cov_supplement_test.rs | 40 | 全部通过 |
| **总计** | **40** | **40 passed; 0 failed** |

验证命令：`cargo test -p sz-rust-sz300 --test cov_supplement_test --jobs 1`
输出：`test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`

## 3. 覆盖率度量结果（服务器实测）

### 度量环境
- 服务器：122.51.216.76（root，SSH 密钥认证）
- Docker：26.1.4
- Rust：1.97.1
- cargo-llvm-cov：0.9.0
- MySQL 镜像：mysql:9.6
- CARGO_TARGET_DIR：`/www/rust/sz-rust-enterprise-target`

### lib 视角覆盖率（--lib，仅 lib 单元测试）
- 度量命令：`cargo llvm-cov -p sz-rust-sz300 --lib --summary-only`
- 覆盖率：84.27% 行
- 未覆盖行：2681

### 含集成测试覆盖率（排除 main.rs，lib + 31 个集成测试目标）
- 度量命令：`cargo llvm-cov -p sz-rust-sz300 --summary-only --ignore-filename-regex "main.rs" --lib --test cov_supplement_test --test endpoint_coverage_test ...（31 个测试目标）`
- **覆盖率：84.91% 行 / 86.56% 分支 / 86.39% 函数**
- 未覆盖行：2578
- 总行数：17079

### 基线对比
| 指标 | phase1 基线 | phase2 实测 | 变化 |
|------|------------|------------|------|
| 行覆盖率 | 85.85%（含 Docker） | 84.91%（服务器实测） | -0.94% |
| 未覆盖行 | ~2420 | 2578 | +158 |

## 4. 代码审查结果

| 审查项 | 结果 | 证据 |
|--------|------|------|
| 无 `std::fs` | ✅ 通过 | `grep "std::fs" cov_supplement_test.rs` 无命中 |
| 无 `#![allow(dead_code)]` | ✅ 通过 | `grep "#![allow(dead_code)]" src/` 无命中 |
| 无 `SELECT *` | ✅ 通过 | `grep "SELECT *" src/` 仅注释命中（"禁 SELECT *"） |
| 无裸 `std::env::set_var` | ✅ 通过 | `grep "std::env::set_var" cov_supplement_test.rs` 无命中 |
| 无残留容器 | ✅ 通过 | 测试后清理 Docker 容器 |
| 无残留进程 | ✅ 通过 | 无子进程启动 |
| 无临时文件 | ✅ 通过 | 测试不落盘 |

## 5. 防幻影交付三件套证据

### 5.1 交付物路径
- `packages/sz-rust-sz300/tests/cov_supplement_test.rs` — 存在于企业版仓库
- `packages/sz-rust-sz300/tests/common/db_fixture.rs` — 存在于企业版仓库（已修复）
- `packages/sz-rust-sz300/tests/common/request.rs` — 存在于企业版仓库（已修复）
- `packages/sz-rust-sz300/tests/success_path_test.rs` — 存在于企业版仓库（已修复）
- `docs/audit/sz300-cov-phase2-uncovered.md` — 存在于开源版仓库
- `docs/audit/2026-09-06-sz300-cov-phase2-delivery.md` — 存在于开源版仓库

### 5.2 验证命令真实输出
```
$ cargo test -p sz-rust-sz300 --test cov_supplement_test --jobs 1
test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

$ cargo llvm-cov -p sz-rust-sz300 --summary-only --ignore-filename-regex "main.rs" --lib --test cov_supplement_test ...
TOTAL  17079  2578  84.91%  1615  217  86.56%  10363  1410  86.39%  0  0  -
```

### 5.3 变更标识
- 企业版仓库：本地修改（已上传到服务器 `/www/rust/sz-rust-enterprise`）
- 开源版仓库：待提交

## 6. 未达 90% 目标原因分析

**实测覆盖率 84.91%，目标 90%，差距 5.09%（约 869 行）**

1. **Docker 测试基础设施问题（已修复 6 项）**：
   - MySQL 容器等待条件：`message_on_stdout("ready for connections")` → `message_on_stderr("port: 3306")`（MySQL 9.6 的 "ready for connections" 输出到 stderr，且临时服务器 port:0 和最终服务器 port:3306 需区分）
   - 建表 SQL 注释跳过 bug：`trimmed.starts_with("--")` 跳过了以注释开头的整个 SQL 片段（含 CREATE TABLE）→ 先移除注释行再按 `;` 分割
   - 端口冲突：`with_mapped_port(3306, 3306)` 固定端口映射导致多测试目标冲突 → 移除固定映射，使用随机端口
   - 连接重试：`db::init_pool` 首次连接失败 → 添加 10 次重试（每次间隔 2s）
   - 请求 URI：`/` → `/api/v1/device/list` 等正确路由路径
   - 认证头：`X-User-Id`/`X-Username` → `Authorization: Bearer <JWT>` + `X-CSRF-Token` + `Cookie: csrf_token=...`

2. **仍存在的 Docker 测试问题**：
   - success_path_test 多测试卡住（OnceCell 共享容器可能死锁，单个测试通过但多测试一起运行卡住）
   - db_integration_test 等其他 Docker 测试未修复认证（需同样的 JWT + CSRF 修复）
   - bin_e2e_test 服务器启动超时（缺少环境变量 SZ300_JWT_SECRET 等）
   - cargo-llvm-cov `--failure-mode all` 不生效（需 nextest 后端，服务器未安装 cargo-nextest）

3. **主要未覆盖区域**（需 DB 连接的成功路径）：
   - controllers/order.rs：49.05% 行
   - controllers/ai.rs：47.70% 行
   - controllers/product.rs：61.10% 行
   - controllers/device.rs：68.32% 行
   - controllers/merchant.rs：68.34% 行
   - services/merchant_service.rs：63.86% 行
   - services/auth_service.rs：68.51% 行
   - services/product_service.rs：72.38% 行
   - services/mqtt_service.rs：76.03% 行
   - services/device_service.rs：80.15% 行
   - services/order_service.rs：82.02% 行

## 7. 达到 90% 目标的后续条件

1. **修复 OnceCell 共享容器死锁**：排查 success_path_test 多测试卡住原因（可能是 tokio::sync::OnceCell 在多测试间的初始化竞争）
2. **修复其他 Docker 测试认证**：对 db_integration_test、services_success_test、common_smoke_test、order_expire_handler_test 应用同样的 JWT + CSRF 修复
3. **修复 bin_e2e_test 环境变量**：设置 SZ300_JWT_SECRET、SZ300_DB_HOST 等环境变量
4. **安装 cargo-nextest**：使 `--failure-mode all` 生效，让 llvm-cov 在测试失败时也生成报告
5. **补充 mock 测试**：对无法通过 Docker 测试覆盖的路径，编写 mock 测试（可能无法覆盖 DB 成功路径）

## 8. CI 配置状态

- `COVERAGE_THRESHOLD: 85`（保持，附理由注释）
- bin 视角度量 step 已存在（`Run sz300 bin coverage`）
- `--include-ignored --test-threads=1` 已配置

## 9. Docker 基础设施修复详情

### 9.1 db_fixture.rs 修改
| 修改项 | 原代码 | 修复后 | 原因 |
|--------|--------|--------|------|
| 等待条件 | `message_on_stdout("ready for connections")` | `message_on_stderr("port: 3306")` | MySQL 9.6 日志输出到 stderr；需区分临时服务器(port:0)和最终服务器(port:3306) |
| 超时时间 | 60s | 120s | 容器启动 + 初始化可能需要更长时间 |
| 端口映射 | `with_mapped_port(3306, 3306)` | 移除（随机端口） | 多测试目标同时映射 3306 导致冲突 |
| 建表 SQL | `if trimmed.starts_with("--") { continue; }` | 先移除注释行再分割 | 注释行开头的片段含 CREATE TABLE 被错误跳过 |
| 连接重试 | 无 | 10 次重试（间隔 2s） | MySQL 容器启动后可能需要几秒才能接受连接 |
| JWT 初始化 | 无 | `init_auth("test-secret-key-for-coverage", ...)` | 认证中间件需要 JWT 密钥验证 token |

### 9.2 request.rs 修改
| 修改项 | 原代码 | 修复后 | 原因 |
|--------|--------|--------|------|
| 认证头 | `X-User-Id` + `X-Username` | `Authorization: Bearer <JWT>` | 认证中间件使用 JWT Bearer token |
| CSRF | 无 | `X-CSRF-Token` + `Cookie: csrf_token=...` | CSRF 中间件双提交 Cookie 校验 |
| URI | `/` | 参数化 URI | 测试需指定正确的路由路径 |

### 9.3 success_path_test.rs 修改
| 测试 | 原 URI | 修复后 URI |
|------|--------|-----------|
| device_list_returns_owned_devices | `/` | `/api/v1/device/list` |
| device_info_returns_detail | `/` | `/api/v1/device/info` |
| product_list_returns_owned_products | `/` | `/api/v1/product/list` |
| product_create_returns_good_id | `/` | `/api/v1/product/create` |
| order_list_returns_owned_orders | `/` | `/api/v1/order/list` |
| merchant_list_returns_data | `/` | `/api/v1/merchant/list` |
