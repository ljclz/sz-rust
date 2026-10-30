# ADR-0021: serde_json preserve_order feature 评估

> 状态：已决策
> 日期：2026-09-22
> 对应任务：T012

## 背景

`sz-rust` workspace 在 `Cargo.toml` 第 81 行全局启用 `serde_json` 的 `preserve_order` feature：

```toml
serde_json = { version = "1", features = ["preserve_order"] }
```

`preserve_order` 使 `serde_json::Value::Object` 内部使用 `indexmap::IndexMap`（保持插入顺序），而非默认的 `BTreeMap`（字母序）。`indexmap` 会增加内存开销和序列化/反序列化时间。

性能优化评估需要决定是否可以禁用 `preserve_order` 以消除 `indexmap` 开销。

## 评估

### preserve_order 影响范围

1. **`serde_json::Value::Object`**：键迭代顺序保持插入顺序（禁用后变为字母序）
2. **`#[derive(Serialize)]` 结构体**：字段顺序由结构体定义控制，**不受 preserve_order 影响**
3. **`utoipa` OpenAPI 文档**：生成的 schema 字段顺序可能受影响

### 项目使用情况

- `serde_json::Value` 在项目中广泛使用（486 处匹配），作为动态 JSON 数据传递
- 主要场景：capability args、API 响应解析、AI agent tool calls、workflow context
- `utoipa`（OpenAPI 文档生成）在 workspace 依赖中存在

### 禁用风险

1. **API 响应键顺序变化**：使用 `serde_json::Value::Object` 构建的动态 JSON 响应，键顺序会从插入顺序变为字母序，可能影响前端兼容性
2. **OpenAPI 文档键顺序变化**：utoipa 生成的 schema 字段顺序可能变化
3. **测试断言失败**：依赖 JSON 字符串精确匹配的测试可能因键顺序变化而失败

### 保留收益

1. 保持 API 响应键顺序稳定，无需前端兼容性审查
2. OpenAPI 文档字段顺序保持定义顺序
3. 测试无需修改

## 决策

**保留 `preserve_order` feature。**

### 理由

1. 项目大量使用 `serde_json::Value` 作为动态 JSON 数据（486 处），键顺序稳定性重要
2. `utoipa` OpenAPI 文档生成需要键顺序保持定义顺序
3. 禁用后需全面审查前端兼容性和测试断言，风险高、收益低
4. `indexmap` 开销在大多数场景下不是性能瓶颈（主要瓶颈在 I/O 和 DB 查询）
5. 若未来确认无键顺序依赖，可单独评估禁用

### 影响范围

- `Cargo.toml` 第 81 行保持不变
- 无代码修改
- 无测试修改

## 验收清单

- [x] 评估结论记录在 ADR 中（保留 + 理由）
- [x] 影响范围已分析（486 处 `serde_json::Value` 使用 + utoipa）
- [x] 风险已评估（API 响应键顺序 + OpenAPI 文档 + 测试断言）