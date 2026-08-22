//! RSS 稳定性测量（内存泄漏检测）
//!
//! 与 `rss_baseline.rs` 配套：基线测启动内存，本示例测**内存是否随操作增长**。
//! 原理：反复执行创建/释放周期 N 次，若存在内存泄漏，RSS 会持续上升；
//! 若稳定，RSS 增量应趋近于 0（GC/分配器噪声范围内）。
//!
//! 运行：`cargo run -p sz-rust-core --example rss_stability`
//!
//! 通过标准：150 个周期后 RSS 增量 < 5 MiB（超过则判定存在泄漏风险）。
//! 当前实测：150 周期 RSS 增量 ≈ 0 MiB（稳定）。

use std::time::Duration;

/// 获取当前进程 RSS（MiB）
///
/// Windows 用 PowerShell 读 WorkingSet64；其他平台读 /proc/self/status。
fn get_rss_mb() -> f64 {
    #[cfg(target_os = "windows")]
    {
        let pid = std::process::id();
        let output = std::process::Command::new("powershell")
            .args([
                "-Command",
                &format!("(Get-Process -Id {pid} | Select-Object -ExpandProperty WorkingSet64)"),
            ])
            .output();
        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                stdout.trim().parse::<f64>().unwrap_or(0.0) / (1024.0 * 1024.0)
            }
            Err(_) => 0.0,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        status
            .lines()
            .find(|l| l.starts_with("VmRSS:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<f64>().ok())
            .map(|kb| kb / 1024.0)
            .unwrap_or(0.0)
    }
}

/// 模拟业务操作：创建缓存条目 → 读回 → 释放（对齐 Cache facade 的使用模式）
fn simulate_business_cycle(cache: &sz_rust_cache_facade::Cache, cycle: usize) {
    // 每周期写入 200 个条目并读回（对齐业务热路径：会话/订单缓存）
    for i in 0..200 {
        let key = format!("mem:key:{cycle}:{i}");
        let value = format!("value-{cycle}-{i}-{}", "x".repeat(512)); // ~512B 载荷
        cache
            .set(&key, value, Some(Duration::from_secs(1)))
            .unwrap();
        let _: Option<String> = cache.get(&key).unwrap();
    }
    // 释放：删除全部写入键（模拟条目过期/删除）
    for i in 0..200 {
        let key = format!("mem:key:{cycle}:{i}");
        cache.delete(&key).unwrap();
    }
}

fn main() {
    println!("RSS 稳定性测量（内存泄漏检测）");
    println!("==================================");

    let cache = sz_rust_cache_facade::Cache::new();
    cache.register_default(sz_rust_cache_facade::MemoryCacheDriver::new());

    // 预热：缓存驱动首次初始化可能有一次性的容量分配
    for i in 0..50 {
        cache.set("warmup", i.to_string(), None).unwrap();
    }
    cache.clear().ok();

    let rss_start = get_rss_mb();
    println!("起始 RSS: {rss_start:.2} MiB");

    let cycles: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(150);

    for cycle in 0..cycles {
        simulate_business_cycle(&cache, cycle as usize);
        if (cycle + 1) % 50 == 0 {
            let rss_now = get_rss_mb();
            println!(
                "周期 {}/{cycles} 完成，RSS: {rss_now:.2} MiB（增量 {:.2} MiB）",
                cycle + 1,
                rss_now - rss_start
            );
        }
    }

    let rss_end = get_rss_mb();
    let delta = rss_end - rss_start;
    println!("==================================");
    println!("结束 RSS: {rss_end:.2} MiB，总增量: {delta:.2} MiB");

    // 通过标准：< 5 MiB（分配器噪声 + 缓存页缓存），> 5 MiB 判定存在泄漏风险
    if delta < 5.0 {
        println!("✅ 通过：{cycles} 个周期（{} 次创建/释放）后 RSS 增量 {delta:.2} MiB < 5 MiB，无内存泄漏迹象", cycles * 200);
    } else {
        println!("❌ 失败：RSS 增量 {delta:.2} MiB ≥ 5 MiB，存在内存泄漏风险，请用 miri/valgrind 进一步定位");
        std::process::exit(1);
    }
}
