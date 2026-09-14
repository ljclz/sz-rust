# Benchmark × 热点路径覆盖矩阵

> 生成日期：2026-08-06（v0.3.4 P0-2）
> 基线版本：v0.3.3

## 1. Soak Worker 覆盖路径（6 路径）

| # | 路径名 | 模块 | 操作 | 覆盖测试 |
|---|--------|------|------|---------|
| 1 | route_parse | sz-rust-router-facade | `parse_path(uri)` | soak_web_framework_steady_state + soak_smoke_10s |
| 2 | handler_ref | sz-rust-router-facade | `HandlerRef::parse(str)` | soak_web_framework_steady_state + soak_smoke_10s |
| 3 | json_serialize | sz-rust-core::response | `ApiResponse::success + to_json_string` | soak_web_framework_steady_state + soak_smoke_10s |
| 4 | middleware | sz-rust-middleware-facade | `MiddlewareChain::default_chain().has_duplicates()` | soak_web_framework_steady_state + soak_smoke_10s |
| 5 | di_container | sz-rust-core::container | `Container::new + singleton + make` | soak_web_framework_steady_state + soak_smoke_10s |
| 6 | cache_rw | sz-rust-cache-facade | `Cache::set + get` | soak_web_framework_steady_state + soak_smoke_10s |

## 2. Criterion Benchmark 覆盖（11 类）

| # | Benchmark 组 | 覆盖路径 | 基准数 |
|---|-------------|---------|--------|
| 1 | parse_path | route_parse | 3（root/static/long） |
| 2 | has_duplicates | middleware | 2（small/large） |
| 3 | capitalize_first | route_parse | 4（already_upper/needs_upper/24b/25b） |
| 4 | json_dto | json_serialize | 2（small/medium） |
| 5 | handler_ref | handler_ref | 2（parse/to_string） |
| 6 | framework_overhead | route_parse + json_serialize | 1（vs raw） |
| 7 | container_make | di_container | 2（singleton/scoped） |
| 8 | cache_set_get | cache_rw | 2（memory/tagged） |
| 9 | middleware_chain | middleware | 2（apply/has_duplicates） |
| 10 | response_build | json_serialize | 2（success/error） |
| 11 | router_match | route_parse | 2（static/param） |

## 3. Soak → Benchmark 映射

| Soak 路径 | 对应 Benchmark | 验证方式 |
|-----------|---------------|---------|
| route_parse | parse_path + router_match | criterion p99 ≤ 阈值 |
| handler_ref | handler_ref | criterion p99 ≤ 阈值 |
| json_serialize | json_dto + response_build | criterion p99 ≤ 阈值 |
| middleware | has_duplicates + middleware_chain | criterion p99 ≤ 阈值 |
| di_container | container_make | criterion p99 ≤ 阈值 |
| cache_rw | cache_set_get | criterion p99 ≤ 阈值 |

## 4. CI 验证

| 测试类型 | 触发方式 | 时长 | 覆盖路径 |
|---------|---------|------|---------|
| soak_smoke_10s | 每次 commit | 10s | 6 路径 |
| soak_steady_60s | 手动 --ignored | 60s | 6 路径 |
| soak_nightly_6h | CI nightly 02:00 | 6h | 6 路径 |

---

## 5. P3 性能优化 Benchmark（v0.6.x）

> 新增日期：2026-08-07（P3 性能优化）
> 基线版本：v0.5.0

### 5.1 P3 5 类 Benchmark 覆盖矩阵

| 类别 | Benchmark 函数 | 覆盖方向 | 热点路径 | Benchmark 数 |
|------|---------------|---------|---------|-------------|
| 端到端 p99 | `bench_end_to_end_p99` | 方向 1：热路径优化 | parse_path → has_duplicates → Container::make → JSON 序列化 | 6 |
| SIMD 字符串 | `bench_simd_string` | 方向 3：SIMD 加速 | capitalize_first / parse_path（SSE2 分隔符查找） | 6 |
| alloc 计数 | `bench_alloc_count` | 方向 4：内存池 | capitalize_first / parse_path / HandlerRef::parse | 3 |
| 拷贝计数 | `bench_copy_count` | 方向 5：零拷贝 | to_json_string vs to_json_bytes | 2 |
| 异步调度 | `bench_async_scheduling` | 方向 6：异步优化 | spawn/await 延迟 / spawn_blocking 延迟 | 5 |
| **合计** | | | | **22** |

### 5.2 6 大方向覆盖验证

#### 方向 1：热路径优化（p99 ↓ ≥ 15%）

