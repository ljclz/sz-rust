//! ORM 模型解析器（stub — 任务组 4 实现）

use std::path::{Path, PathBuf};

use quote::ToTokens;

use crate::error::FrontendCodegenError;
use crate::metadata::ModelMetadata;

/// 模型解析器
pub struct ModelParser;

impl ModelParser {
    /// 解析目录下所有模型
    pub async fn parse_dir(model_dir: &Path) -> Result<Vec<ModelMetadata>, FrontendCodegenError> {
        if !tokio::fs::try_exists(model_dir).await.unwrap_or(false) {
            return Err(FrontendCodegenError::ModelDirNotFound(
                model_dir.display().to_string(),
            ));
        }
        let mut results = Vec::new();
        let mut entries = tokio::fs::read_dir(model_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "rs") {
                if let Some(meta) = Self::parse_file(&path).await? {
                    results.push(meta);
                }
            }
        }
        Ok(results)
    }

    /// 解析单个文件
    pub async fn parse_file(
        file_path: &Path,
    ) -> Result<Option<ModelMetadata>, FrontendCodegenError> {
        let content = tokio::fs::read_to_string(file_path).await?;
        let ast = syn::parse_file(&content).map_err(|e| {
            FrontendCodegenError::ModelParseError(format!("{}: {e}", file_path.display()))
        })?;
        for item in &ast.items {
            if let syn::Item::Struct(s) = item {
                if has_model_derive(&s.attrs) {
                    return Ok(Some(Self::extract_metadata(s, file_path)?));
                }
            }
        }
        Ok(None)
    }

    fn extract_metadata(
        s: &syn::ItemStruct,
        _file_path: &Path,
    ) -> Result<ModelMetadata, FrontendCodegenError> {
        let name = s.ident.to_string();
        let module_name = to_snake_case(&name);
        let table_name = module_name.clone();
        let mut fields = Vec::new();
        for f in &s.fields {
            fields.push(Self::extract_field(f)?);
        }
        Ok(ModelMetadata {
            name,
            table_name,
            module_name,
            fields,
            relations: Vec::new(),
            validations: Vec::new(),
            doc_comment: extract_doc_comment(&s.attrs),
        })
    }

    fn extract_field(
        f: &syn::Field,
    ) -> Result<crate::metadata::FieldMetadata, FrontendCodegenError> {
        let name = f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default();
        let rust_type = quote::quote!(#f).to_string();
        let is_nullable = rust_type.contains("Option <");
        let ts_type = rust_to_ts_type(&rust_type);
        let is_primary_key = has_field_attr(&f.attrs, "pk") || name == "id";
        let is_indexed = has_field_attr(&f.attrs, "index");
        let is_sensitive = is_sensitive_field(&name);
        let is_auto_timestamp = is_auto_timestamp(&name);
        Ok(crate::metadata::FieldMetadata {
            name,
            rust_type,
            ts_type,
            sql_type: String::new(),
            is_nullable,
            is_primary_key,
            is_indexed,
            is_sensitive,
            is_auto_timestamp,
            validation_rules: Vec::new(),
            relation: None,
            doc_comment: extract_doc_comment(&f.attrs),
        })
    }
}

fn has_model_derive(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.path().is_ident("derive") {
            let tokens = attr.meta.require_list().ok().map(|l| l.tokens.to_string());
            tokens.is_some_and(|t| t.contains("Model") || t.contains("Entity"))
        } else {
            false
        }
    })
}

fn has_field_attr(attrs: &[syn::Attribute], ident: &str) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("field") && attr.meta.to_token_stream().to_string().contains(ident)
    })
}

fn extract_doc_comment(attrs: &[syn::Attribute]) -> Option<String> {
    let mut docs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
                {
                    docs.push(s.value().trim().to_string());
                }
            }
        }
    }
    if docs.is_empty() {
        None
    } else {
        Some(docs.join("\n"))
    }
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Rust 类型映射到 TypeScript 类型
pub fn rust_to_ts_type(rust_type: &str) -> String {
    rust_to_ts_type_inner(rust_type, true)
}

fn rust_to_ts_type_inner(rust_type: &str, top_level: bool) -> String {
    let t = rust_type.trim();
    if t.starts_with("Option <") {
        let inner = t
            .trim_start_matches("Option <")
            .trim_end_matches('>')
            .trim();
        return format!("{} | null", rust_to_ts_type_inner(inner, false));
    }
    if t.starts_with("Vec <") {
        let inner = t.trim_start_matches("Vec <").trim_end_matches('>').trim();
        return format!("{}[]", rust_to_ts_type_inner(inner, false));
    }
    match t {
        "String" | "string" | "str" => "string".to_string(),
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize" => {
            "number".to_string()
        }
        "f32" | "f64" => "number".to_string(),
        "bool" => "boolean".to_string(),
        "DateTime" | "NaiveDateTime" => "string".to_string(),
        _ if t.contains("DateTime") => "string".to_string(),
        _ if !top_level && t.chars().next().is_some_and(|c| c.is_uppercase()) => t.to_string(),
        _ => "any".to_string(),
    }
}

/// 判断是否敏感字段
pub fn is_sensitive_field(name: &str) -> bool {
    matches!(
        name,
        "password" | "secret" | "token" | "api_key" | "private_key"
    )
}

/// 判断是否自动时间戳字段
pub fn is_auto_timestamp(name: &str) -> bool {
    matches!(name, "created_at" | "updated_at" | "deleted_at")
}

#[allow(unused)]
fn _unused(_p: PathBuf) {}
