// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationPhase {
    Analysis,
    Generation,
    Validation,
    Reporting,
    CaseDoc,
}

impl MigrationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Analysis => "analysis",
            Self::Generation => "generation",
            Self::Validation => "validation",
            Self::Reporting => "reporting",
            Self::CaseDoc => "case_doc",
        }
    }

    pub fn order(self) -> u8 {
        match self {
            Self::Analysis => 0,
            Self::Generation => 1,
            Self::Validation => 2,
            Self::Reporting => 3,
            Self::CaseDoc => 4,
        }
    }

    pub fn next(self) -> Option<Self> {
        match self {
            Self::Analysis => Some(Self::Generation),
            Self::Generation => Some(Self::Validation),
            Self::Validation => Some(Self::Reporting),
            Self::Reporting => Some(Self::CaseDoc),
            Self::CaseDoc => None,
        }
    }
}

impl std::fmt::Display for MigrationPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStatus {
    Pending,
    Running { current_phase: MigrationPhase },
    Completed,
    Failed { error_code: String },
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Get => write!(f, "GET"),
            Self::Post => write!(f, "POST"),
            Self::Put => write!(f, "PUT"),
            Self::Delete => write!(f, "DELETE"),
            Self::Patch => write!(f, "PATCH"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplexityLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ComplexitySummary {
    pub low: u32,
    pub medium: u32,
    pub high: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_serde_snake_case() {
        let json = serde_json::to_string(&MigrationPhase::Analysis).unwrap();
        assert_eq!(json, "\"analysis\"");
        let json = serde_json::to_string(&MigrationPhase::CaseDoc).unwrap();
        assert_eq!(json, "\"case_doc\"");
    }

    #[test]
    fn phase_next_chain() {
        assert_eq!(
            MigrationPhase::Analysis.next(),
            Some(MigrationPhase::Generation)
        );
        assert_eq!(MigrationPhase::CaseDoc.next(), None);
    }

    #[test]
    fn http_method_uppercase_serde() {
        let json = serde_json::to_string(&HttpMethod::Get).unwrap();
        assert_eq!(json, "\"GET\"");
        let json = serde_json::to_string(&HttpMethod::Post).unwrap();
        assert_eq!(json, "\"POST\"");
    }

    #[test]
    fn complexity_level_serde() {
        let json = serde_json::to_string(&ComplexityLevel::Low).unwrap();
        assert_eq!(json, "\"low\"");
    }
}
