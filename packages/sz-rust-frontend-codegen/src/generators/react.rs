//! React 组件生成器
#![allow(dead_code)]

use crate::config::GenerationConfig;
use crate::error::FrontendCodegenError;
use crate::metadata::ModelMetadata;
use crate::report::GeneratedFile;
use crate::template_engine::CodegenTemplateEngine;

/// React 组件生成器
pub struct ReactComponentGenerator<'a> {
    engine: &'a CodegenTemplateEngine,
}

impl<'a> ReactComponentGenerator<'a> {
    /// 创建生成器
    pub fn new(engine: &'a CodegenTemplateEngine) -> Self {
        Self { engine }
    }

    /// 生成 React 组件
    pub async fn generate(
        &self,
        model: &ModelMetadata,
        config: &GenerationConfig,
    ) -> Result<Vec<GeneratedFile>, FrontendCodegenError> {
        let mut context = tera::Context::new();
        context.insert("model", model);
        context.insert("fields", &model.fields);
        context.insert("writable_fields", &model.writable_fields());
        context.insert("module_name", &model.module_name);
        context.insert("api_module", &model.module_name);
        context.insert("with_tests", &config.with_tests);

        let templates = [
            ("react/list.tsx.tera", "Index.tsx"),
            ("react/show.tsx.tera", "Show.tsx"),
            ("react/form_create.tsx.tera", "Create.tsx"),
            ("react/form_edit.tsx.tera", "Edit.tsx"),
        ];

        let mut files = Vec::new();
        for (tmpl, output_name) in templates {
            let content = self.engine.render(tmpl, &context)?;
            let path = std::path::PathBuf::from(format!(
                "src/pages/{}/{}",
                model.module_name, output_name
            ));
            files.push(GeneratedFile {
                path,
                size_bytes: content.len() as u64,
                source_model: model.name.clone(),
                source_template: tmpl.to_string(),
                is_overwritten: false,
            });
        }

        Ok(files)
    }
}
