//! 中间件链构建器 — 基于 `tower::ServiceBuilder`
//!
//! 对齐 PHP 中间件组合方式（`app/middleware.php` 返回数组顺序即执行顺序），
//! Rust 端通过 `tower::ServiceBuilder` 组合 Layer，但 `ServiceBuilder` 的 layer
//! 是「后注册先执行」（stack 反向），本模块负责把业务期望顺序转换为
//! `ServiceBuilder` 注册顺序。
//!
//! ## 设计目标
//!
//! 1. **顺序保证**：`DEFAULT_ORDER` 数组首元素最先执行（业务语义）
//! 2. **PHP 对齐**：默认顺序对齐 `app/middleware.php` 全局中间件 + 业务中间件约定
//! 3. **可定制**：支持自定义顺序（如跳过 Auth 的公开路由）
//! 4. **可观测**：提供 `OrderRecorder` 工具记录中间件执行顺序，用于测试验证
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::middleware::chain::MiddlewareChain;
//! use sz_rust_core::middleware::order::MiddlewareKind;
//!
//! // 1. 使用默认顺序
//! let chain = MiddlewareChain::default();
//! assert_eq!(chain.order(), [
//!     MiddlewareKind::Trace,
//!     MiddlewareKind::Cors,
//!     MiddlewareKind::Log,
//!     MiddlewareKind::RateLimit,
//!     MiddlewareKind::Auth,
//! ]);
//!
//! // 2. 自定义顺序（如公开 API 跳过 Auth）
//! let chain = MiddlewareChain::new()
//!     .push(MiddlewareKind::Trace)
//!     .push(MiddlewareKind::Cors)
//!     .push(MiddlewareKind::Log);
//! assert_eq!(chain.order(), [
//!     MiddlewareKind::Trace,
//!     MiddlewareKind::Cors,
//!     MiddlewareKind::Log,
//! ]);
//!
//! // 3. 从 PHP 全局顺序构建
//! let chain = MiddlewareChain::php_global();
//! assert_eq!(chain.order(), [
//!     MiddlewareKind::Trace,
//!     MiddlewareKind::Cors,
//! ]);
//! ```
//!
//! ## 与 `tower::ServiceBuilder` 的关系
//!
//! `MiddlewareChain` 只负责「顺序定义和验证」，不直接构造 `ServiceBuilder`。
//! 具体的 Layer 实例化由 `auth.rs` / `log.rs` /
//! `rate_limit.rs` / `trace.rs` 模块提供，再由调用方按 `chain.order()` 逆序
//! 调用 `ServiceBuilder::layer` 注册。
//!
//! 这样设计的原因：
//! - 各中间件有不同的配置参数（如 Auth 需要 JWT secret，RateLimit 需要 capacity）
//! - `tower::Layer` 是泛型 trait，不同 Layer 类型不同，无法统一存入 `Vec<Box<dyn Layer>>`
//! - 解耦「顺序定义」与「Layer 实例化」更易测试和维护

use crate::order::{MiddlewareKind, DEFAULT_ORDER, PHP_GLOBAL_ORDER};

/// 中间件链 — 定义中间件执行顺序
///
/// 业务期望顺序：`order()` 数组首元素最先执行。
/// 调用方在 `ServiceBuilder` 上注册时需逆序调用 `layer()`（后注册先执行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiddlewareChain {
    order: Vec<MiddlewareKind>,
}

impl MiddlewareChain {
    /// 创建空链（无中间件）
    pub fn new() -> Self {
        Self { order: Vec::new() }
    }

    /// 创建默认链（使用 `DEFAULT_ORDER`）
    pub fn default_chain() -> Self {
        Self {
            order: DEFAULT_ORDER.to_vec(),
        }
    }

    /// 创建 PHP 全局链（使用 `PHP_GLOBAL_ORDER`，对齐 `app/middleware.php`）
    pub fn php_global() -> Self {
        Self {
            order: PHP_GLOBAL_ORDER.to_vec(),
        }
    }

