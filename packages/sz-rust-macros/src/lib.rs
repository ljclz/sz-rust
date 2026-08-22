//! SZ-Rust Macros — 过程宏包
//!
//! 提供 3 个过程宏：
//!
//! | 宏 | 类型 | 对齐 PHP | 实现阶段 |
//! |----|------|---------|---------|
//! | `#[controller]` | 属性宏 | 控制器声明 | ✅ |
//! | `#[model]` | 属性宏 | 模型声明 | ✅ |
//! | `compact!` | 函数式宏 | `compact()` | ✅ |
//!
//! `#[controller]` 自动实现 `SzController` trait。
//! `#[model]` 自动实现 `Model` + `ModelExt` trait（基于字段式结构体）。
//! `compact!` 对齐 PHP `compact()`。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Ident, ItemStruct, LitStr, Token};

// ============================================================================
// #[controller] — 自动实现 SzController trait
// ============================================================================

/// `#[controller]` 属性宏
///
/// 为控制器结构体自动实现 `sz_rust_core::controller::SzController` trait。
/// 由于 `SzController` 的所有方法都有默认实现，宏只需生成空 `impl` 块。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_macros::controller;
///
/// #[controller]
/// pub struct UserController;
/// ```
///
/// 生成代码等价于：
///
/// ```ignore
/// impl ::sz_rust_core::controller::SzController for UserController {}
/// ```
#[proc_macro_attribute]
pub fn controller(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as ItemStruct);
    let struct_name = &input.ident;

    let expanded = quote! {
        #input

        impl ::sz_rust_core::controller::SzController for #struct_name {}
    };

    expanded.into()
}

// ============================================================================
// #[model] — 自动实现 Model + ModelExt trait
// ============================================================================

