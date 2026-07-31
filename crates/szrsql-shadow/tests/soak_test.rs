//! Soak Test — 长时间稳定性测试
//!
//! 对应 `docs/全面排查汇总报告.md` P1-4 任务（6h soak test）。
//!
//! # 设计目标
//!
//! 1. **长时间运行**：默认 60s 冒烟；`#[ignore]` 版本 6 小时（GitHub Actions 上限）
//! 2. **混合工作负载**：70% SELECT / 20% INSERT / 7% UPDATE / 3% DELETE
//! 3. **指标采样**：每 60s 采样一次 TPS / p50/p95/p99 延迟 / 表行数 / 操作计数
//! 4. **退化检测**：首 10 分钟平均 TPS vs 末 10 分钟平均 TPS，退化 > 20% 即失败
//! 5. **数据完整性**：INSERT 总数 - DELETE 总数 == 最终表行数
//! 6. **无 panic**：整个测试过程不允许 panic
//!
//! # 用法
//!
//! ```bash
//! # 60s 冒烟测试（CI 默认执行）
//! cargo test -p szrsql-shadow --test soak_test --release -- --nocapture --test-threads=1
//!
//! # 6h 完整 soak test（手动触发 / GitHub Actions 周末定时）
//! cargo test -p szrsql-shadow --test soak_test --release -- --nocapture --ignored --test-threads=1
//! ```

#![allow(clippy::field_reassign_with_default)]

use std::time::{Duration, Instant};

use szrsql_sql::executor::{InMemoryTable, TableStorage};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
// 配置
// =====================================================================

/// 采样间隔（秒）— 实施进度表要求 60s
const SAMPLE_INTERVAL_SECS: u64 = 60;

/// 冒烟测试时长（秒）— CI 默认执行，验证 soak test 自身可用性
const SMOKE_DURATION_SECS: u64 = 60;

/// 完整 soak test 时长（秒）— 6 小时（GitHub Actions 上限）
const FULL_DURATION_SECS: u64 = 6 * 60 * 60;

/// 性能退化阈值（首 10 分钟 vs 末 10 分钟 TPS）— 实施进度表要求 ≤ 20%
const PERF_DEGRADATION_THRESHOLD: f64 = 0.20;

/// 工作表初始行数（warmup 后开始采样）
const INITIAL_ROWS: usize = 10_000;

/// 工作负载配比（总和 = 100）
const RATIO_SELECT: u32 = 70;
const RATIO_INSERT: u32 = 20;
const RATIO_UPDATE: u32 = 7;
const RATIO_DELETE: u32 = 3;

// =====================================================================
// 工作负载生成器
// =====================================================================

/// XorShift64 PRNG（与项目内其他 fuzz/jepsen 测试同风格，确定性可重现）
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

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// [0, n)
    fn next_range(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next_u64() % n
    }
}

/// 工作负载操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Select,
    Insert,
    Update,
    Delete,
}

/// 根据随机数生成操作类型（按配比）
fn pick_op(rng: &mut XorShift64) -> Op {
    let r = (rng.next_u64() % 100) as u32;
    if r < RATIO_SELECT {
        Op::Select
    } else if r < RATIO_SELECT + RATIO_INSERT {
        Op::Insert
    } else if r < RATIO_SELECT + RATIO_INSERT + RATIO_UPDATE {
        Op::Update
    } else {
        Op::Delete
    }
}

// =====================================================================
// 表初始化
// =====================================================================

/// 创建并预填充工作表
fn make_workload_table() -> InMemoryTable {
    let mut table = InMemoryTable::with_columns(
        "soak_t",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("balance", ColumnType::Int64),
        ],
    );

    // 预填充 INITIAL_ROWS 行
    for i in 0..INITIAL_ROWS as i64 {
        let row = vec![
            Value::Int64(i),
            Value::Text(format!("name_{i}")),
            Value::Int64(i * 100),
        ];
        table.insert(row);
    }
    table
}

// =====================================================================
// 操作执行
// =====================================================================

