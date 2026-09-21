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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citation_new_sets_defaults() {
        let c = Citation::new("doc-1", 0.95, "sample text");
        assert_eq!(c.doc_id, "doc-1");
        assert_eq!(c.offset, 0);
        assert_eq!(c.length, 0);
        assert!((c.score - 0.95).abs() < 1e-6);
        assert_eq!(c.text, "sample text");
    }

    #[test]
    fn citation_new_accepts_owned_string() {
        let doc_id = String::from("doc-2");
        let text = String::from("hello world");
        let c = Citation::new(doc_id, 0.5, text);
        assert_eq!(c.doc_id, "doc-2");
        assert_eq!(c.text, "hello world");
    }

    #[test]
    fn citation_serde_roundtrip() {
        let c = Citation {
            doc_id: "doc-3".into(),
            offset: 10,
            length: 256,
            score: 0.88,
            text: "roundtrip text".into(),
        };
        let json = serde_json::to_string(&c).unwrap();
        let de: Citation = serde_json::from_str(&json).unwrap();
        assert_eq!(de.doc_id, "doc-3");
        assert_eq!(de.offset, 10);
        assert_eq!(de.length, 256);
        assert!((de.score - 0.88).abs() < 1e-6);
        assert_eq!(de.text, "roundtrip text");
    }

    #[test]
    fn citation_new_zero_score() {
        let c = Citation::new("doc-zero", 0.0, "");
        assert!((c.score - 0.0).abs() < 1e-6);
        assert_eq!(c.text, "");
    }
}