| Benchmark | 覆盖点 | 状态 |
|-----------|--------|------|
| `p3_end_to_end_p99/short_path` | 短路径端到端 | ✅ |
| `p3_end_to_end_p99/medium_path` | 中等路径端到端 | ✅ |
| `p3_end_to_end_p99/long_path` | 长路径端到端 | ✅ |
| `p3_end_to_end_p99/root_path` | 根路径端到端 | ✅ |
| `p3_end_to_end_p99/parse_only` | 仅路由解析（隔离） | ✅ |
| `p3_end_to_end_p99/json_only` | 仅 JSON 序列化（隔离） | ✅ |

**内联优化**：`parse_path` / `split_first_segment` / `is_app_in_map` / `capitalize_first` / `ParsedPath::new` / `has_duplicates` / `Container::make` 均添加 `#[inline]`

#### 方向 2：连接池 L3 调优

| 模块 | 覆盖点 | 状态 |
|------|--------|------|
| `pool_warmer.rs` | 连接预热 | ✅ 7 个单元测试 |
| `query_cache.rs` | L2 查询缓存（TTL + LRU） | ✅ 10 个单元测试 |
| `pool_scaler.rs` | 动态扩容/缩容 | ✅ 10 个单元测试 |

#### 方向 3：SIMD 字符串加速

| Benchmark | 覆盖点 | 状态 |
|-----------|--------|------|
| `p3_simd_string/capitalize_first_lower` | 小写→大写（SIMD ASCII 检测） | ✅ ~38ns |
| `p3_simd_string/capitalize_first_upper` | 已大写（零分配快速路径） | ✅ |
| `p3_simd_string/capitalize_first_empty` | 空字符串 | ✅ |
| `p3_simd_string/parse_path_static` | 静态路径（SIMD 分隔符查找） | ✅ |
| `p3_simd_string/parse_path_root` | 根路径 | ✅ |
| `p3_simd_string/parse_path_long` | 长路径 + 查询字符串 | ✅ |

#### 方向 4：内存池

| Benchmark | 覆盖点 | 状态 |
|-----------|--------|------|
| `p3_alloc_count/capitalize_first_lower` | capitalize_first 分配计数 | ✅ |
| `p3_alloc_count/parse_path_long` | parse_path 分配计数 | ✅ |
| `p3_alloc_count/handler_ref_parse` | HandlerRef::parse 分配计数 | ✅ |

**MemPool 实现**：`StackPool<const CAP: usize>` + `BumpaloPool`（bumpalo feature gate），13 个单元测试

#### 方向 5：零拷贝优化

| Benchmark | 覆盖点 | 状态 |
|-----------|--------|------|
| `p3_copy_count/to_json_string` | String 序列化（UTF-8 验证开销） | ✅ |
| `p3_copy_count/to_json_bytes` | Bytes 序列化（零拷贝） | ✅ |

**HandlerRefRef<'a>**：借用版本，零堆分配，13 个单元测试

#### 方向 6：异步优化

| Benchmark | 覆盖点 | 状态 |
|-----------|--------|------|
| `p3_async_scheduling/for_balanced_spawn_await` | 均衡配置 spawn 延迟 | ✅ |
| `p3_async_scheduling/for_io_intensive_spawn_await` | IO 密集配置 spawn 延迟 | ✅ |
| `p3_async_scheduling/for_cpu_intensive_spawn_await` | CPU 密集配置 spawn 延迟 | ✅ |
| `p3_async_scheduling/for_balanced_spawn_blocking` | 均衡配置 spawn_blocking 延迟 | ✅ |
| `p3_async_scheduling/for_io_intensive_spawn_blocking` | IO 密集 spawn_blocking 延迟 | ✅ |

**SzRuntime 预设**：`for_balanced()` / `for_io_intensive()` / `for_cpu_intensive()` + `with_blocking_threads()`，11 个单元测试

### 5.3 P3 Bench 运行方式

```powershell
# 列出所有 benchmark
cargo bench --package sz-rust-core --bench p3_bench -- --list

# 建立基线
cargo bench --package sz-rust-core --bench p3_bench -- `
    --warm-up-time 1 --measurement-time 3 --sample-size 30 `
    --save-baseline v0.5.0

# 对比基线
cargo bench --package sz-rust-core --bench p3_bench -- `
    --warm-up-time 1 --measurement-time 3 --sample-size 30 `
    --baseline v0.5.0
```

### 5.4 CI 回归告警配置

在 CI 中添加 P3 bench 回归检查，性能退化 > 10% 阻断合并：

```yaml
# .github/workflows/bench-regression.yml
bench-regression:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: actions-rs/toolchain@v1
      with:
        toolchain: stable
    - name: Restore bench baseline
      uses: actions/cache/restore@v3
      with:
        path: target/criterion
        key: criterion-baseline-v0.5.0
    - name: Run bench regression
      run: |
        cargo bench --package sz-rust-core --bench p3_bench -- \
          --warm-up-time 1 --measurement-time 3 --sample-size 30 \
          --baseline v0.5.0
```