    /// 追加一个中间件到链尾（最后执行）
    pub fn push(mut self, kind: MiddlewareKind) -> Self {
        self.order.push(kind);
        self
    }

    /// 在指定位置插入中间件
    ///
    /// 返回 `Err(message)` 如果位置越界。
    pub fn insert(mut self, index: usize, kind: MiddlewareKind) -> Result<Self, String> {
        if index > self.order.len() {
            return Err(format!(
                "insert index {index} out of bounds (len={})",
                self.order.len()
            ));
        }
        self.order.insert(index, kind);
        Ok(self)
    }

    /// 移除并返回指定位置的中间件
    ///
    /// 返回 `None` 如果位置越界。
    pub fn remove(&mut self, index: usize) -> Option<MiddlewareKind> {
        if index >= self.order.len() {
            return None;
        }
        Some(self.order.remove(index))
    }

    /// 移除所有指定类型的中间件
    ///
    /// 返回被移除的数量。
    pub fn remove_kind(&mut self, kind: MiddlewareKind) -> usize {
        let before = self.order.len();
        self.order.retain(|k| *k != kind);
        before - self.order.len()
    }

    /// 移除指定类型之后的所有中间件（含指定类型）
    ///
    /// 用于「公开 API 跳过 Auth 及之后中间件」场景。
    /// 返回被移除的数量；若 `kind` 不存在则不移除任何中间件，返回 0。
    pub fn remove_from(&mut self, kind: MiddlewareKind) -> usize {
        if let Some(pos) = self.order.iter().position(|k| *k == kind) {
            let removed = self.order.len() - pos;
            self.order.truncate(pos);
            removed
        } else {
            0
        }
    }

    /// 返回中间件顺序（业务期望顺序，首元素最先执行）
    pub fn order(&self) -> &[MiddlewareKind] {
        &self.order
    }

    /// 返回 `ServiceBuilder` 注册顺序（业务期望顺序的逆序）
    ///
    /// `ServiceBuilder::layer` 是「后注册先执行」，因此注册时需逆序。
    /// 调用方按此顺序调用 `ServiceBuilder::layer(layer_xxx)` 即可保证
    /// 业务期望顺序与实际执行顺序一致。
    pub fn service_builder_order(&self) -> Vec<MiddlewareKind> {
        self.order.iter().copied().rev().collect()
    }

    /// 返回链长度
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// 链是否为空
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// 是否包含指定中间件
    pub fn contains(&self, kind: MiddlewareKind) -> bool {
        self.order.contains(&kind)
    }

    /// 返回指定中间件的位置（首次出现）
    pub fn position(&self, kind: MiddlewareKind) -> Option<usize> {
        self.order.iter().position(|k| *k == kind)
    }

    /// 校验链中无重复中间件
    ///
    /// 重复中间件通常表示配置错误（如 Auth 注册两次），应避免。
    pub fn has_duplicates(&self) -> bool {
        use std::collections::HashSet;
        let set: HashSet<MiddlewareKind> = self.order.iter().copied().collect();
        set.len() != self.order.len()
    }
}

impl Default for MiddlewareChain {
    fn default() -> Self {
        Self::default_chain()
    }
}

