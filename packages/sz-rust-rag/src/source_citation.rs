//! 来源引用（v1.5.0 P2-3）
//!
//! 每个检索结果附来源引用，包含文档 ID、片段位置、置信度（spec 5.7.3）。
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// 来源引用
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceCitation {
    /// 文档 ID
    pub doc_id: String,
    /// 片段位置（在文档中的偏移）
    pub fragment_position: usize,
    /// 置信度（0.0-1.0）
    pub confidence: f64,
}

impl SourceCitation {
    /// 创建来源引用
    pub fn new(doc_id: &str, fragment_position: usize, confidence: f64) -> Self {
        Self {
            doc_id: doc_id.to_string(),
            fragment_position,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_citation_creation() {
        let citation = SourceCitation::new("doc_001", 42, 0.95);
        assert_eq!(citation.doc_id, "doc_001");
        assert_eq!(citation.fragment_position, 42);
        assert!((citation.confidence - 0.95).abs() < 1e-9);
    }

    #[test]
    fn test_confidence_clamped() {
        let c1 = SourceCitation::new("doc", 0, 1.5);
        assert!((c1.confidence - 1.0).abs() < 1e-9, "应钳制到 1.0");

        let c2 = SourceCitation::new("doc", 0, -0.5);
        assert!((c2.confidence - 0.0).abs() < 1e-9, "应钳制到 0.0");
    }

    #[test]
    fn test_source_citation_equality() {
        let c1 = SourceCitation::new("doc_1", 10, 0.8);
        let c2 = SourceCitation::new("doc_1", 10, 0.8);
        let c3 = SourceCitation::new("doc_2", 10, 0.8);
        assert_eq!(c1, c2);
        assert_ne!(c1, c3);
    }
}
