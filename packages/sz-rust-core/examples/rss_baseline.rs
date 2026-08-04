//! 启动内存 RSS 基线测量（铁律 R9）
//!
//! ## 运行方式
//!
//! ```powershell
//! cargo run --package sz-rust-core --example rss_baseline --release
//! ```
//!
//! ## 输出示例
//!
//! ```text
//! === sz-rust-core 启动内存 RSS 基线 ===
//! 初始 RSS:        2.3 MiB
//! Container 初始化后:  5.1 MiB
//! 峰值 RSS:        5.1 MiB
//! 铁律 R9 阈值:     30.0 MiB
//! 判定:           ✅ 通过（峰值 < 30 MiB）
//! ```

fn get_rss_mb() -> f64 {
    // 使用 Windows tasklist 获取当前进程 RSS（Working Set）
    let pid = std::process::id();
    let output = std::process::Command::new("powershell")
        .args([
            "-Command",
            &format!(
                "(Get-Process -Id {} | Select-Object -ExpandProperty WorkingSet64)",
                pid
            ),
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

fn main() {
    use sz_rust_core::container::Container;

    const R9_THRESHOLD_MB: f64 = 30.0;

    println!("=== sz-rust-core 启动内存 RSS 基线（铁律 R9） ===");

    let mut peak = get_rss_mb();
    println!("初始 RSS:           {:.2} MiB", peak);

    // 初始化 DI 容器（框架核心初始化）
    let _container = Container::default();
    let after_container = get_rss_mb();
    peak = peak.max(after_container);
    println!("Container 初始化后: {:.2} MiB", after_container);

    println!("峰值 RSS:           {:.2} MiB", peak);
    println!("铁律 R9 阈值:        {:.1} MiB", R9_THRESHOLD_MB);

    if peak < R9_THRESHOLD_MB {
        println!("判定:               ✅ 通过（峰值 < 30 MiB）");
    } else {
        println!("判定:               ❌ 失败（峰值 >= 30 MiB，需优化）");
        std::process::exit(1);
    }
}
