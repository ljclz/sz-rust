//! 权限生成器

use crate::config::GenerationConfig;
use crate::error::FrontendCodegenError;
use crate::report::GeneratedFile;
use crate::template_engine::CodegenTemplateEngine;

/// 权限配置
#[derive(Debug, Clone, serde::Serialize)]
pub struct PermissionConfig {
    /// 权限码列表
    pub permissions: Vec<String>,
    /// 角色列表
    pub roles: Vec<String>,
    /// 登录路径
    pub login_path: String,
    /// 禁止访问路径
    pub forbidden_path: String,
}

/// 权限生成器
pub struct PermissionGenerator<'a> {
    engine: &'a CodegenTemplateEngine,
}

impl<'a> PermissionGenerator<'a> {
    /// 创建生成器
    pub fn new(engine: &'a CodegenTemplateEngine) -> Self {
        Self { engine }
    }

    /// 生成权限文件
    pub async fn generate(
        &self,
        perm_config: &PermissionConfig,
        _config: &GenerationConfig,
    ) -> Result<Vec<GeneratedFile>, FrontendCodegenError> {
        let mut context = tera::Context::new();
        context.insert("permissions", &perm_config.permissions);
        context.insert("roles", &perm_config.roles);
        context.insert("login_path", &perm_config.login_path);
        context.insert("forbidden_path", &perm_config.forbidden_path);

        let templates = [
            ("router/guard.ts.tera", "src/router/guard.ts"),
            (
                "composables/usePermission.ts.tera",
                "src/composables/usePermission.ts",
            ),
            (
                "directives/permission.ts.tera",
                "src/directives/permission.ts",
            ),
            (
                "constants/permissions.ts.tera",
                "src/constants/permissions.ts",
            ),
        ];

        let mut files = Vec::new();
        for (tmpl, output) in templates {
            let content = self.engine.render(tmpl, &context)?;
            files.push(GeneratedFile {
                path: std::path::PathBuf::from(output),
                size_bytes: content.len() as u64,
                source_model: "permission".to_string(),
                source_template: tmpl.to_string(),
                is_overwritten: false,
            });
        }
        Ok(files)
    }
}
