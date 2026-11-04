// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use crate::error::MigrationError;

pub struct SddBridge;

impl SddBridge {
    pub fn new() -> Self {
        Self
    }

    pub async fn check_availability(&self) -> Result<(), MigrationError> {
        Ok(())
    }
}

impl Default for SddBridge {
    fn default() -> Self {
        Self::new()
    }
}
