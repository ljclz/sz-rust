//! SZ-Rust Macros — 过程宏包
//!
//! 提供 3 个过程宏：
//!
//! | 宏 | 类型 | 对齐 PHP | 实现阶段 |
//! |----|------|---------|---------|
//! | `#[controller]` | 属性宏 | 控制器声明 | Phase 2 |
//! | `#[model]` | 属性宏 | 模型声明 | Phase 2 |
//! | `compact!` | 函数式宏 | `compact()` | Phase 2.6 ✅ |
//!
//! 骨架实现，属性宏原样透传 TokenStream，`compact!` 生成空 `serde_json::Map`。
//! `compact!` 完整实现，对齐 PHP `compact()`。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use proc_macro::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Ident, Token};

/// `#[controller]` 属性宏骨架
///
/// 原样透传 TokenStream（合法的属性宏最小实现）。
/// TODO: 自动实现 `SzController` trait、注册路由、生成 `postData`/`renderJson` 方法。
///
/// # 示例
///
/// ```ignore
/// #[controller]
/// pub struct UserController;
/// ```
#[proc_macro_attribute]
pub fn controller(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// `#[model]` 属性宏骨架
///
/// 原样透传 TokenStream（合法的属性宏最小实现）。
/// TODO: 自动实现 `BaseModel` trait、生成 `name`/`pk`/`append`/`fillable`/`guarded`/`hidden`。
///
/// # 示例
///
/// ```ignore
/// #[model(table = "sz_user", pk = "user_id")]
/// pub struct User {
///     pub user_id: i64,
///     pub username: String,
/// }
/// ```
#[proc_macro_attribute]
pub fn model(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

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
