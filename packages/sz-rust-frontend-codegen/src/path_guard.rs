//! 路径穿越防护

use std::path::Path;

use crate::error::FrontendCodegenError;

/// 路径守卫
pub struct PathGuard;

impl PathGuard {
    /// 校验路径在 base_dir 内，拒绝路径穿越
    pub fn validate(path: &Path, base_dir: &Path) -> Result<(), FrontendCodegenError> {
        let path_str = path.to_string_lossy();
        if path_str.contains('\0') {
            return Err(FrontendCodegenError::TemplatePathTraversal(format!(
                "路径含控制字符: {path_str}"
            )));
        }
        if path.is_absolute() {
            return Err(FrontendCodegenError::TemplatePathTraversal(format!(
                "拒绝绝对路径: {path_str}"
            )));
        }
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(FrontendCodegenError::TemplatePathTraversal(format!(
                    "拒绝路径穿越（含 ..）: {path_str}"
                )));
            }
        }
        let canonical_base = base_dir
            .canonicalize()
            .unwrap_or_else(|_| base_dir.to_path_buf());
        let full = canonical_base.join(path);
        if !full.starts_with(&canonical_base) {
            return Err(FrontendCodegenError::TemplatePathTraversal(format!(
                "路径超出 base_dir: {path_str}"
            )));
        }
        Ok(())
    }
}
