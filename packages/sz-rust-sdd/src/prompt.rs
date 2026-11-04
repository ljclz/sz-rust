// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::HashMap;

use tera::{Tera, Value};

use crate::error::{SddError, SddResult};
use crate::model::Stage;

const SPEC_TEMPLATE: &str = r#"# SDD Spec 阶段 Prompt

## 需求文本
{{ requirement }}

## EARS 格式约束
- 所有需求必须使用 EARS 格式（Ubiquitous / Event-driven / State-driven / Optional）
- 每条需求含唯一 ID + 前置条件 + 后置条件

## 六章节结构要求
1. **组件定位**：明确组件名称/职责/边界
2. **领域术语**：定义领域概念与术语表
3. **角色边界**：明确用户角色与权限边界
4. **DFX 约束**：性能/安全/可用性/可维护性约束
5. **核心能力**：列举核心功能与接口
6. **数据约束**：数据模型与持久化约束

{% if redo_comment %}
## 修改意见（Redo）
{{ redo_comment }}
{% endif %}

## 铁律约束
- async fn 必须 Send + 'static
- 禁止 std::fs，统一 tokio::fs
- unsafe_code = "forbid"
- 禁止 crate 级 #![allow(dead_code)]
- 敏感字段脱敏
"#;

const DESIGN_TEMPLATE: &str = r#"# SDD Design 阶段 Prompt

## 需求规格（spec.md）
{{ spec_content }}

## 存量分析报告
{{ legacy_report }}

## 设计章节要求
1. **需求与存量功能关系分析**：对比需求与现有代码，标注新增/扩展/复用
2. **架构设计**：模块划分与依赖关系
3. **接口设计**：API 签名与数据流
4. **数据模型**：持久化策略与版本化

{% if redo_comment %}
## 修改意见（Redo）
{{ redo_comment }}
{% endif %}

## 铁律约束
- 复用开源核心（sz-rust-ai-facade / sz-rust-mcp / sz-rust-capability）
- 所有 async fn Send + 'static
- 禁止 std::fs
"#;

const TASK_TEMPLATE: &str = r#"# SDD Task 阶段 Prompt

## 技术设计（design.md）
{{ design_content }}

## 任务清单要求
- 每任务含：可执行命令 + 验证方法 + 依赖关系 + 预估工期
- 任务分组：脚手架 → 核心实现 → 测试 → 集成 → 文档
- 每任务标注前置依赖

{% if redo_comment %}
## 修改意见（Redo）
{{ redo_comment }}
{% endif %}

## 铁律约束
- 禁止跳过测试任务
- 每任务必须含验证方法
"#;

const CODING_TEMPLATE: &str = r#"# SDD Coding 阶段 Prompt

## 任务清单（tasks.md）
{{ task_content }}

## 代码生成约束
- 生成代码不得包含 unsafe 块
- 不得用 std::fs
- 不得含 crate 级 #![allow(dead_code)]
- async fn 必须 Send + 'static
- 敏感字段脱敏

{% if redo_comment %}
## 修改意见（Redo）
{{ redo_comment }}
{% endif %}

## MCP 工具白名单
{{ whitelist }}
"#;

pub struct PromptBuilder {
    tera: Tera,
}

impl PromptBuilder {
    pub fn new() -> SddResult<Self> {
        let mut tera = Tera::default();
        tera.add_raw_template("spec", SPEC_TEMPLATE).map_err(|e| {
            SddError::new(crate::error::SddErrorCode::SddStageGenFailed, e.to_string())
        })?;
        tera.add_raw_template("design", DESIGN_TEMPLATE)
            .map_err(|e| {
                SddError::new(crate::error::SddErrorCode::SddStageGenFailed, e.to_string())
            })?;
        tera.add_raw_template("task", TASK_TEMPLATE).map_err(|e| {
            SddError::new(crate::error::SddErrorCode::SddStageGenFailed, e.to_string())
        })?;
        tera.add_raw_template("coding", CODING_TEMPLATE)
            .map_err(|e| {
                SddError::new(crate::error::SddErrorCode::SddStageGenFailed, e.to_string())
            })?;
        Ok(Self { tera })
    }

    pub fn build_spec_prompt(
        &self,
        requirement: &str,
        redo_comment: Option<&str>,
    ) -> SddResult<String> {
        let mut ctx = HashMap::new();
        ctx.insert("requirement", Value::String(requirement.to_string()));
        ctx.insert(
            "redo_comment",
            Value::String(redo_comment.unwrap_or("").to_string()),
        );
        self.render("spec", &ctx)
    }

