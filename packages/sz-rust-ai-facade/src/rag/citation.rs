// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Citation {
    pub doc_id: String,
    pub offset: u32,
    pub length: u32,
    pub score: f32,
    pub text: String,
}

impl Citation {
    pub fn new(doc_id: impl Into<String>, score: f32, text: impl Into<String>) -> Self {
        Self {
            doc_id: doc_id.into(),
            offset: 0,
            length: 0,
            score,
            text: text.into(),
        }
    }
}