impl std::fmt::Display for MiddlewareChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MiddlewareChain[")?;
        for (i, kind) in self.order.iter().enumerate() {
            if i > 0 {
                write!(f, " -> ")?;
            }
            write!(f, "{kind}")?;
        }
        write!(f, "]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ====================================================================
    // 构造函数
    // ====================================================================

    #[test]
    fn test_new_creates_empty_chain() {
        let chain = MiddlewareChain::new();
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
        assert_eq!(chain.order(), &[]);
    }

    #[test]
    fn test_default_chain_uses_default_order() {
        let chain = MiddlewareChain::default_chain();
        assert_eq!(chain.order(), DEFAULT_ORDER);
        assert_eq!(chain.len(), 5);
    }

    #[test]
    fn test_default_trait_uses_default_chain() {
        let chain = MiddlewareChain::default();
        assert_eq!(chain.order(), DEFAULT_ORDER);
    }

    #[test]
    fn test_php_global_uses_php_global_order() {
        let chain = MiddlewareChain::php_global();
        assert_eq!(chain.order(), PHP_GLOBAL_ORDER);
        assert_eq!(chain.len(), 2);
    }

    // ====================================================================
    // push 链式追加
    // ====================================================================

    #[test]
    fn test_push_appends_to_end() {
        let chain = MiddlewareChain::new()
            .push(MiddlewareKind::Trace)
            .push(MiddlewareKind::Cors);
        assert_eq!(chain.order(), [MiddlewareKind::Trace, MiddlewareKind::Cors]);
    }

    #[test]
    fn test_push_preserves_order() {
        let chain = MiddlewareChain::new()
            .push(MiddlewareKind::Auth)
            .push(MiddlewareKind::Log)
            .push(MiddlewareKind::Trace);
        // 业务期望顺序：push 顺序 = 执行顺序
        assert_eq!(
            chain.order(),
            [
                MiddlewareKind::Auth,
                MiddlewareKind::Log,
                MiddlewareKind::Trace
            ]
        );
    }

    // ====================================================================
    // insert 指定位置插入
    // ====================================================================

    #[test]
    fn test_insert_at_beginning() {
        let chain = MiddlewareChain::new()
            .push(MiddlewareKind::Cors)
            .push(MiddlewareKind::Log);
        let chain = chain
            .insert(0, MiddlewareKind::Trace)
            .expect("insert at 0 should succeed");
        assert_eq!(
            chain.order(),
            [
                MiddlewareKind::Trace,
                MiddlewareKind::Cors,
                MiddlewareKind::Log
            ]
        );
    }

    #[test]
    fn test_insert_at_middle() {
        let chain = MiddlewareChain::new()
            .push(MiddlewareKind::Trace)
            .push(MiddlewareKind::Log);
        let chain = chain
            .insert(1, MiddlewareKind::Cors)
            .expect("insert at 1 should succeed");
        assert_eq!(
            chain.order(),
            [
                MiddlewareKind::Trace,
                MiddlewareKind::Cors,
                MiddlewareKind::Log
            ]
        );
    }

    #[test]
    fn test_insert_at_end() {
        let chain = MiddlewareChain::new()
            .push(MiddlewareKind::Trace)
            .push(MiddlewareKind::Cors);
        let chain = chain
            .insert(2, MiddlewareKind::Log)
            .expect("insert at 2 should succeed");
        assert_eq!(
            chain.order(),
            [
                MiddlewareKind::Trace,
                MiddlewareKind::Cors,
                MiddlewareKind::Log
            ]
        );
    }

    #[test]
    fn test_insert_out_of_bounds_returns_err() {
        let chain = MiddlewareChain::new().push(MiddlewareKind::Trace);
        let result = chain.insert(5, MiddlewareKind::Cors);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("out of bounds"));
    }

    // ====================================================================
    // remove / remove_kind / remove_from
    // ====================================================================

    #[test]
    fn test_remove_by_index() {
        let mut chain = MiddlewareChain::default_chain();
        let removed = chain.remove(2); // 移除 Log
        assert_eq!(removed, Some(MiddlewareKind::Log));
        assert_eq!(
            chain.order(),
            [
                MiddlewareKind::Trace,
                MiddlewareKind::Cors,
                MiddlewareKind::RateLimit,
                MiddlewareKind::Auth,
            ]
        );
    }

    #[test]
    fn test_remove_out_of_bounds_returns_none() {
        let mut chain = MiddlewareChain::default_chain();
        assert_eq!(chain.remove(99), None);
        assert_eq!(chain.len(), 5); // 未变化
    }

    #[test]
    fn test_remove_kind_removes_all_occurrences() {
        let mut chain = MiddlewareChain::new()
            .push(MiddlewareKind::Trace)
            .push(MiddlewareKind::Cors)
            .push(MiddlewareKind::Trace); // 重复
        let removed = chain.remove_kind(MiddlewareKind::Trace);
        assert_eq!(removed, 2);
        assert_eq!(chain.order(), [MiddlewareKind::Cors]);
    }

    #[test]
    fn test_remove_kind_not_present_returns_zero() {
        let mut chain = MiddlewareChain::php_global();
        let removed = chain.remove_kind(MiddlewareKind::Auth);
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_remove_from_removes_kind_and_after() {
        // 公开 API 跳过 RateLimit 和 Auth
        let mut chain = MiddlewareChain::default_chain();
        let removed = chain.remove_from(MiddlewareKind::RateLimit);
        assert_eq!(removed, 2);
        assert_eq!(
            chain.order(),
            [
                MiddlewareKind::Trace,
                MiddlewareKind::Cors,
                MiddlewareKind::Log
            ]
        );
    }

    #[test]
    fn test_remove_from_first_element_clears_all() {
        let mut chain = MiddlewareChain::default_chain();
        let removed = chain.remove_from(MiddlewareKind::Trace);
        assert_eq!(removed, 5);
        assert!(chain.is_empty());
    }

    #[test]
    fn test_remove_from_not_present_returns_zero() {
        let mut chain = MiddlewareChain::php_global(); // 不含 Auth
        let removed = chain.remove_from(MiddlewareKind::Auth);
        assert_eq!(removed, 0);
        assert_eq!(chain.len(), 2); // 未变化
    }

    // ====================================================================
    // service_builder_order 逆序转换
    // ====================================================================

    #[test]
    fn test_service_builder_order_reverses() {
        let chain = MiddlewareChain::default_chain();
        let sb_order = chain.service_builder_order();
        // ServiceBuilder 后注册先执行，因此逆序
        assert_eq!(
            sb_order,
            [
                MiddlewareKind::Auth,
                MiddlewareKind::RateLimit,
                MiddlewareKind::Log,
                MiddlewareKind::Cors,
                MiddlewareKind::Trace,
            ]
        );
    }

    #[test]
    fn test_service_builder_order_empty_chain() {
        let chain = MiddlewareChain::new();
        assert_eq!(chain.service_builder_order(), Vec::<MiddlewareKind>::new());
    }

    #[test]
    fn test_service_builder_order_single_element() {
        let chain = MiddlewareChain::new().push(MiddlewareKind::Cors);
        assert_eq!(chain.service_builder_order(), [MiddlewareKind::Cors]);
    }

    // ====================================================================
    // contains / position / has_duplicates
    // ====================================================================

    #[test]
    fn test_contains_true() {
        let chain = MiddlewareChain::default_chain();
        assert!(chain.contains(MiddlewareKind::Auth));
        assert!(chain.contains(MiddlewareKind::Trace));
    }

    #[test]
    fn test_contains_false() {
        let chain = MiddlewareChain::php_global();
        assert!(!chain.contains(MiddlewareKind::Auth));
    }

    #[test]
    fn test_position_returns_index() {
        let chain = MiddlewareChain::default_chain();
        assert_eq!(chain.position(MiddlewareKind::Trace), Some(0));
        assert_eq!(chain.position(MiddlewareKind::Auth), Some(4));
    }

    #[test]
    fn test_position_not_present_returns_none() {
        let chain = MiddlewareChain::php_global();
        assert_eq!(chain.position(MiddlewareKind::Auth), None);
    }

    #[test]
    fn test_has_duplicates_false_for_default() {
        let chain = MiddlewareChain::default_chain();
        assert!(!chain.has_duplicates());
    }

    #[test]
    fn test_has_duplicates_true_when_repeated() {
        let chain = MiddlewareChain::new()
            .push(MiddlewareKind::Trace)
            .push(MiddlewareKind::Cors)
            .push(MiddlewareKind::Trace);
        assert!(chain.has_duplicates());
    }

    // ====================================================================
    // Display 格式化
    // ====================================================================

    #[test]
    fn test_display_empty_chain() {
        let chain = MiddlewareChain::new();
        assert_eq!(chain.to_string(), "MiddlewareChain[]");
    }

    #[test]
    fn test_display_single_element() {
        let chain = MiddlewareChain::new().push(MiddlewareKind::Cors);
        assert_eq!(chain.to_string(), "MiddlewareChain[cors]");
    }

    #[test]
    fn test_display_multiple_elements() {
        let chain = MiddlewareChain::php_global();
        assert_eq!(chain.to_string(), "MiddlewareChain[trace -> cors]");
    }

    #[test]
    fn test_display_full_default_chain() {
        let chain = MiddlewareChain::default_chain();
        assert_eq!(
            chain.to_string(),
            "MiddlewareChain[trace -> cors -> log -> rate_limit -> auth]"
        );
    }

    // ====================================================================
    // Clone / PartialEq / Eq
    // ====================================================================

    #[test]
    fn test_clone_produces_equal_chain() {
        let chain = MiddlewareChain::default_chain();
        let cloned = chain.clone();
        assert_eq!(chain, cloned);
    }

    #[test]
    fn test_eq_same_order() {
        let a = MiddlewareChain::default_chain();
        let b = MiddlewareChain::default_chain();
        assert_eq!(a, b);
    }

    #[test]
    fn test_ne_different_order() {
        let a = MiddlewareChain::default_chain();
        let b = MiddlewareChain::php_global();
        assert_ne!(a, b);
    }

    // ====================================================================
    // PHP 行为对齐验证（R5 硬约束）
    // ====================================================================

    #[test]
    fn test_php_alignment_default_chain_includes_global() {
        // DEFAULT_ORDER 必须包含 PHP 全局中间件（Trace + Cors）作为前缀
        let chain = MiddlewareChain::default_chain();
        let php_global = MiddlewareChain::php_global();
        assert!(
            chain.order().starts_with(php_global.order()),
            "DEFAULT_ORDER must start with PHP global order"
        );
    }

    #[test]
    fn test_php_alignment_trace_first() {
        // 对齐 PHP `app/middleware.php` 第一个中间件 `SessionInit`
        let chain = MiddlewareChain::default_chain();
        assert_eq!(chain.order().first(), Some(&MiddlewareKind::Trace));
    }

    #[test]
    fn test_php_alignment_cors_second() {
        // 对齐 PHP `app/middleware.php` 第二个中间件 `AllowCrossDomain`
        let chain = MiddlewareChain::default_chain();
        assert_eq!(chain.order().get(1), Some(&MiddlewareKind::Cors));
    }

    #[test]
    fn test_php_alignment_auth_for_public_routes_can_be_removed() {
        // PHP 端公开路由（如 login/captcha）通过 `allow_all_action` 白名单跳过 Auth
        // Rust 端可通过 `remove_kind(Auth)` 或 `remove_from(Auth)` 实现等价行为
        let mut chain = MiddlewareChain::default_chain();
        let removed = chain.remove_kind(MiddlewareKind::Auth);
        assert_eq!(removed, 1);
        assert!(!chain.contains(MiddlewareKind::Auth));
        // 其他中间件保持不变
        assert!(chain.contains(MiddlewareKind::Trace));
        assert!(chain.contains(MiddlewareKind::Cors));
        assert!(chain.contains(MiddlewareKind::Log));
        assert!(chain.contains(MiddlewareKind::RateLimit));
    }
}