    pub fn build_design_prompt(
        &self,
        spec_content: &str,
        legacy_report: &str,
        redo_comment: Option<&str>,
    ) -> SddResult<String> {
        let mut ctx = HashMap::new();
        ctx.insert("spec_content", Value::String(spec_content.to_string()));
        ctx.insert("legacy_report", Value::String(legacy_report.to_string()));
        ctx.insert(
            "redo_comment",
            Value::String(redo_comment.unwrap_or("").to_string()),
        );
        self.render("design", &ctx)
    }

    pub fn build_task_prompt(
        &self,
        design_content: &str,
        redo_comment: Option<&str>,
    ) -> SddResult<String> {
        let mut ctx = HashMap::new();
        ctx.insert("design_content", Value::String(design_content.to_string()));
        ctx.insert(
            "redo_comment",
            Value::String(redo_comment.unwrap_or("").to_string()),
        );
        self.render("task", &ctx)
    }

    pub fn build_coding_prompt(
        &self,
        task_content: &str,
        whitelist: &str,
        redo_comment: Option<&str>,
    ) -> SddResult<String> {
        let mut ctx = HashMap::new();
        ctx.insert("task_content", Value::String(task_content.to_string()));
        ctx.insert("whitelist", Value::String(whitelist.to_string()));
        ctx.insert(
            "redo_comment",
            Value::String(redo_comment.unwrap_or("").to_string()),
        );
        self.render("coding", &ctx)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_prompt(
        &self,
        stage: Stage,
        requirement: &str,
        spec_content: &str,
        design_content: &str,
        task_content: &str,
        legacy_report: &str,
        whitelist: &str,
        redo_comment: Option<&str>,
    ) -> SddResult<String> {
        match stage {
            Stage::Spec => self.build_spec_prompt(requirement, redo_comment),
            Stage::Design => self.build_design_prompt(spec_content, legacy_report, redo_comment),
            Stage::Task => self.build_task_prompt(design_content, redo_comment),
            Stage::Coding => self.build_coding_prompt(task_content, whitelist, redo_comment),
            _ => Err(SddError::new(
                crate::error::SddErrorCode::SddStageGenFailed,
                format!("no prompt template for stage {stage}"),
            )),
        }
    }

    fn render(&self, template: &str, ctx: &HashMap<&str, Value>) -> SddResult<String> {
        let context = tera::Context::from_serialize(ctx)?;
        self.tera.render(template, &context).map_err(|e| {
            SddError::new(crate::error::SddErrorCode::SddStageGenFailed, e.to_string())
        })
    }
}

impl Default for PromptBuilder {
    fn default() -> Self {
        Self::new().expect("PromptBuilder initialization failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_builder_spec() {
        let builder = PromptBuilder::new().unwrap();
        let prompt = builder.build_spec_prompt("实现用户登录功能", None).unwrap();
        assert!(prompt.contains("实现用户登录功能"));
        assert!(prompt.contains("EARS 格式约束"));
        assert!(prompt.contains("六章节结构要求"));
    }

    #[test]
    fn prompt_builder_design() {
        let builder = PromptBuilder::new().unwrap();
        let prompt = builder
            .build_design_prompt("# Spec\n", "# Legacy Report\n", None)
            .unwrap();
        assert!(prompt.contains("# Spec"));
        assert!(prompt.contains("# Legacy Report"));
        assert!(prompt.contains("需求与存量功能关系分析"));
    }

    #[test]
    fn prompt_builder_task() {
        let builder = PromptBuilder::new().unwrap();
        let prompt = builder.build_task_prompt("# Design\n", None).unwrap();
        assert!(prompt.contains("# Design"));
        assert!(prompt.contains("任务清单要求"));
    }

    #[test]
    fn prompt_builder_coding() {
        let builder = PromptBuilder::new().unwrap();
        let prompt = builder
            .build_coding_prompt("# Tasks\n", "tool_a,tool_b", None)
            .unwrap();
        assert!(prompt.contains("# Tasks"));
        assert!(prompt.contains("tool_a,tool_b"));
        assert!(prompt.contains("unsafe"));
    }

    #[test]
    fn prompt_builder_redo_comment() {
        let builder = PromptBuilder::new().unwrap();
        let prompt = builder
            .build_spec_prompt("需求", Some("请增加权限校验"))
            .unwrap();
        assert!(prompt.contains("请增加权限校验"));
        assert!(prompt.contains("修改意见"));
    }

    #[test]
    fn prompt_builder_all_stages() {
        let builder = PromptBuilder::new().unwrap();
        for stage in [Stage::Spec, Stage::Design, Stage::Task, Stage::Coding] {
            let prompt = builder
                .build_prompt(
                    stage, "req", "# Spec", "# Design", "# Tasks", "# Legacy", "tool_a", None,
                )
                .unwrap();
            assert!(!prompt.is_empty());
        }
    }
}
