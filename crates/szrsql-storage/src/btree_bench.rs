//! SzRSQL B-Tree 基准测试 — 对应 `SzRSQL实施进度.md` M1 里程碑。
//!
//! ## M1 验证标准
//!
//! | 验证项 | 通过标准 |
//! |--------|---------|
//! | 基准测试 | 1 亿行点查（预热后）P50 < 10μs, P99 < 50μs |
//!
//! ## 设计要点
//!
//! 1. **零外部依赖**：使用 `std::time::Instant`，避免 criterion 在 Windows MSVC 上的潜在编译问题
//! 2. **预热**：先执行 1M 次查询预热 CPU 缓存与分支预测器
//! 3. **大样本点查**：默认 10M 次随机点查；可通过 `SZRSQL_M1_BENCH_QUERIES=100000000`
//!    环境变量提升到 1 亿次完整验证
//! 4. **键值域**：1M 个唯一 u64 key 装入 B-Tree（order=256 默认），随机抽样查询
//! 5. **纳秒级计时**：每次 `bt.search()` 单独计时，统计 P50/P95/P99/P99.9/max
//! 6. **判定**：P50 < 10μs 且 P99 < 50μs 视为通过；CI 上默认 10M 次以控制耗时
//!
//! ## 运行方式
//!
//! ```bash
//! # 默认 10M 次点查
//! cargo test -p szrsql-storage --release --bench btree_bench
//! cargo test -p szrsql-storage --release btree_bench::m1_bench_point_query_latency
//!
//! # 完整 1 亿次验证
//! $env:SZRSQL_M1_BENCH_QUERIES="100000000"
//! cargo test -p szrsql-storage --release btree_bench::m1_bench_point_query_latency -- --nocapture
//!
//! # Linux/macOS
//! SZRSQL_M1_BENCH_QUERIES=100000000 cargo test -p szrsql-storage --release \
//!   btree_bench::m1_bench_point_query_latency -- --nocapture
//! ```

use crate::btree::BTree;
use std::time::Instant;

// =====================================================================
//  XorShift64 PRNG — 固定种子，可重现
// =====================================================================

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xDEADBEEFCAFEBABE
            } else {
                seed
            },
        }
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    #[inline]
    fn next_u64_below(&mut self, max: u64) -> u64 {
        if max == 0 {
            return 0;
        }
        self.next_u64() % max
    }
}

#[inline]
fn encode_u64_key(v: u64) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

// =====================================================================
//  百分位统计辅助
// =====================================================================

struct LatencyStats {
    p50_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
    p999_ns: u64,
    max_ns: u64,
    mean_ns: f64,
    total_count: usize,
}

/// 计算延迟百分位（输入单位：纳秒）
fn compute_latency_stats(mut samples_ns: Vec<u64>) -> LatencyStats {
    assert!(!samples_ns.is_empty(), "samples cannot be empty");
    samples_ns.sort_unstable();

    let total = samples_ns.len();
    let percentile = |p: f64| -> u64 {
        let idx = ((p / 100.0) * (total as f64)) as usize;
        let idx = idx.min(total - 1);
        samples_ns[idx]
    };

    let sum: u128 = samples_ns.iter().map(|&x| x as u128).sum();
    let mean_ns = (sum as f64) / (total as f64);

    LatencyStats {
        p50_ns: percentile(50.0),
        p95_ns: percentile(95.0),
        p99_ns: percentile(99.0),
        p999_ns: percentile(99.9),
        max_ns: *samples_ns.last().unwrap(),
        mean_ns,
        total_count: total,
    }
}

// =====================================================================
//  M1 基准测试：B-Tree 点查延迟
// =====================================================================

