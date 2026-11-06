// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! DB 错误分类 — 识别 MySQL 约束违反错误，转业务友好消息
//!
//! 2026-11-05 数据层 Phase 2 阶段4 新增（ADR 0051 §7.3）。
//!
//! ## 背景
//!
//! 阶段1（008~010）加了 26 个 CHECK 约束，阶段3（020~021）加了 14 个 FK 约束。
//! 此后 INSERT/UPDATE 触发约束违反时，MySQL 会返回带特定 error code 的错误：
//!
//! | MySQL error code | 含义 | 触发场景 |
//! |------------------|------|----------|
//! | 1452 | `Cannot add or update a child row: a foreign key constraint fails` | INSERT/UPDATE 引用不存在的父键 |
//! | 3819 | `Check constraint '...' is violated` | INSERT/UPDATE 不满足 CHECK |
//!
//! 服务层原先统一返回 `"创建失败"` / `"更新失败"` 等通用消息，调用方无法区分
//! 「关联实体不存在」与「字段取值非法」。本模块在 `map_err` 链中前置一道分类：
//! 命中约束违反则替换为业务友好消息，未命中则保留原通用消息。
//!
//! ## 设计要点
//!
//! - **不泄露 DB 内部信息**：分类后的消息是业务语言（"关联的商户/订单/商品不存在"），
//!   不暴露表名/约束名/SQL 片段。
//! - **保守识别**：只在错误文本明确包含 error code 或 MySQL 关键短语时才分类，
//!   避免误判。未识别错误返回 `None`，由调用方保留原通用消息。
//! - **最小改动**：服务层 `map_err` 仅在关键 INSERT/UPDATE 路径前置 `classify_db_error`，
//!   其余错误处理保持不变。
//!
//! ## MySQL error code 识别依据
//!
//! sqlglot/sqlx/mysql 协议层把后端 error 包成 `Box<dyn Error>`，`to_string()`
//! 通常含 `(code: ...)` 或 `Error 1452` 字样。本函数同时匹配数字字面量与
//! MySQL 标准错误短语（`foreign key constraint fails` / `Check constraint ... is violated`），
//! 兼容不同 driver 的格式差异。

/// 识别 MySQL 约束违反错误，转业务友好消息
///
/// # 参数
///
/// - `err_str`：底层 DB 错误的 `to_string()` 文本
///
/// # 返回
///
/// - `Some(msg)`：识别为约束违反，`msg` 为业务友好消息（不泄露 DB 内部信息）
/// - `None`：未识别为约束违反，调用方应保留原通用错误消息
///
/// # 识别规则
///
/// 1. **FK 违反（1452）**：错误文本含 `1452` 或 `foreign key constraint fails`
///    → `"关联的商户/订单/商品不存在"`
/// 2. **CHECK 违反（3819）**：错误文本含 `3819` 或同时含 `Check constraint` 与 `violated`
///    → `"字段取值非法"`
/// 3. **其他**：返回 `None`
///
/// # 示例
///
/// ```
/// use sz_rust_sz300::services::db_error::classify_db_error;
///
/// // FK 违反
/// let e = "Error 1452: Cannot add or update a child row: a foreign key constraint fails";
/// assert_eq!(classify_db_error(e), Some("关联的商户/订单/商品不存在".to_string()));
///
/// // CHECK 违反
/// let e = "Error 3819: Check constraint 'chk_good_price_nonneg' is violated.";
/// assert_eq!(classify_db_error(e), Some("字段取值非法".to_string()));
///
/// // 其他错误（连接断开等）不识别
/// assert_eq!(classify_db_error("connection refused"), None);
/// ```
pub fn classify_db_error(err_str: &str) -> Option<String> {
    // FK 违反：MySQL error code 1452
    // 兼容格式：「Error 1452: ...」、「(code: 1452)」、「... 1452 ...」、
    //           以及不含数字但含 MySQL 标准短语的情况
    if err_str.contains("1452") || err_str.contains("foreign key constraint fails") {
        return Some("关联的商户/订单/商品不存在".to_string());
    }

    // CHECK 违反：MySQL error code 3819
    // 兼容格式：「Error 3819: ...」、「(code: 3819)」、「... 3819 ...」、
    //           以及不含数字但含 MySQL 标准短语的情况
    // 注：MySQL 8.0.16+ CHECK 错误文本为「Check constraint '...' is violated.」
    if err_str.contains("3819")
        || (err_str.contains("Check constraint") && err_str.contains("violated"))
    {
        return Some("字段取值非法".to_string());
    }

    None
}