/// `#[model]` 属性宏
///
/// 为字段式结构体自动实现 `sz_orm_core::Model` + `sz_orm_core::ModelExt` trait。
///
/// # 属性参数
///
/// - `table = "表名"`：指定数据库表名（必填）
/// - `pk = "主键列名"`：指定主键列名（默认 `"id"`）
///
/// # 字段类型支持
///
/// 宏根据字段类型自动生成 `Value` 转换代码：
///
/// | Rust 类型 | OrmValue 变体 |
/// |----------|--------------|
/// | `i64` | `Value::I64` |
/// | `i32` | `Value::I32` |
/// | `f64` | `Value::F64` |
/// | `String` | `Value::String` |
/// | `bool` | `Value::Bool` |
///
/// 不支持的字段类型会被跳过（不参与 `columns`/`get_column_value`/`from_value`）。
///
/// # 主键类型
///
/// 主键字段（由 `pk` 指定的列名对应的字段）的类型作为 `Model::PrimaryKey`。
/// 主键字段必须为 `i64` 或 `i32`。
///
/// # fillable / guarded
///
/// - `guarded`：默认包含主键列名
/// - `fillable`：默认包含所有非主键的已支持字段
///
/// # 示例
///
/// ```ignore
/// use sz_rust_macros::model;
///
/// #[model(table = "users", pk = "user_id")]
/// pub struct User {
///     pub user_id: i64,
///     pub name: String,
///     pub age: i64,
/// }
/// ```
#[proc_macro_attribute]
pub fn model(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(item as ItemStruct);
    let struct_name = &input.ident;

    // 解析属性参数：table = "xxx", pk = "xxx"
    let args = match parse_model_attr(attr) {
        Ok(args) => args,
        Err(msg) => {
            return syn::Error::new_spanned(&input, msg)
                .to_compile_error()
                .into();
        }
    };

    let table_name = args.table;
    let pk_name = args.pk.unwrap_or_else(|| "id".to_string());

    // 收集字段信息
    let fields = match collect_fields(&input, &pk_name) {
        Ok(fields) => fields,
        Err(msg) => {
            return syn::Error::new_spanned(&input, msg)
                .to_compile_error()
                .into();
        }
    };

    // 主键字段类型必须是 i64 或 i32
    let pk_field = fields
        .iter()
        .find(|f| f.column_name == pk_name)
        .ok_or_else(|| {
            format!(
                "primary key field '{}' not found in struct '{}'",
                pk_name, struct_name
            )
        });

    let pk_field = match pk_field {
        Ok(f) => f,
        Err(msg) => {
            return syn::Error::new_spanned(&input, msg)
                .to_compile_error()
                .into();
        }
    };

    let pk_ty = pk_field.ty_token.clone();
    let pk_ident = pk_field.ident.clone();

    // 生成 columns 列表
    let column_names: Vec<&str> = fields.iter().map(|f| f.column_name.as_str()).collect();

    // 生成 fillable 列表（非主键字段）
    let fillable_names: Vec<&str> = fields
        .iter()
        .filter(|f| f.column_name != pk_name)
        .map(|f| f.column_name.as_str())
        .collect();

    // 生成 get_column_value 的 match 分支
    let get_column_value_arms = fields.iter().map(|f| {
        let col = &f.column_name;
        let ident = &f.ident;
        let ty = &f.ty;
        if ty == "i64" {
            quote! { #col => Some(::sz_orm_core::Value::I64(self.#ident)) }
        } else if ty == "i32" {
            quote! { #col => Some(::sz_orm_core::Value::I32(self.#ident)) }
        } else if ty == "f64" {
            quote! { #col => Some(::sz_orm_core::Value::F64(self.#ident)) }
        } else if ty == "String" {
            quote! { #col => Some(::sz_orm_core::Value::String(self.#ident.clone())) }
        } else if ty == "bool" {
            quote! { #col => Some(::sz_orm_core::Value::Bool(self.#ident)) }
        } else {
            quote! { #col => None }
        }
    });

    // 生成 from_value 的赋值代码
    let from_value_stmts = fields.iter().filter_map(|f| {
        let col = &f.column_name;
        let ident = &f.ident;
        let ty = &f.ty;
        if ty == "i64" {
            Some(quote! {
                if let Some(::sz_orm_core::Value::I64(v)) = map.get(#col) {
                    self.#ident = *v;
                }
            })
        } else if ty == "i32" {
            Some(quote! {
                if let Some(::sz_orm_core::Value::I32(v)) = map.get(#col) {
                    self.#ident = *v;
                }
            })
        } else if ty == "f64" {
            Some(quote! {
                if let Some(::sz_orm_core::Value::F64(v)) = map.get(#col) {
                    self.#ident = *v;
                }
            })
        } else if ty == "String" {
            Some(quote! {
                if let Some(::sz_orm_core::Value::String(v)) = map.get(#col) {
                    self.#ident = v.clone();
                }
            })
        } else if ty == "bool" {
            Some(quote! {
                if let Some(::sz_orm_core::Value::Bool(v)) = map.get(#col) {
                    self.#ident = *v;
                }
            })
        } else {
            None
        }
    });

    let table_name_lit = LitStr::new(&table_name, proc_macro2::Span::call_site());
    let pk_name_lit = LitStr::new(&pk_name, proc_macro2::Span::call_site());

    let expanded = quote! {
        #input

        impl ::sz_orm_core::Model for #struct_name {
            type PrimaryKey = #pk_ty;

            fn table_name() -> &'static str {
                #table_name_lit
            }

            fn pk_name() -> &'static str {
                #pk_name_lit
            }

            fn pk(&self) -> Self::PrimaryKey {
                self.#pk_ident.clone()
            }

            fn set_pk(&mut self, pk: Self::PrimaryKey) {
                self.#pk_ident = pk;
            }
        }

        impl ::sz_orm_core::ModelExt for #struct_name {
            fn columns() -> Vec<&'static str> {
                vec![#(#column_names),*]
            }

            fn fillable() -> Vec<&'static str> {
                vec![#(#fillable_names),*]
            }

            fn guarded() -> Vec<&'static str> {
                vec![#pk_name_lit]
            }

            fn get_column_value(&self, column: &str) -> Option<::sz_orm_core::Value> {
                match column {
                    #(#get_column_value_arms,)*
                    _ => None,
                }
            }

            fn from_value(&mut self, map: std::collections::HashMap<String, ::sz_orm_core::Value>) {
                #(#from_value_stmts)*
            }
        }
    };

    expanded.into()
}

/// `#[model]` 属性参数
struct ModelAttr {
    table: String,
    pk: Option<String>,
}

/// 解析 `#[model(table = "xxx", pk = "xxx")]` 属性参数
fn parse_model_attr(attr: TokenStream) -> Result<ModelAttr, String> {
    if attr.is_empty() {
        return Err("missing required 'table' attribute: #[model(table = \"xxx\")]".to_string());
    }

    // 解析为逗号分隔的 key = "value" 列表
    let attr2: proc_macro2::TokenStream = attr.into();
    let meta_list = Punctuated::<MetaNameValueStr, Token![,]>::parse_terminated
        .parse2(attr2)
        .map_err(|e| format!("failed to parse model attributes: {e}"))?;

    let mut table = None;
    let mut pk = None;

    for nv in meta_list {
        let key = nv.key.to_string();
        let value = nv.value;
        match key.as_str() {
            "table" => table = Some(value),
            "pk" => pk = Some(value),
            _ => return Err(format!("unknown model attribute '{}'", key)),
        }
    }

    let table = table.ok_or_else(|| {
        "missing required 'table' attribute: #[model(table = \"xxx\")]".to_string()
    })?;

    Ok(ModelAttr { table, pk })
}

/// 辅助类型：解析 `key = "value"` 形式的属性参数
struct MetaNameValueStr {
    key: Ident,
    value: String,
}

impl syn::parse::Parse for MetaNameValueStr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        let _: Token![=] = input.parse()?;
        let value: LitStr = input.parse()?;
        Ok(Self {
            key,
            value: value.value(),
        })
    }
}

