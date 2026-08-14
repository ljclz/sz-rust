//! 模板引擎封装

use std::path::{Path, PathBuf};

use tera::Tera;

use crate::error::FrontendCodegenError;
use crate::filters::register_filters;
use crate::path_guard::PathGuard;

/// 代码生成模板引擎
pub struct CodegenTemplateEngine {
    /// Tera 实例
    tera: Tera,
    /// 内置模板目录
    builtin_dir: PathBuf,
}

impl CodegenTemplateEngine {
    /// 初始化模板引擎
    pub async fn init(
        builtin_dir: &Path,
        custom_dir: Option<&Path>,
    ) -> Result<Self, FrontendCodegenError> {
        let mut tera = Tera::default();

        Self::load_builtin_templates(&mut tera, builtin_dir).await?;

        if let Some(custom) = custom_dir {
            Self::load_custom_templates(&mut tera, custom).await?;
        }

        register_filters(&mut tera);

        Ok(Self {
            tera,
            builtin_dir: builtin_dir.to_path_buf(),
        })
    }

    async fn load_builtin_templates(
        tera: &mut Tera,
        dir: &Path,
    ) -> Result<(), FrontendCodegenError> {
        if !tokio::fs::try_exists(dir).await.unwrap_or(false) {
            return Err(FrontendCodegenError::TemplateDirNotFound(
                dir.display().to_string(),
            ));
        }
        Box::pin(Self::load_templates_recursive(tera, dir, dir)).await
    }

    async fn load_custom_templates(
        tera: &mut Tera,
        dir: &Path,
    ) -> Result<(), FrontendCodegenError> {
        if !tokio::fs::try_exists(dir).await.unwrap_or(false) {
            return Err(FrontendCodegenError::TemplateDirNotFound(
                dir.display().to_string(),
            ));
        }
        Box::pin(Self::load_templates_recursive(tera, dir, dir)).await
    }

    async fn load_templates_recursive(
        tera: &mut Tera,
        current_dir: &Path,
        base_dir: &Path,
    ) -> Result<(), FrontendCodegenError> {
        let mut entries = tokio::fs::read_dir(current_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                Box::pin(Self::load_templates_recursive(tera, &path, base_dir)).await?;
            } else if path.extension().is_some_and(|ext| ext == "tera") {
                let rel = path.strip_prefix(base_dir).unwrap_or(&path);
                PathGuard::validate(rel, Path::new("."))?;
                let name = rel.to_string_lossy().replace('\\', "/");
                let content = tokio::fs::read_to_string(&path).await?;
                tera.add_raw_template(&name, &content).map_err(|e| {
                    FrontendCodegenError::TemplateSyntaxError(format!("{name}: {e}"))
                })?;
            }
        }
        Ok(())
    }

    /// 渲染模板
    pub fn render(
        &self,
        template_name: &str,
        context: &tera::Context,
    ) -> Result<String, FrontendCodegenError> {
        self.tera
            .render(template_name, context)
            .map_err(|e| FrontendCodegenError::TemplateRenderError(format!("{template_name}: {e}")))
    }

    /// 返回内置模板目录
    pub fn builtin_dir(&self) -> &Path {
        &self.builtin_dir
    }
}
