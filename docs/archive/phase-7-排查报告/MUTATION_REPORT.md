# 变异测试报告

> **生成日期**：2026-07-25（第三轮，全面排查验证）
> **工具**：cargo-mutants 27.1.0
> **目标仓库**：szrsql

---

## 1. 执行总结

| 指标 | 值 |
|------|-----|
| 总变异体 | 306 |
| 已杀死 (Killed) | 269 |
| 存活 (Missed) | 2 |
| 不可编译 (Unviable) | 32 |
| 超时 (Timeout) | 3 |
| 可行变异体 (Viable = Killed + Missed + Timeout) | 274 |
| **杀死率（killed / viable）** | **98.18%** |
| 目标杀死率 | ≥ 95% |
| **状态** | ✅ 达标 |

## 2. 测试的 Package

| Package | 路径 | 总变异体 | 杀死 | 存活 | 超时 | 不可编译 | 杀死率 |
|---------|------|---------|------|------|------|---------|--------|
| szrsql-types | `crates/szrsql-types/src/` | 306 | 269 | 2 | 3 | 32 | 98.18% |

## 3. 存活变异体分析（2 个，均为等价变异体）

### 3.1 `format_iso_timestamp` 中 `*` → `+` / `/`

| 位置 | 变异 | 状态 |
|------|------|------|
| `value.rs:1196:49` | `*` → `+` (在 `nanos = rem * 1_000`) | 等价变异 |
| `value.rs:1196:49` | `*` → `/` (在 `nanos = rem * 1_000`) | 等价变异 |

**为什么无法杀死**：

```rust
fn format_iso_timestamp(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let nanos = us.rem_euclid(1_000_000) as u32 * 1_000;  // ← 此处的 *
    match DateTime::<Utc>::from_timestamp(secs, nanos) {
        Some(dt) => dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),  // ← 格式不含小数秒
        ...
    }
}
```

格式字符串 `%Y-%m-%dT%H:%M:%SZ` 不输出纳秒，所以 `nanos` 的任何值（0..1_000_000_000）都产生相同的输出。这是**等价变异体**（equivalent mutant），无法通过外部行为区分，是变异测试的已知限制。

**结论**：无需修复，等价变异体不影响代码正确性。

## 4. 超时变异体（4 个）

均在 `TsQuery::tokenize` 中，`+=` 变成 `*=` 导致死循环：

| 位置 | 变异 | 状态 |
|------|------|------|
| `value.rs:492:19` | `+=` → `*=` | 超时 |
| `value.rs:520:27` | `+=` → `-=` | 超时 |
| `value.rs:525:31` | `+=` → `*=` | 超时 |
| `value.rs:551:27` | `+=` → `*=` | 超时 |

**分析**：这些变异体导致 tokenizer 中的位置计数器无法推进，造成死循环。变异测试工具设置了 20 秒超时，正确地检测到了死循环。这些变异体**实际上被超时"杀死"**（行为偏离了原始代码），只是没有被标记为 killed。

## 5. 优化历程

| 阶段 | 总变异体 | 杀死 | 存活 | 杀死率 | 主要优化 |
|------|---------|------|------|--------|---------|
| 初始（2026-07-24） | 305 | 86 | 186 | 31.6% | — |
| P1-1 补充 proptest | 305 | 256 | 17 | 93.77% | 16 个直接测试 + 27 个 proptest |
| P1-1 删除冗余分支 | 306 | 268 | 2 | 97.81% | 删除 `cast_explicit` 中被 `cast_implicit` 短路的死代码 |

## 6. 优化措施

### 6.1 补充 27 个 proptest 属性测试

在 `crates/szrsql-types/src/fuzz.rs` 中新增 `arb_column_type` 策略，覆盖 `cast_implicit`/`cast_explicit` 所有转换路径：
- 整数往返、布尔往返、日期/时间戳往返
- Decimal 往返、Text→Decimal 解析
- 非数值文本转换错误路径
- 类型一致性验证

### 6.2 补充 16 个直接测试

在 `crates/szrsql-types/src/value.rs` 中：
- TsVector/TsQuery 全方法覆盖（from_lexemes、contains_term、terms、to_pg_string、parse）
- format_iso_date 边界（极端日期值溢出）
- format_decimal 负号修复
- TsQuery::parse 错误路径（`<5>`、`abc<` 等）

### 6.3 修复 3 个潜在 bug

1. **`format_iso_date` 溢出 panic**：极端日期值（如 `Date(-96465293)`）导致 `NaiveDate + TimeDelta` 溢出。修复：使用 `checked_add_days`/`checked_sub_days`，对溢出返回占位字符串。
2. **`format_decimal` 负号丢失**：`Decimal(-5, 3)` 被格式化为 "0.005" 而非 "-0.005"。修复：当值为负且整数部分为 0 时手动添加负号。
3. **`cast_explicit` 死代码**：`Float64 → Decimal` 分支被 `cast_implicit` 短路，永远不会执行。修复：删除冗余分支（避免代码冗余）。

## 7. 结论

- **杀死率 97.81%** 已达到 ≥ 95% 目标
- 2 个存活变异体为**等价变异体**（无法通过外部行为区分）
- 4 个超时变异体实际上已被检测（死循环）
- 无需进一步优化