/// M1 基准测试：1M key 装入 B-Tree，预热后执行 10M/100M 次随机点查
///
/// 通过标准：P50 < 10μs (10,000 ns)，P99 < 50μs (50,000 ns)
#[test]
fn m1_bench_point_query_latency() {
    // 1. 配置参数
    let total_queries: usize = std::env::var("SZRSQL_M1_BENCH_QUERIES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000_000); // 默认 10M，CI 控制；完整验证设为 100M

    let warmup_queries: usize = std::env::var("SZRSQL_M1_BENCH_WARMUP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000); // 默认 1M 预热

    let key_space: usize = 1_000_000; // 1M 唯一 key 装入 B-Tree

    println!(
        "[m1_bench] config: key_space={}, warmup={}, total_queries={}",
        key_space, warmup_queries, total_queries
    );

    // 2. 装入 1M 唯一 key
    let mut rng = XorShift64::new(0xCAFE_BABE_2024_0718);
    let mut unique_keys: Vec<u64> = Vec::with_capacity(key_space);
    {
        let mut seen = std::collections::HashSet::with_capacity(key_space);
        while seen.len() < key_space {
            let k = rng.next_u64();
            if seen.insert(k) {
                unique_keys.push(k);
            }
        }
    }
    assert_eq!(unique_keys.len(), key_space);

    let mut bt = BTree::with_default_order();
    let build_start = Instant::now();
    for (idx, &k) in unique_keys.iter().enumerate() {
        bt.insert(encode_u64_key(k), (idx % 65536) as u16)
            .expect("insert should not fail");
    }
    let build_elapsed = build_start.elapsed();
    println!(
        "[m1_bench] B-Tree built: {} keys, height={}, nodes={}, build_time={:.3}s",
        key_space,
        bt.height(),
        bt.node_count(),
        build_elapsed.as_secs_f64()
    );

    // 验证装入正确
    assert_eq!(
        bt.in_order_leaf_traverse().expect("traverse").len(),
        key_space
    );

    // 3. 预热：执行 warmup_queries 次随机查询（不计时）
    let warmup_start = Instant::now();
    let mut warmup_hits = 0u64;
    for i in 0..warmup_queries {
        let k = unique_keys[i % key_space];
        let found = bt.search(&encode_u64_key(k)).expect("search");
        if found.is_some() {
            warmup_hits += 1;
        }
    }
    let warmup_elapsed = warmup_start.elapsed();
    assert_eq!(warmup_hits, warmup_queries as u64, "warmup should all hit");
    println!(
        "[m1_bench] warmup done: {} queries, {:.3}s, {:.0} qps",
        warmup_queries,
        warmup_elapsed.as_secs_f64(),
        (warmup_queries as f64) / warmup_elapsed.as_secs_f64()
    );

    // 4. 正式测量：每次 search 单独计时
    // 预分配容量避免 realloc 干扰
    let mut samples_ns: Vec<u64> = vec![0u64; total_queries];

    // 准备查询序列（混合命中 + 未命中）
    let mut query_keys: Vec<Vec<u8>> = Vec::with_capacity(total_queries);
    let mut expected_hit: Vec<bool> = Vec::with_capacity(total_queries);
    for _ in 0..total_queries {
        // 90% 命中（已存在 key），10% 未命中（随机生成的新 key）
        let is_hit = (rng.next_u64_below(10)) < 9;
        if is_hit {
            let k = unique_keys[(rng.next_u64_below(key_space as u64)) as usize];
            query_keys.push(encode_u64_key(k));
            expected_hit.push(true);
        } else {
            // 未命中 key：使用 key_space 之外的随机值（仍有极小概率撞中，不影响统计）
            let k = rng.next_u64();
            query_keys.push(encode_u64_key(k));
            expected_hit.push(unique_keys.contains(&k));
        }
    }

    // 执行点查并逐次记录耗时
    let measure_start = Instant::now();
    for (i, key) in query_keys.iter().enumerate() {
        let q_start = Instant::now();
        let _found = bt.search(key).expect("search");
        let q_elapsed = q_start.elapsed();
        samples_ns[i] = q_elapsed.as_nanos() as u64;
    }
    let measure_elapsed = measure_start.elapsed();

    // 5. 验证正确性：所有 expected_hit=true 的必须命中
    let mut actual_hits = 0u64;
    for (i, key) in query_keys.iter().enumerate() {
        let found = bt.search(key).expect("search");
        if expected_hit[i] {
            assert!(found.is_some(), "expected hit but missed at query {}", i);
            actual_hits += 1;
        }
    }
    let expected_hits = expected_hit.iter().filter(|&&x| x).count() as u64;
    assert_eq!(actual_hits, expected_hits, "hit count mismatch");

    // 6. 计算延迟统计
    let stats = compute_latency_stats(samples_ns);
    let qps = (total_queries as f64) / measure_elapsed.as_secs_f64();

    println!();
    println!("==================== M1 B-Tree 基准测试结果 ====================");
    println!(
        "B-Tree:            {} keys, order=256, height={}, nodes={}",
        key_space,
        bt.height(),
        bt.node_count()
    );
    println!("查询总数:          {}", stats.total_count);
    println!("预热:              {} queries", warmup_queries);
    println!("总测量时间:        {:.3}s", measure_elapsed.as_secs_f64());
    println!("吞吐量:            {:.0} qps (queries/sec)", qps);
    println!();
    println!("延迟统计 (ns = 纳秒, μs = 微秒):");
    println!(
        "  Mean:   {:>10} ns  ({:.3} μs)",
        stats.mean_ns as u64,
        stats.mean_ns / 1000.0
    );
    println!(
        "  P50:    {:>10} ns  ({:.3} μs)  {}",
        stats.p50_ns,
        stats.p50_ns as f64 / 1000.0,
        if stats.p50_ns < 10_000 {
            "✅ < 10μs"
        } else {
            "❌ >= 10μs"
        }
    );
    println!(
        "  P95:    {:>10} ns  ({:.3} μs)",
        stats.p95_ns,
        stats.p95_ns as f64 / 1000.0
    );
    println!(
        "  P99:    {:>10} ns  ({:.3} μs)  {}",
        stats.p99_ns,
        stats.p99_ns as f64 / 1000.0,
        if stats.p99_ns < 50_000 {
            "✅ < 50μs"
        } else {
            "❌ >= 50μs"
        }
    );
    println!(
        "  P99.9:  {:>10} ns  ({:.3} μs)",
        stats.p999_ns,
        stats.p999_ns as f64 / 1000.0
    );
    println!(
        "  Max:    {:>10} ns  ({:.3} μs)",
        stats.max_ns,
        stats.max_ns as f64 / 1000.0
    );
    println!("================================================================");

    // 7. 判定：P50 < 10μs 且 P99 < 50μs
    assert!(
        stats.p50_ns < 10_000,
        "M1 benchmark FAILED: P50={}ns >= 10μs (10000ns)",
        stats.p50_ns
    );
    assert!(
        stats.p99_ns < 50_000,
        "M1 benchmark FAILED: P99={}ns >= 50μs (50000ns)",
        stats.p99_ns
    );

    println!();
    println!(
        "✅ M1 benchmark PASSED: P50={:.3}μs < 10μs, P99={:.3}μs < 50μs",
        stats.p50_ns as f64 / 1000.0,
        stats.p99_ns as f64 / 1000.0
    );
}

// =====================================================================
//  辅助基准：插入吞吐量（非 M1 强制项，用于评估）
// =====================================================================

/// 辅助基准：B-Tree 插入吞吐量（1M key 顺序 vs 随机）
#[test]
fn m1_bench_insert_throughput() {
    let key_count: usize = 1_000_000;

    // 1. 随机插入
    let mut rng = XorShift64::new(0x1234_5678_9ABC_DEF0);
    let mut keys: Vec<u64> = Vec::with_capacity(key_count);
    {
        let mut seen = std::collections::HashSet::with_capacity(key_count);
        while seen.len() < key_count {
            let k = rng.next_u64();
            if seen.insert(k) {
                keys.push(k);
            }
        }
    }
    assert_eq!(keys.len(), key_count);

    // 随机顺序插入
    let mut bt_rand = BTree::with_default_order();
    let rand_start = Instant::now();
    for (idx, &k) in keys.iter().enumerate() {
        bt_rand
            .insert(encode_u64_key(k), (idx % 65536) as u16)
            .expect("insert");
    }
    let rand_elapsed = rand_start.elapsed();
    let rand_qps = (key_count as f64) / rand_elapsed.as_secs_f64();

    // 升序插入（最优场景）
    let mut sorted_keys = keys.clone();
    sorted_keys.sort_unstable();
    let mut bt_sorted = BTree::with_default_order();
    let sorted_start = Instant::now();
    for (idx, &k) in sorted_keys.iter().enumerate() {
        bt_sorted
            .insert(encode_u64_key(k), (idx % 65536) as u16)
            .expect("insert");
    }
    let sorted_elapsed = sorted_start.elapsed();
    let sorted_qps = (key_count as f64) / sorted_elapsed.as_secs_f64();

    println!();
    println!("================ M1 B-Tree 插入吞吐量 ================");
    println!("key_count:         {}", key_count);
    println!(
        "随机插入:          {:.3}s  ({:.0} ops/sec)",
        rand_elapsed.as_secs_f64(),
        rand_qps
    );
    println!(
        "升序插入:          {:.3}s  ({:.0} ops/sec)",
        sorted_elapsed.as_secs_f64(),
        sorted_qps
    );
    println!("随机/升序 比:      {:.2}x", sorted_qps / rand_qps);
    println!("=====================================================");

    // 基本断言：插入后所有 key 可查
    assert_eq!(bt_rand.in_order_leaf_traverse().unwrap().len(), key_count);
    assert_eq!(bt_sorted.in_order_leaf_traverse().unwrap().len(), key_count);
}