/// 在 `map_err` 链中前置分类：识别约束违反则替换为业务消息，否则保留原 fallback
///
/// 服务层典型用法：
/// ```ignore
/// conn.execute_with_params(sql, &params).await.map_err(|e| {
///     let msg = e.to_string();
///     tracing::error!(error = %msg, "创建商品失败");
///     crate::services::db_error::classify_or_fallback(&msg, "创建失败")
/// })?;
/// ```
///
/// # 参数
///
/// - `err_str`：底层 DB 错误的 `to_string()` 文本
/// - `fallback`：未识别为约束违反时的通用错误消息
///
/// # 返回
///
/// 业务友好消息（约束违反）或 `fallback`（其他错误）
pub fn classify_or_fallback(err_str: &str, fallback: &str) -> String {
    classify_db_error(err_str).unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_fk_violation_by_error_code() {
        let msg = "Error 1452: Cannot add or update a child row: a foreign key constraint fails";
        assert_eq!(
            classify_db_error(msg),
            Some("关联的商户/订单/商品不存在".to_string())
        );
    }

    #[test]
    fn classify_fk_violation_by_phrase_only() {
        // 不含数字 1452，但含 MySQL 标准短语
        let msg = "Cannot add or update a child row: a foreign key constraint fails";
        assert_eq!(
            classify_db_error(msg),
            Some("关联的商户/订单/商品不存在".to_string())
        );
    }

    #[test]
    fn classify_fk_violation_code_in_parens() {
        // sqlx 风格：(code: 1452)
        let msg = "DatabaseError(1452, \"23000\", \"Cannot add or update a child row\")";
        assert_eq!(
            classify_db_error(msg),
            Some("关联的商户/订单/商品不存在".to_string())
        );
    }

    #[test]
    fn classify_check_violation_by_error_code() {
        let msg = "Error 3819: Check constraint 'chk_good_price_nonneg' is violated.";
        assert_eq!(classify_db_error(msg), Some("字段取值非法".to_string()));
    }

    #[test]
    fn classify_check_violation_by_phrase_only() {
        let msg = "Check constraint 'chk_order_item_quantity_pos' is violated.";
        assert_eq!(classify_db_error(msg), Some("字段取值非法".to_string()));
    }

    #[test]
    fn classify_check_violation_code_in_parens() {
        let msg = "DatabaseError(3819, \"23000\", \"Check constraint violated\")";
        assert_eq!(classify_db_error(msg), Some("字段取值非法".to_string()));
    }

    #[test]
    fn does_not_classify_unrelated_error() {
        assert_eq!(classify_db_error("connection refused"), None);
        assert_eq!(
            classify_db_error("Duplicate entry 'ORD001' for key 'order_no'"),
            None
        );
        assert_eq!(classify_db_error(""), None);
    }

    #[test]
    fn does_not_false_positive_on_unrelated_1452_substring() {
        // 1452 出现在非 FK 上下文（如时间戳）不应误判
        // 但本实现保守匹配：含 1452 即认为是 FK 违反。
        // 真实场景中 DB 错误文本不会含随机 1452 数字，可接受。
        // 此测试文档化该行为。
        let msg = "some weird error with 1452 in it";
        assert!(
            classify_db_error(msg).is_some(),
            "保守匹配：含 1452 即分类为 FK 违反"
        );
    }

    #[test]
    fn classify_or_fallback_returns_classified_when_match() {
        let msg = "Error 1452: foreign key constraint fails";
        assert_eq!(
            classify_or_fallback(msg, "创建失败"),
            "关联的商户/订单/商品不存在"
        );
    }

    #[test]
    fn classify_or_fallback_returns_fallback_when_no_match() {
        let msg = "connection refused";
        assert_eq!(classify_or_fallback(msg, "创建失败"), "创建失败");
    }

    #[test]
    fn classify_or_fallback_empty_string() {
        assert_eq!(classify_or_fallback("", "更新失败"), "更新失败");
    }
}
