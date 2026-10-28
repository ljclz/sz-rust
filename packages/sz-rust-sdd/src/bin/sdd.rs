// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use clap::Parser;
use sz_rust_sdd::cli::SddCli;
use sz_rust_sdd::error::SddResult;

#[tokio::main]
async fn main() -> SddResult<()> {
    let cli = SddCli::parse();
    let exit_code = cli.execute().await?;
    std::process::exit(exit_code);
}
