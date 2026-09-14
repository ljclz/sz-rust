# 模块清理与插件接线任务清单

> **生成日期**：2026-08-14
> **基于审计**：docs/audit/2026-08-14-深度模块功能审计报告.md
> **产品定位**：sz-rust = 通用 Web 框架；sz300 = 鲜视达 IoT 设备+电商后端；sz-pay = 支付中台

---

## 关键发现

6 个插件（cms/crm/ecommerce/erp/forum/im）**均已有 `register_routes` 函数**（编译期静态注册接口已存在）：
- `addons-ecommerce/src/lib.rs:69: pub fn register_routes<S>(builder, state) -> builder`
- `addons-cms/src/lib.rs:62`、`addons-crm/src/lib.rs:97`、`addons-erp/src/lib.rs:71`、`addons-forum/src/lib.rs:51`、`addons-im/src/lib.rs:50`

**结论**：不是架构缺陷，是 sz300 接线缺失。修复 = sz300 调用 register_routes。

---

## 任务 1（高优先级）：sz300 接入 ecommerce 插件

**目标**：sz300 生产路径可达 `/api/ecommerce/*` 路由

**步骤**：
1. sz300 `Cargo.toml` 添加 `sz-rust-addons-ecommerce.workspace = true`
2. sz300 `router.rs` 在 `create_router` 中调用 `sz_rust_addons_ecommerce::register_routes`
3. sz300 `state.rs` 添加 `EcommerceState` 到 `AppState`
4. 添加端到端测试：`/api/ecommerce/orders` CRUD 可达

**验证**：
- `cargo check -p sz-rust-sz300`
- `cargo test -p sz-rust-sz300`
- `grep -n "ecommerce" packages/sz-rust-sz300/src/router.rs`

**影响范围**：sz300 Cargo.toml + router.rs + state.rs + 新测试文件

---

## 任务 2（中优先级）：移除 operator/wasm

**目标**：移除非框架核心、零测试、零依赖方的 crate

**步骤**：
1. `Cargo.toml` workspace members 移除 `sz-rust-operator`（wasm 已非 member）
2. 删除 `packages/sz-rust-operator/` 目录
3. 删除 `packages/sz-rust-wasm/` 目录
4. 更新 README.md / README.en.md 项目结构（移除对应行）
5. 更新 `scripts/audit/doc-code-consistency.js` 的 NON_CRATE_NAMES（如有引用）

**验证**：
- `cargo check --workspace`
- `node scripts/audit/doc-code-consistency.js`（退出码 0）
- `ls packages/ | grep -E "operator|wasm"`（空）

**影响范围**：Cargo.toml + 删除 2 目录 + README.md/README.en.md

---

## 任务 3（中优先级）：sz300 接入 rag

**目标**：sz300 `ai::chat` 增加行业 RAG 检索

**步骤**：
1. sz300 `Cargo.toml` 添加 `sz-rust-rag.workspace = true`
2. `controllers/ai.rs` 在 `chat` 中：先 RAG 检索行业术语 → 拼接到 prompt → 调用 `Ai::chat`
3. 添加测试：RAG 检索返回行业术语 + prompt 增强

**验证**：
- `cargo check -p sz-rust-sz300`
- `cargo test -p sz-rust-sz300`
- `grep -n "rag" packages/sz-rust-sz300/src/controllers/ai.rs`

**影响范围**：sz300 Cargo.toml + controllers/ai.rs + 新测试

---

## 任务 4（低优先级）：sz-pay 评估 workflow + pdf

**目标**：评估 sz-pay 是否需要工作流引擎和 PDF 导出

**步骤**：
1. 读取 sz-pay 迁移方案，确认订单流程复杂度
2. 评估是否需要 workflow（支付状态机 vs 直接状态管理）
3. 评估是否需要 pdf（对账单 Excel 导出）
4. 输出评估结论（不编码，仅建议）

**验证**：评估报告附 sz-pay 实际代码证据

**影响范围**：无代码变更，仅评估

---

## 执行顺序

1 → 2 → 3 → 4（每项完成后运行 `cargo test` 验证，再进入下一项）