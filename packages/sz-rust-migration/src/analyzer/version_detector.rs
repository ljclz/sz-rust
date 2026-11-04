// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::Path;

use crate::error::{MigrationError, MigrationResult};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Tp6Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

pub struct Tp6VersionDetector;

impl Tp6VersionDetector {
    pub fn new() -> Self {
        Self
    }

    pub async fn detect(&self, project_path: &Path) -> MigrationResult<Tp6Version> {
        let composer_path = project_path.join("composer.json");
        let content = tokio::fs::read_to_string(&composer_path)
            .await
            .map_err(|_| {
                MigrationError::Tp6FormatInvalid(format!(
                    "composer.json not found at {}",
                    composer_path.display()
                ))
            })?;
        let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
            MigrationError::Tp6FormatInvalid(format!("composer.json parse error: {e}"))
        })?;
        let framework_version = json
            .get("require")
            .and_then(|v| v.get("topthink/framework"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                MigrationError::Tp6FormatInvalid(
                    "composer.json missing require.topthink/framework".into(),
                )
            })?;
        let version = parse_version_constraint(framework_version)
            .ok_or_else(|| MigrationError::Tp6VersionUnsupported(framework_version.into()))?;
        if version.major < 6 {
            return Err(MigrationError::Tp6VersionUnsupported(format!(
                "TP{major}.{minor}.{patch}, requires >= 6.0.0",
                major = version.major,
                minor = version.minor,
                patch = version.patch,
            )));
        }
        Ok(version)
    }
}

impl Default for Tp6VersionDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_version_constraint(constraint: &str) -> Option<Tp6Version> {
    let cleaned = constraint
        .trim_start_matches('^')
        .trim_start_matches('~')
        .trim_start_matches('>')
        .trim_start_matches('=')
        .trim();
    let parts: Vec<&str> = cleaned.split('.').collect();
    if parts.len() >= 3 {
        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        let patch = parts[2].parse().ok()?;
        Some(Tp6Version {
            major,
            minor,
            patch,
        })
    } else if parts.len() == 2 {
        let major = parts[0].parse().ok()?;
        let minor = parts[1].parse().ok()?;
        Some(Tp6Version {
            major,
            minor,
            patch: 0,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_caret_constraint() {
        let v = parse_version_constraint("^6.0.13").unwrap();
        assert_eq!(
            v,
            Tp6Version {
                major: 6,
                minor: 0,
                patch: 13
            }
        );
    }

    #[test]
    fn parse_tilde_constraint() {
        let v = parse_version_constraint("~6.1.0").unwrap();
        assert_eq!(
            v,
            Tp6Version {
                major: 6,
                minor: 1,
                patch: 0
            }
        );
    }

    #[test]
    fn parse_two_parts() {
        let v = parse_version_constraint("^6.0").unwrap();
        assert_eq!(
            v,
            Tp6Version {
                major: 6,
                minor: 0,
                patch: 0
            }
        );
    }

    #[test]
    fn parse_invalid_returns_none() {
        assert!(parse_version_constraint("invalid").is_none());
    }

    #[tokio::test]
    async fn detect_tp6_project() {
        let temp = tempfile::TempDir::new().unwrap();
        let composer = r#"{"require": {"topthink/framework": "^6.0.13"}}"#;
        tokio::fs::write(temp.path().join("composer.json"), composer)
            .await
            .unwrap();
        let v = Tp6VersionDetector::new().detect(temp.path()).await.unwrap();
        assert_eq!(v.major, 6);
    }

    #[tokio::test]
    async fn detect_tp5_rejects() {
        let temp = tempfile::TempDir::new().unwrap();
        let composer = r#"{"require": {"topthink/framework": "^5.1.0"}}"#;
        tokio::fs::write(temp.path().join("composer.json"), composer)
            .await
            .unwrap();
        let result = Tp6VersionDetector::new().detect(temp.path()).await;
        assert!(matches!(
            result,
            Err(MigrationError::Tp6VersionUnsupported(_))
        ));
    }

    #[tokio::test]
    async fn detect_missing_composer_json() {
        let temp = tempfile::TempDir::new().unwrap();
        let result = Tp6VersionDetector::new().detect(temp.path()).await;
        assert!(matches!(result, Err(MigrationError::Tp6FormatInvalid(_))));
    }
}
