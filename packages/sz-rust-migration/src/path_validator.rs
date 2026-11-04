// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::{Path, PathBuf};

use crate::error::MigrationError;

pub struct PathValidator;

impl PathValidator {
    pub fn validate(path: &Path) -> Result<PathBuf, MigrationError> {
        for component in path.components() {
            if component == std::path::Component::ParentDir {
                return Err(MigrationError::PathInvalid(path.display().to_string()));
            }
        }
        Ok(path.to_path_buf())
    }

    pub fn validate_dir(path: &Path) -> Result<PathBuf, MigrationError> {
        Self::validate(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn reject_parent_dir() {
        let path = PathBuf::from("/tmp/../etc/passwd");
        assert!(matches!(
            PathValidator::validate(&path),
            Err(MigrationError::PathInvalid(_))
        ));
    }

    #[test]
    fn accept_normal_absolute() {
        let path = PathBuf::from("/tmp/projects/tp6");
        let result = PathValidator::validate(&path).unwrap();
        assert_eq!(result, path);
    }

    #[test]
    fn accept_relative() {
        let path = PathBuf::from("projects/tp6");
        let result = PathValidator::validate(&path).unwrap();
        assert_eq!(result, path);
    }

    #[test]
    fn validate_dir_accepts_directory() {
        let path = PathBuf::from("/tmp/tp6");
        let result = PathValidator::validate_dir(&path).unwrap();
        assert_eq!(result, path);
    }
}