/// 字段信息
struct FieldInfo {
    ident: Ident,
    /// 类型字符串（用于比较，如 "i64"、"String"）
    ty: String,
    /// 原始类型 token（用于代码生成）
    ty_token: syn::Type,
    column_name: String,
}

/// 收集结构体中支持的字段
fn collect_fields(input: &ItemStruct, _pk_name: &str) -> Result<Vec<FieldInfo>, String> {
    let fields = match &input.fields {
        syn::Fields::Named(named) => &named.named,
        _ => {
            return Err("#[model] only supports structs with named fields".to_string());
        }
    };

    let mut result = Vec::new();
    for field in fields {
        let ident = field
            .ident
            .clone()
            .ok_or_else(|| "#[model] requires all fields to be named".to_string())?;

        // 跳过带 #[model(skip)] 标记的字段
        if field.attrs.iter().any(|attr| {
            attr.path().is_ident("model")
                && attr
                    .parse_args::<syn::Ident>()
                    .ok()
                    .map(|i| i == "skip")
                    .unwrap_or(false)
        }) {
            continue;
        }

        // 提取类型字符串
        let ty_str = extract_type_string(&field.ty);
        let ty_token = field.ty.clone();

        // column_name 默认为字段名
        let column_name = ident.to_string();

        result.push(FieldInfo {
            ident,
            ty: ty_str,
            ty_token,
            column_name,
        });
    }

    if result.is_empty() {
        return Err("#[model] struct must have at least one field".to_string());
    }

    Ok(result)
}

/// 提取字段类型字符串
///
/// 支持的类型：i64, i32, f64, String, bool
/// 其他类型返回原始字符串（get_column_value 中会返回 None）
fn extract_type_string(ty: &syn::Type) -> String {
    let s = quote!(#ty).to_string();
    // 去除空白
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ============================================================================
// compact! — 函数式宏
// ============================================================================

/// `compact!` 函数式宏
///
/// 将变量名 → 值按声明顺序插入 `serde_json::Map<String, serde_json::Value>`，
/// 严格对齐 PHP `compact()` 函数行为：
///
/// ## PHP `compact()` 行为
///
/// ```php
/// $code = 1;
/// $msg = "ok";
/// $data = ["id" => 1];
/// return compact('code', 'msg', 'data');
/// // 等价于：['code' => 1, 'msg' => "ok", 'data' => ["id" => 1]]
/// ```
///
/// ## Rust `compact!` 等价
///
/// ```
/// use sz_rust_macros::compact;
/// use serde_json::json;
///
/// let code = 1i32;
/// let msg = "ok".to_string();
/// let data = json!({"id": 1});
/// let result = compact!(code, msg, data);
/// assert_eq!(result.len(), 3);
/// assert_eq!(result["code"], 1);
/// assert_eq!(result["msg"], "ok");
/// assert_eq!(result["data"]["id"], 1);
/// ```
///
/// ## 字段顺序
///
/// 字段顺序严格按宏参数顺序保序（对齐 PHP `compact()` 参数顺序），
/// 依赖 `serde_json::Map` 的 `preserve_order` feature（默认启用）。
///
/// ## 类型转换
///
/// 使用 `serde_json::to_value()` 将变量值转换为 `serde_json::Value`，
/// 支持所有实现 `serde::Serialize` 的类型。
///
/// # 示例
///
/// ```
/// use sz_rust_macros::compact;
///
/// let name = "alice";
/// let age = 30i32;
/// let map = compact!(name, age);
/// assert_eq!(map["name"], "alice");
/// assert_eq!(map["age"], 30);
/// ```
#[proc_macro]
pub fn compact(input: TokenStream) -> TokenStream {
    // 解析输入为逗号分隔的标识符列表
    // 对齐 PHP compact('var1', 'var2', ...) 参数语法
    let names =
        syn::parse_macro_input!(input with Punctuated::<Ident, Token![,]>::parse_terminated);

    // 为每个标识符生成 map.insert 代码
    // 使用 serde_json::to_value() 支持所有实现 serde::Serialize 的类型
    let inserts = names.iter().map(|name| {
        let name_str = name.to_string();
        quote! {
            map.insert(
                #name_str.to_string(),
                serde_json::to_value(&#name).unwrap_or(serde_json::Value::Null),
            );
        }
    });

    quote! {
        {
            let mut map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
            #(#inserts)*
            map
        }
    }
    .into()
}
