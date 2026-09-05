// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 路由生成器

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::{Framework, GenerationConfig};
use crate::error::FrontendCodegenError;
use crate::report::GeneratedFile;
use crate::template_engine::CodegenTemplateEngine;

/// 页面类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageType {
    /// 列表页
    List,
    /// 详情页
    Show,
    /// 新建页
    Create,
    /// 编辑页
    Edit,
}

/// 路由页面映射器
pub struct RoutePageMapper;

impl RoutePageMapper {
    /// 根据 HTTP 方法与路径映射页面类型
    pub fn map(method: &str, has_path_param: bool) -> Option<PageType> {
        match method {
            "GET" if has_path_param => Some(PageType::Show),
            "GET" => Some(PageType::List),
            "POST" => Some(PageType::Create),
            "PUT" | "PATCH" => Some(PageType::Edit),
            _ => None,
        }
    }
}

/// 前端路由
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendRoute {
    /// 路径
    pub path: String,
    /// 名称
    pub name: String,
    /// 组件路径
    pub component: String,
    /// 元信息
    pub meta: RouteMeta,
    /// 子路由
    pub children: Vec<FrontendRoute>,
}

/// 路由元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteMeta {
    /// 标题
    pub title: Option<String>,
    /// 权限码
    pub permission: Option<String>,
    /// 是否懒加载
    pub lazy: bool,
}

/// 路由生成器
pub struct RouteGenerator<'a> {
    engine: &'a CodegenTemplateEngine,
}

impl<'a> RouteGenerator<'a> {
    /// 创建生成器
    pub fn new(engine: &'a CodegenTemplateEngine) -> Self {
        Self { engine }
    }

    /// 生成路由文件
    pub async fn generate(
        &self,
        routes: &[FrontendRoute],
        config: &GenerationConfig,
    ) -> Result<GeneratedFile, FrontendCodegenError> {
        let mut context = tera::Context::new();
        context.insert("routes", routes);
        context.insert("lazy_load", &config.lazy_load);

        let (tmpl, output) = match config.framework {
            Framework::Vue => ("router/routes.ts.tera", "src/router/routes.ts"),
            Framework::React => ("router/routes.tsx.tera", "src/router/routes.tsx"),
        };
        let content = self.engine.render(tmpl, &context)?;
        Ok(GeneratedFile {
            path: std::path::PathBuf::from(output),
            size_bytes: content.len() as u64,
            source_model: "router".to_string(),
            source_template: tmpl.to_string(),
            is_overwritten: false,
        })
    }
}

/// 按路径前缀分组路由
pub fn group_by_prefix(routes: &[FrontendRoute]) -> BTreeMap<String, Vec<&FrontendRoute>> {
    let mut groups = BTreeMap::new();
    for r in routes {
        let prefix = r.path.split('/').nth(1).unwrap_or("").to_string();
        groups.entry(prefix).or_insert_with(Vec::new).push(r);
    }
    groups
}
