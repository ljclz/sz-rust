//! Vue 组件生成器

use crate::config::GenerationConfig;
use crate::error::FrontendCodegenError;
use crate::metadata::ModelMetadata;
use crate::report::GeneratedFile;
use crate::template_engine::CodegenTemplateEngine;
use crate::ui_adapter::UiAdaptedModel;

/// Vue 组件生成器
pub struct VueComponentGenerator<'a> {
    engine: &'a CodegenTemplateEngine,
}

impl<'a> VueComponentGenerator<'a> {
    /// 创建生成器
    pub fn new(engine: &'a CodegenTemplateEngine) -> Self {
        Self { engine }
    }

    /// 生成 Vue 组件
    pub async fn generate(
        &self,
        model: &ModelMetadata,
        ui_adapted: &UiAdaptedModel<'_>,
        config: &GenerationConfig,
    ) -> Result<Vec<GeneratedFile>, FrontendCodegenError> {
        let mut context = tera::Context::new();
        context.insert("model", model);
        context.insert("fields", &model.fields);
        context.insert("writable_fields", &model.writable_fields());
        context.insert("relations", &model.relations);
        context.insert("ui_tags", &ui_adapted.tags);
        context.insert("module_name", &model.module_name);
        context.insert("api_module", &model.module_name);
        context.insert("with_tests", &config.with_tests);

        let templates = [
            ("vue/list.vue.tera", "Index.vue"),
            ("vue/show.vue.tera", "Show.vue"),
            ("vue/form_create.vue.tera", "Create.vue"),
            ("vue/form_edit.vue.tera", "Edit.vue"),
        ];

        let mut files = Vec::new();
        for (tmpl, output_name) in templates {
            let content = self.engine.render(tmpl, &context)?;
            let path = std::path::PathBuf::from(format!(
                "src/views/{}/{}",
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

        if config.with_tests {
            let test_templates = [
                ("vue/list.spec.ts.tera", "Index.spec.ts"),
                ("vue/show.spec.ts.tera", "Show.spec.ts"),
                ("vue/form_create.spec.ts.tera", "Create.spec.ts"),
                ("vue/form_edit.spec.ts.tera", "Edit.spec.ts"),
            ];
            for (tmpl, output_name) in test_templates {
                let content = self.engine.render(tmpl, &context)?;
                let path = std::path::PathBuf::from(format!(
                    "src/views/{}/__tests__/{}",
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
        }

        Ok(files)
    }
}
