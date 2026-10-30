// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! UI 库适配器

use serde::{Deserialize, Serialize};

use crate::config::UiLibrary;
use crate::metadata::ModelMetadata;

/// UI 标签集合
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiTags {
    /// 表格标签
    pub table: &'static str,
    /// 表格列标签
    pub table_column: &'static str,
    /// 表单标签
    pub form: &'static str,
    /// 表单项标签
    pub form_item: &'static str,
    /// 输入框标签
    pub input: &'static str,
    /// 按钮标签
    pub button: &'static str,
    /// 分页标签
    pub pagination: &'static str,
    /// 描述列表标签
    pub descriptions: &'static str,
    /// 描述列表项标签
    pub descriptions_item: &'static str,
    /// 选择器标签
    pub select: &'static str,
    /// 日期选择器标签
    pub date_picker: &'static str,
}

/// UI 适配后的模型
#[derive(Debug, Clone)]
pub struct UiAdaptedModel<'a> {
    /// 模型
    pub model: &'a ModelMetadata,
    /// UI 标签
    pub tags: UiTags,
}

/// UI 适配器
pub struct UiAdapter;

impl UiAdapter {
    /// 适配模型到指定 UI 库
    pub fn adapt(model: &ModelMetadata, ui_library: UiLibrary) -> UiAdaptedModel<'_> {
        let tags = match ui_library {
            UiLibrary::ElementPlus => UiTags {
                table: "el-table",
                table_column: "el-table-column",
                form: "el-form",
                form_item: "el-form-item",
                input: "el-input",
                button: "el-button",
                pagination: "el-pagination",
                descriptions: "el-descriptions",
                descriptions_item: "el-descriptions-item",
                select: "el-select",
                date_picker: "el-date-picker",
            },
            UiLibrary::AntDesignVue => UiTags {
                table: "a-table",
                table_column: "a-table-column",
                form: "a-form",
                form_item: "a-form-item",
                input: "a-input",
                button: "a-button",
                pagination: "a-pagination",
                descriptions: "a-descriptions",
                descriptions_item: "a-descriptions-item",
                select: "a-select",
                date_picker: "a-date-picker",
            },
        };
        UiAdaptedModel { model, tags }
    }
}