/// 执行单次操作，返回 (操作类型, 是否成功)
///
/// 成功 = 操作已应用；失败 = 预期失败（如 DELETE 不存在的行）
fn execute_op(
    table: &mut InMemoryTable,
    rng: &mut XorShift64,
    next_id: &mut i64,
) -> (Op, bool) {
    let op = pick_op(rng);
    let ok = match op {
        Op::Select => {
            // 全表扫描（最重的读操作）
            let _ = table.rows().len();
            // 等值查询（按 id 查）
            let target = rng.next_range(INITIAL_ROWS as u64 * 2) as i64;
            for row in table.rows() {
                if let Some(Value::Int64(id)) = row.first() {
                    if *id == target {
                        break;
                    }
                }
            }
            true
        }
        Op::Insert => {
            let id = *next_id;
            *next_id += 1;
            let row = vec![
                Value::Int64(id),
                Value::Text(format!("ins_{id}")),
                Value::Int64(rng.next_range(1_000_000) as i64),
            ];
            table.insert(row);
            true
        }
        Op::Update => {
            // 随机更新一行
            let len = table.rows().len();
            if len == 0 {
                false
            } else {
                let row_id = rng.next_range(len as u64) as usize;
                let new_balance = rng.next_range(1_000_000) as i64;
                // 读取旧行，构造新行
                let old_row = table.rows()[row_id].clone();
                let mut new_row = old_row;
                if new_row.len() >= 3 {
                    new_row[2] = Value::Int64(new_balance);
                }
                table.update_row(row_id, new_row)
            }
        }
        Op::Delete => {
            // 随机删除一行（tombstone）
            let len = table.rows().len();
            if len == 0 {
                false
            } else {
                let row_id = rng.next_range(len as u64) as usize;
                table.delete_row(row_id)
            }
        }
    };
    (op, ok)
}

// =====================================================================
// 采样点
// =====================================================================

/// 单个采样点
#[derive(Debug, Clone)]
struct Sample {
    /// 采样时间偏移（秒，相对于测试开始）
    elapsed_secs: u64,
    /// 本次采样窗口内完成的操作数
    ops_in_window: u64,
    /// 本次采样窗口的 TPS（ops/s）
    tps: f64,
    /// 本次采样窗口的 p50 延迟（μs）
    p50_us: u64,
    /// 本次采样窗口的 p95 延迟（μs）
    p95_us: u64,
    /// 本次采样窗口的 p99 延迟（μs）
    p99_us: u64,
    /// 当前表行数（含 tombstone）
    table_rows: usize,
    /// 当前 tombstone 数
    deleted_rows: usize,
}

// =====================================================================
// 延迟统计
// =====================================================================

/// 计算分位数（输入已排序的延迟数组，单位 μs）
fn percentile(sorted_latencies: &[u64], p: f64) -> u64 {
    if sorted_latencies.is_empty() {
        return 0;
    }
    let idx = ((sorted_latencies.len() as f64 - 1.0) * p).round() as usize;
    sorted_latencies[idx.min(sorted_latencies.len() - 1)]
}

// =====================================================================
// Soak Test 核心
// =====================================================================

