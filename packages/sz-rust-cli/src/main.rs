//! SZ-Rust CLI 二进制入口
//!
//! 对齐 PHP `think` 命令入口脚本 `think`：
//!
//! ```php
//! #!/usr/bin/env php
//! <?php
//! require __DIR__ . '/vendor/autoload.php';
//! $console = new \think\Console();
//! $console->run();
//! ```

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match sz_rust_cli::run(args).await {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("Error: {}", e);
            ExitCode::from(1)
        }
    }
}
