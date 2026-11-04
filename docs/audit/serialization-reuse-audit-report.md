# 序列化复用审计报告

> 审计时间：2026-09-22
> 审计范围：`packages/sz-rust-core/src/` 和 `packages/sz-rust-mvc-facade/src/`
> 对应任务：T011

## 1. 审计方法

搜索 `serde_json::to_string` 和 `serde_json::to_vec` 调用，识别同一对象在单次请求中被多次 JSON 序列化的点。

## 2. 审计结果

### sz-rust-core/src/

| 文件 | 行号 | 调用 | 上下文 | 重复序列化？ |
|------|------|------|--------|-------------|
| `json/fast_small.rs` | 22 | `serde_json::to_vec(value)` | `fast_serialize_small` 函数实现 | 否（设计如此，单次序列化） |
| `json/fast_small.rs` | 41,63,80,90 | `serde_json::to_vec(...)` | 测试断言 | 否（测试代码） |
| `schema_cache.rs` | 628 | `serde_json::to_string(&schema)` | 测试代码 | 否（测试代码） |
| `runtime/hot_reload.rs` | 511,531 | `serde_json::to_string(...)` | 测试代码 | 否（测试代码） |
| `plugin/schema.rs` | 202 | `serde_json::to_string(&user)` | 测试代码 | 否（测试代码） |
| `pay.rs` | 1117 | `serde_json::to_string(&result)` | 测试代码 | 否（测试代码） |
| `json/simd_safe.rs` | 133 | `serde_json::to_string(&result)` | 测试代码 | 否（测试代码） |

### sz-rust-mvc-facade/src/

| 文件 | 行号 | 调用 | 上下文 | 重复序列化？ |
|------|------|------|--------|-------------|
| `view.rs` | 1035 | `serde_json::to_string(a)` | 模板渲染：`Value::Array` → 字符串 | 否（单次序列化，模板渲染路径） |
| `view.rs` | 1036 | `serde_json::to_string(o)` | 模板渲染：`Value::Object` → 字符串 | 否（单次序列化，模板渲染路径） |

## 3. 结论

**无重复序列化问题。** 审计范围内所有 `serde_json::to_string` / `serde_json::to_vec` 调用均为：

1. **测试代码**（`#[cfg(test)]` 或 `unwrap()` 测试断言）— 不影响生产性能
2. **单次序列化**（`fast_serialize_small` 实现、模板渲染）— 设计如此，无重复
3. **未发现**同一对象在单次请求内被多次序列化的模式

无需修改。序列化路径已是最优：每个对象在单次请求内最多序列化一次。

## 4. 验收清单

- [x] 审查 `sz-rust-core/src/` 和 `sz-rust-mvc-facade/src/` 中所有 `serde_json::to_string` / `serde_json::to_vec` 调用
- [x] 同一对象在单次请求内序列化次数 ≤1（无重复序列化点）
- [x] 审计报告已生成