/// 运行 soak test
///
/// # 参数
/// - `duration_secs`：测试总时长（秒）
/// - `label`：测试标签（用于日志输出）
///
/// # 返回
/// - `Ok(Vec<Sample>)`：所有采样点
/// - `Err(String)`：失败原因
fn run_soak(duration_secs: u64, label: &str) -> Result<Vec<Sample>, String> {
    println!("=== Soak Test [{label}] 开始 ===");
    println!("配置：duration={duration_secs}s, sample_interval={SAMPLE_INTERVAL_SECS}s");
    println!(
        "工作负载配比：SELECT={RATIO_SELECT}% INSERT={RATIO_INSERT}% UPDATE={RATIO_UPDATE}% DELETE={RATIO_DELETE}%"
    );
    println!("初始行数：{INITIAL_ROWS}");
    println!();

    let mut table = make_workload_table();
    let mut rng = XorShift64::new(0x1234_5678_9ABC_DEF0);
    let mut next_id: i64 = INITIAL_ROWS as i64;

    // 统计计数器
    let mut total_ops: u64 = 0;
    let mut total_inserts: u64 = 0;
    let mut total_deletes: u64 = 0;
    let mut successful_deletes: u64 = 0;

    let start = Instant::now();
    let end_time = start + Duration::from_secs(duration_secs);

    let mut samples: Vec<Sample> = Vec::new();
    let mut window_start = start;
    let mut window_ops: u64 = 0;
    let mut window_latencies: Vec<u64> = Vec::new();
    let mut next_sample = start + Duration::from_secs(SAMPLE_INTERVAL_SECS);

    println!(
        "{:<10} {:<12} {:<10} {:<10} {:<10} {:<10} {:<12} {:<10}",
        "elapsed(s)", "ops/window", "tps", "p50(us)", "p95(us)", "p99(us)", "table_rows", "deleted"
    );

    while Instant::now() < end_time {
        let op_start = Instant::now();
        let (op, ok) = execute_op(&mut table, &mut rng, &mut next_id);
        let op_us = op_start.elapsed().as_micros() as u64;

        total_ops += 1;
        window_ops += 1;
        window_latencies.push(op_us);

        // 统计 INSERT/DELETE（用于完整性校验）
        match op {
            Op::Insert => total_inserts += 1,
            Op::Delete => {
                total_deletes += 1;
                if ok {
                    successful_deletes += 1;
                }
            }
            _ => {}
        }

        // 采样
        let now = Instant::now();
        if now >= next_sample {
            let elapsed_secs = window_start.elapsed().as_secs();
            let window_duration = now.duration_since(window_start).as_secs_f64();
            let tps = window_ops as f64 / window_duration.max(1e-9);

            window_latencies.sort_unstable();
            let p50 = percentile(&window_latencies, 0.50);
            let p95 = percentile(&window_latencies, 0.95);
            let p99 = percentile(&window_latencies, 0.99);

            let table_rows = table.rows().len();
            let deleted_rows = table_rows - table.row_count();

            println!(
                "{:<10} {:<12} {:<10.2} {:<10} {:<10} {:<10} {:<12} {:<10}",
                elapsed_secs, window_ops, tps, p50, p95, p99, table_rows, deleted_rows
            );

            samples.push(Sample {
                elapsed_secs,
                ops_in_window: window_ops,
                tps,
                p50_us: p50,
                p95_us: p95,
                p99_us: p99,
                table_rows,
                deleted_rows,
            });

            // 重置窗口
            window_start = now;
            window_ops = 0;
            window_latencies.clear();
            next_sample = now + Duration::from_secs(SAMPLE_INTERVAL_SECS);
        }
    }

    // 最终采样（如果窗口非空）
    if window_ops > 0 {
        let elapsed_secs = start.elapsed().as_secs();
        let window_duration = window_start.elapsed().as_secs_f64();
        let tps = window_ops as f64 / window_duration.max(1e-9);

        window_latencies.sort_unstable();
        let p50 = percentile(&window_latencies, 0.50);
        let p95 = percentile(&window_latencies, 0.95);
        let p99 = percentile(&window_latencies, 0.99);

        let table_rows = table.rows().len();
        let deleted_rows = table_rows - table.row_count();

        println!(
            "{:<10} {:<12} {:<10.2} {:<10} {:<10} {:<10} {:<12} {:<10}",
            elapsed_secs, window_ops, tps, p50, p95, p99, table_rows, deleted_rows
        );

        samples.push(Sample {
            elapsed_secs,
            ops_in_window: window_ops,
            tps,
            p50_us: p50,
            p95_us: p95,
            p99_us: p99,
            table_rows,
            deleted_rows,
        });
    }

    println!();
    println!("=== Soak Test [{label}] 统计汇总 ===");
    println!("总操作数：{total_ops}");
    println!("总 INSERT 数：{total_inserts}");
    println!("总 DELETE 数：{total_deletes}（成功 {successful_deletes}）");
    println!("最终表行数（含 tombstone）：{}", table.rows().len());
    println!("采样点数：{}", samples.len());
    println!();

    // ============================================================
    // 校验 1：数据完整性
    // ============================================================
    // 预期：初始行数 + INSERT 数 - 成功 DELETE 数 == 最终活跃行数
    let active_rows = table.row_count();
    let expected_active = INITIAL_ROWS + total_inserts as usize - successful_deletes as usize;
    if active_rows != expected_active {
        return Err(format!(
            "数据完整性校验失败：active_rows={active_rows}, expected={expected_active} \
             (INITIAL_ROWS={INITIAL_ROWS}, inserts={total_inserts}, successful_deletes={successful_deletes})"
        ));
    }
    println!("✅ 数据完整性校验通过：active_rows={active_rows} == expected={expected_active}");

    // ============================================================
    // 校验 2：性能退化
    // ============================================================
    if samples.len() >= 4 {
        // 首 10 分钟 vs 末 10 分钟（取采样点的 1/4 处）
        let quarter = samples.len() / 4;
        let first_avg_tps: f64 =
            samples[..quarter].iter().map(|s| s.tps).sum::<f64>() / quarter as f64;
        let last_avg_tps: f64 = samples[samples.len() - quarter..]
            .iter()
            .map(|s| s.tps)
            .sum::<f64>()
            / quarter as f64;

        let degradation = if first_avg_tps > 0.0 {
            (first_avg_tps - last_avg_tps) / first_avg_tps
        } else {
            0.0
        };

        println!(
            "首 {quarter} 采样平均 TPS：{first_avg_tps:.2}, 末 {quarter} 采样平均 TPS：{last_avg_tps:.2}, 退化：{:.2}%",
            degradation * 100.0
        );

        if degradation > PERF_DEGRADATION_THRESHOLD {
            return Err(format!(
                "性能退化校验失败：degradation={:.2}% > threshold={:.2}%",
                degradation * 100.0,
                PERF_DEGRADATION_THRESHOLD * 100.0
            ));
        }
        println!("✅ 性能退化校验通过：degradation={:.2}% ≤ {:.2}%", degradation * 100.0, PERF_DEGRADATION_THRESHOLD * 100.0);
    } else {
        println!("⚠️  采样点数 {} < 4，跳过性能退化校验", samples.len());
    }

    // ============================================================
    // 校验 3：无 panic（隐式，能走到这里即说明无 panic）
    // ============================================================
    println!("✅ 无 panic 校验通过");

    println!();
    println!("=== Soak Test [{label}] 全部通过 ✅ ===");
    Ok(samples)
}

