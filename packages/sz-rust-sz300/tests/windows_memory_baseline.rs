//! Windows 内存基线占位测试
//!
//! 在 Windows 环境下测量系统空载内存、请求延迟和内存增长，作为后续回归对比基线。
//!
//! ## 当前状态
//!
//! 此测试为**占位实现**，通过 PowerShell 调用 Windows API 采集当前进程的内存使用快照，
//! 不启动完整 sz300-server，不引入额外 crate 依赖。
//!
//! 完整基线测试需要：
//! 1. 启动 sz300-server（独立进程）
//! 2. 持续 N 秒采样进程内存（工作集 / 私有字节）
//! 3. 并发压测，记录 QPS 与延迟分位数
//! 4. 输出基线 JSON 到 `docs/benchmarks/windows-baseline.json`
//!
//! 当前仅验证：
//! - PowerShell 可成功调用并返回内存数据
//! - 内存值在合理范围内（> 0）
//!
//! ## 运行方式
//!
//! ```bash
//! cargo test -p sz-rust-sz300 --test windows_memory_baseline -- --ignored --nocapture
//! ```

#![cfg(windows)]

use std::process::Command;

/// 通过 PowerShell 调用 .NET API 获取当前进程的工作集内存（字节）
///
/// 使用 `[Environment]::WorkingSet` 获取 PowerShell 进程自身的工作集，
/// 用作基线对比参考。返回 0 表示调用失败。
fn powershell_working_set() -> u64 {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "[long][Environment]::WorkingSet",
        ])
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout.trim().parse::<u64>().unwrap_or(0)
        }
        Err(_) => 0,
    }
}

/// 通过 PowerShell 获取系统可用物理内存（字节）
///
/// 调用 `Get-CimInstance Win32_OperatingSystem` 获取 `FreePhysicalMemory`，
/// 返回 0 表示调用失败。
fn powershell_free_physical_memory() -> u64 {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory * 1KB",
        ])
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            // PowerShell 可能返回科学计数法或带小数点的字符串
            let trimmed = stdout.trim();
            trimmed.parse::<u64>().unwrap_or_else(|_| {
                trimmed
                    .parse::<f64>()
                    .map(|v| v as u64)
                    .unwrap_or(0)
            })
        }
        Err(_) => 0,
    }
}

/// 通过 PowerShell 获取系统总物理内存（字节）
fn powershell_total_physical_memory() -> u64 {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_OperatingSystem).TotalVisibleMemorySize * 1KB",
        ])
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            trimmed.parse::<u64>().unwrap_or_else(|_| {
                trimmed
                    .parse::<f64>()
                    .map(|v| v as u64)
                    .unwrap_or(0)
            })
        }
        Err(_) => 0,
    }
}

/// 占位：系统空载内存基线
///
/// 采集系统总内存、可用内存和 PowerShell 进程工作集，作为基线快照。
/// 完整实现应启动 sz300-server 并采样其内存。
#[test]
#[ignore = "基线测试，需手动运行"]
fn baseline_memory_snapshot() {
    let total = powershell_total_physical_memory();
    let free = powershell_free_physical_memory();
    let working_set = powershell_working_set();

    println!("===== Windows Memory Baseline (Placeholder) =====");
    println!("TotalPhysicalMemory: {} bytes ({:.2} GB)", total, total as f64 / 1_073_741_824.0);
    println!("FreePhysicalMemory:  {} bytes ({:.2} GB)", free, free as f64 / 1_073_741_824.0);
    println!("WorkingSet (PS):     {} bytes ({:.2} MB)", working_set, working_set as f64 / 1_048_576.0);
    println!("================================================");

    // 基础校验：系统内存信息可读取
    assert!(total > 0, "TotalVisibleMemorySize 应大于 0");
    assert!(free > 0, "FreePhysicalMemory 应大于 0");
    assert!(working_set > 0, "WorkingSet 应大于 0");

    // 合理范围校验：可用内存必须小于总内存
    assert!(
        free <= total,
        "FreePhysicalMemory ({}) 不应超过 TotalVisibleMemorySize ({})",
        free,
        total
    );

    // 合理范围校验：PowerShell 进程工作集 < 200 MB
    assert!(
        working_set < 200 * 1024 * 1024,
        "PowerShell 工作集 {} MB 超出预期（< 200 MB）",
        working_set as f64 / 1_048_576.0
    );
}

/// 占位：连续采样内存增长
///
/// 当前仅做 5 次采样，验证 PowerShell 多次调用稳定性。
/// 完整实现应采样 60 秒，每秒一次，并输出 JSON 报告。
#[test]
#[ignore = "基线测试，需手动运行"]
fn baseline_memory_growth_over_sampling() {
    let samples = 5;
    let mut working_sets: Vec<u64> = Vec::with_capacity(samples);

    for i in 0..samples {
        let ws = powershell_working_set();
        working_sets.push(ws);
        println!("[{:2}/{}] WorkingSet: {:.2} MB", i + 1, samples, ws as f64 / 1_048_576.0);
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    let first = *working_sets.first().unwrap();
    let last = *working_sets.last().unwrap();
    let growth = last.saturating_sub(first);

    println!();
    println!("First: {:.2} MB", first as f64 / 1_048_576.0);
    println!("Last:  {:.2} MB", last as f64 / 1_048_576.0);
    println!("Growth: {:.2} MB", growth as f64 / 1_048_576.0);

    // 占位校验：5 次采样（2.5 秒）内内存增长 < 100 MB
    // PowerShell 进程自身有波动，留足余量
    assert!(
        growth < 100 * 1024 * 1024,
        "2.5 秒内 PowerShell 工作集增长 {:.2} MB 超出预期（< 100 MB）",
        growth as f64 / 1_048_576.0
    );
}

/// 验证 PowerShell 命令调用稳定性
///
/// 此测试用于确保 Windows 环境下 PowerShell 可被 Rust 进程调用，
/// 后续完整基线测试依赖此能力。
#[test]
fn powershell_invocation_stability() {
    let mut success_count = 0;
    for _ in 0..5 {
        let ws = powershell_working_set();
        if ws > 0 {
            success_count += 1;
        }
    }
    assert_eq!(
        success_count, 5,
        "PowerShell 调用应 5/5 成功，实际 {}/5",
        success_count
    );
}
