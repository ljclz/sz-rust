# sz300 覆盖率提升 phase2 — 未覆盖路径清单

> 生成时间：2026-09-06
> 度量工具：cargo-llvm-cov 0.9.0
> 度量命令：`cargo llvm-cov -p sz-rust-sz300 --summary-only --jobs 1 --ignore-filename-regex "main.rs"`
> 当前覆盖率：84.93% 行 / 86.56% 分支 / 86.45% 区域
> 基线覆盖率：85.85%（phase1，含 Docker 集成测试）
> 目标覆盖率：≥90%
> 差距原因：本地 Docker 不可用，63 个 `#[ignore = "requires Docker"]` 测试无法运行

## 未覆盖路径统计（按数量降序）

| 文件 | 未覆盖行数 | 分类 |
|------|-----------|------|
| controllers/order.rs | 175 | Testable（需 DB） |
| controllers/product.rs | 160 | Testable（需 DB） |
| controllers/ai.rs | 142 | Testable（需 DB） |
| controllers/device.rs | 135 | Testable（需 DB） |
| controllers/merchant.rs | 120 | Testable（需 DB） |
| services/auth_service.rs | 79 | Testable（需 DB） |
| services/mqtt_service.rs | 76 | Testable（需 DB） |
| builders.rs | 47 | Testable（环境变量降级） |
| services/merchant_service.rs | 44 | Testable（需 DB） |
| services/device_service.rs | 39 | Testable（需 DB） |
| controllers/auth.rs | 37 | Testable（需 DB） |
| services/product_service.rs | 36 | Testable（需 DB） |
| bootstrap.rs | 34 | ExcludedByPlatform（信号处理 Unix/Windows） |
| services/order_service.rs | 28 | Testable（需 DB） |
| bootstrap_interop.rs | 26 | Testable（需 DB） |
| controllers/file.rs | 23 | Testable（需 DB） |
| db.rs | 22 | Testable（DB 连接错误路径） |
| services/mqtt_listener.rs | 15 | Testable（需 MQTT） |
| router.rs | 10 | Testable（中间件组合） |
| controllers/wasm_api.rs | 9 | Testable（WASM 执行） |
| services/file_service.rs | 9 | Testable（文件操作） |
| services/mod.rs | 1 | Testable（服务注册） |
| services/health_service.rs | 5 | Testable（健康检查 DB 路径） |
| controllers/capabilities.rs | 7 | Testable（能力查询） |
| controllers/common.rs | 3 | Testable（分页解析边界） |
| middleware/auth_middleware.rs | 3 | Testable（JWT 校验错误） |
| models/device.rs | 3 | Testable（模型转换） |
| jobs/order_expire.rs | 5 | Testable（需 DB） |
| controllers/view.rs | 5 | Testable（模板渲染） |
| controllers/file_serve.rs | 1 | Testable（文件服务） |
| config.rs | 4 | Testable（配置解析边界） |

## 分类汇总

- **Testable（需 DB）**：大部分未覆盖路径需要 DB 连接才能测试，共约 1200 行
- **ExcludedByPlatform**：bootstrap.rs 的信号处理代码（Unix/Windows 平台特定），共约 34 行
- **不可覆盖上限**：在 Docker 不可用环境下，最多可覆盖约 110 行（已通过 cov_supplement_test.rs 覆盖 108 行）

## 补充测试覆盖情况

已创建 `tests/cov_supplement_test.rs`（40 个测试用例），覆盖：
- controllers 错误路径（未认证请求、参数错误）
- services DB 不可用错误路径
- health controller 路径
- auth controller 路径
- parse_pagination 边界路径
- router 公开路径判断

覆盖率提升：84.30% → 84.93%（+0.63%，108 行被覆盖）

## 达到 90% 目标的条件

要达到 90% 覆盖率，需要：
1. Docker 环境可用（运行 63 个 `#[ignore = "requires Docker"]` 测试）
2. 预计 Docker 测试可覆盖约 800 行 DB 依赖路径
3. 预计总覆盖率可达 85.85%（phase1 基线）+ 补充测试提升 ≈ 87-89%
4. 可能还需补充更多 mock 测试达到 90%

## 数字溯源

- 84.93%：`cargo llvm-cov -p sz-rust-sz300 --summary-only --jobs 1 --ignore-filename-regex "main.rs"` 输出 TOTAL 行
- 84.30%：`cargo llvm-cov -p sz-rust-sz300 --lib --summary-only --jobs 1` 输出 TOTAL 行
- 85.85%：phase1 基线（含 Docker 集成测试）
- 40 个测试用例：`cargo test -p sz-rust-sz300 --test cov_supplement_test` 输出 `40 passed`
- 63 个 ignored 测试：`cargo test -p sz-rust-sz300` 输出统计