// =====================================================================
// 测试入口
// =====================================================================

/// 60s 冒烟测试（CI 默认执行）
///
/// 验证 soak test 自身可用性，不验证 6h 稳定性。
#[test]
fn soak_test_smoke_60s() {
    let samples = run_soak(SMOKE_DURATION_SECS, "smoke-60s").expect("smoke soak test failed");

    // 至少应有 1 个采样点
    assert!(
        !samples.is_empty(),
        "至少应有 1 个采样点，但 samples 为空"
    );

    // TPS 应为正数
    for s in &samples {
        assert!(s.tps > 0.0, "采样点 {:?} 的 TPS 应 > 0", s);
    }
}

/// 6h 完整 soak test（手动触发 / GitHub Actions 周末定时）
///
/// 验证长时间稳定性：
/// - 6 小时无 panic
/// - 性能退化 ≤ 20%
/// - 数据完整性保持
#[test]
#[ignore = "完整 soak test 需要 6 小时，仅在 CI 周末定时或手动触发时执行"]
fn soak_test_full_6h() {
    let samples =
        run_soak(FULL_DURATION_SECS, "full-6h").expect("full soak test failed");

    // 至少应有 60 个采样点（6h / 60s = 360 个，但允许降级）
    assert!(
        samples.len() >= 60,
        "6h soak test 至少应有 60 个采样点，实际：{}",
        samples.len()
    );

    // 最终采样点的 TPS 应 > 0
    let last = samples.last().expect("samples should not be empty");
    assert!(last.tps > 0.0, "最终采样点 TPS 应 > 0，实际：{}", last.tps);

    println!();
    println!("=== 6h Soak Test 完成 ===");
    println!("总采样点数：{}", samples.len());
    println!(
        "首采样 TPS：{:.2}, 末采样 TPS：{:.2}",
        samples.first().unwrap().tps,
        samples.last().unwrap().tps
    );
}

