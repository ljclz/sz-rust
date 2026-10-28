// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! Windows 内存基线测试（真实 RSS 测量，2026-10-27 由占位实现升级）
//!
//! 在 Windows 环境下使用 `sysinfo` 实测**当前测试进程**的工作集内存（RSS），
//! 作为回归对比基线。原 PowerShell 占位实现已废弃（其只测量 PowerShell 进程自身，
//! 不反映 Rust 进程内存）。
//!
//! ## 运行方式
//!
//! ```bash
//! cargo test -p sz-rust-sz300 --test windows_memory_baseline -- --ignored --nocapture
//! ```
//!
//! ## 阈值
//!
//! - 绝对阈值：默认 150 MB，可用环境变量 `SZ300_MEM_BASELINE_MB` 覆盖。
//! - 增长阈值：5 次采样（间隔 500ms，共约 2.5s）内 RSS 增长 < 20 MB。
//!
//! ## 与铁律 9（启动内存 < 30MB）的关系
//!
//! 铁律 9 针对 sz300-server **release 二进制**的空载 RSS（目标 < 30MB）。本测试测量
//! 的是测试进程自身（含测试框架 + debug 构建），不直接断言 30MB，而是提供真实 RSS
//! 的回归包络。完整的 server 级 30MB 基线需要 DB 环境启动 sz300-server 独立进程，
//! 列为后续项（见 `docs/audit/2026-10-27-全栈工程师评测报告.md` 实施记录）。

#![cfg(windows)]

use sysinfo::{Pid, ProcessesToUpdate, System};

/// 默认绝对阈值（MB）
const DEFAULT_MEMORY_BASELINE_MB: u64 = 150;

/// 采样增长阈值（MB）
const MAX_GROWTH_MB: u64 = 20;

/// 采样次数与间隔
const SAMPLES: usize = 5;
const SAMPLE_INTERVAL_MS: u64 = 500;

/// 从环境变量读取绝对阈值（MB），未设置时使用默认值
fn baseline_threshold_mb() -> u64 {
    std::env::var("SZ300_MEM_BASELINE_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_MEMORY_BASELINE_MB)
}

/// 当前进程的工作集内存（RSS，字节）
///
/// 使用 `sysinfo` 读取当前进程内存。`System::new()` 后通过
/// `refresh_processes(ProcessesToUpdate::Some(&[pid]), true)` 加载指定进程，
/// `Process::memory()` 返回字节数（sysinfo 0.30+）。
fn current_process_rss_bytes() -> u64 {
    let mut system = System::new();
    let pid = Pid::from_u32(std::process::id());
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|p| p.memory()).unwrap_or(0)
}

/// 当前进程 RSS（MiB）
fn current_process_rss_mb() -> f64 {
    current_process_rss_bytes() as f64 / 1_048_576.0
}

/// 真实基线：当前进程 RSS 快照 + 系统总/可用内存
///
/// 采集当前测试进程的工作集内存与系统内存信息，断言 RSS 在合理包络内。
#[test]
#[ignore = "基线测试，需手动运行"]
fn baseline_memory_snapshot() {
    let system = System::new_all();
    let total = system.total_memory();
    let free = system.available_memory();
    let rss_mb = current_process_rss_mb();

    println!("===== Windows Memory Baseline (real RSS) =====");
    println!(
        "TotalPhysicalMemory: {} bytes ({:.2} GB)",
        total,
        total as f64 / 1_073_741_824.0
    );
    println!(
        "AvailableMemory:     {} bytes ({:.2} GB)",
        free,
        free as f64 / 1_073_741_824.0
    );
    println!(
        "CurrentProcess RSS:  {} bytes ({:.2} MB)",
        current_process_rss_bytes(),
        rss_mb
    );
    println!(
        "Threshold:           {} MB (env SZ300_MEM_BASELINE_MB 可覆盖)",
        baseline_threshold_mb()
    );
    println!("================================================");

    // 基础校验：系统内存信息可读取
    assert!(total > 0, "总内存应大于 0");
    assert!(free > 0, "可用内存应大于 0");
    assert!(free <= total, "可用内存不应超过总内存");

    // 绝对包络：当前测试进程 RSS 不得超过阈值（默认 150 MB）
    let threshold_bytes = baseline_threshold_mb() * 1024 * 1024;
    let rss = current_process_rss_bytes();
    assert!(
        rss < threshold_bytes,
        "测试进程 RSS {} MB 超出阈值 {} MB（可设 SZ300_MEM_BASELINE_MB 放宽）",
        rss_mb,
        baseline_threshold_mb()
    );
}

/// 真实基线：连续采样当前进程 RSS，验证无失控增长
#[test]
#[ignore = "基线测试，需手动运行"]
fn baseline_memory_growth_over_sampling() {
    let mut samples: Vec<u64> = Vec::with_capacity(SAMPLES);

    for i in 0..SAMPLES {
        let rss = current_process_rss_bytes();
        samples.push(rss);
        println!(
            "[{:2}/{}] RSS: {:.2} MB ({} bytes)",
            i + 1,
            SAMPLES,
            rss as f64 / 1_048_576.0,
            rss
        );
        std::thread::sleep(std::time::Duration::from_millis(SAMPLE_INTERVAL_MS));
    }

    let first = *samples.first().unwrap();
    let last = *samples.last().unwrap();
    let growth = last.saturating_sub(first);

    println!();
    println!("First: {:.2} MB", first as f64 / 1_048_576.0);
    println!("Last:  {:.2} MB", last as f64 / 1_048_576.0);
    println!("Growth: {:.2} MB", growth as f64 / 1_048_576.0);

    assert!(
        growth < MAX_GROWTH_MB * 1024 * 1024,
        "{:.2} 秒内 RSS 增长 {:.2} MB 超出预期（< {} MB）",
        SAMPLES as u64 * SAMPLE_INTERVAL_MS / 1000,
        growth as f64 / 1_048_576.0,
        MAX_GROWTH_MB
    );
}

/// 验证 sysinfo 多次调用稳定性（替代原 PowerShell 调用稳定性测试）
///
/// 确保后续真实基线测试依赖的 sysinfo 读取能力稳定可用。
#[test]
fn sysinfo_invocation_stability() {
    let mut success_count = 0;
    for _ in 0..5 {
        if current_process_rss_bytes() > 0 {
            success_count += 1;
        }
    }
    assert_eq!(
        success_count, 5,
        "sysinfo 调用应 5/5 成功，实际 {}/5",
        success_count
    );
}
