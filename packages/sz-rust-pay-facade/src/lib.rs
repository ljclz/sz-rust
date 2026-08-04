//! SZ-Rust Pay Facade
//!
//! 提取自 `sz-rust-core` 的支付聚合抽象层模块，提供统一的支付抽象，
//! 支持多平台（支付宝、微信支付等）扩展。
//!
//! ## PHP 对齐
//!
//! 对齐 [`yansongda/pay`](https://github.com/yansongda/pay)：
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Pay::alipay()->app($order)` | [`pay::PayProvider::pay`] | 发起支付 |
//! | `Pay::alipay()->find($order)` | [`pay::PayProvider::query`] | 查询订单 |
//! | `Pay::alipay()->close($order)` | [`pay::PayProvider::close`] | 关闭订单 |
//! | `Pay::alipay()->refund($order)` | [`pay::PayProvider::refund`] | 退款 |
//! | `Pay::alipay()->callback($params)` | [`pay::PayProvider::verify_notify`] | 验证回调 |
//!
//! ## 模块结构
//!
//! | 类型 | 说明 |
//! |------|------|
//! | [`pay::PayProvider`] | 支付提供商 trait（核心抽象） |
//! | [`pay::MemoryPayProvider`] | 内存实现（测试/开发用） |
//! | [`pay::PayOrder`] | 支付订单 Builder |
//! | [`pay::RefundOrder`] | 退款订单 Builder |
//! | [`pay::PayConfig`] | 支付配置 Builder |
//! | [`pay::PayResult`] | 统一支付结果 |
//! | [`pay::PayPlatform`] | 支付平台枚举 |
//! | [`pay::PayError`] | 支付错误类型 |
//! | [`pay::PayHttpTransport`] | HTTP 传输抽象 trait |
//! | [`pay::MemoryPayHttpTransport`] | 内存 HTTP 传输实现 |
//!
//! ## 用法
//!
//! ```rust,ignore
//! use sz_rust_pay_facade::pay::{PayOrder, MemoryPayProvider, PayProvider, PayPlatform};
//!
//! let provider = MemoryPayProvider::new();
//! let order = PayOrder::new()
//!     .out_trade_no("202401010001")
//!     .total_amount(8800)
//!     .subject("鲜视达商品");
//!
//! let result = provider.pay(order).unwrap();
//! ```
//!
//! ## 与 sz-rust-core 的关系
//!
//! `sz-rust-core` 通过 `pub use sz_rust_pay_facade::pay;` 重导出本 crate，
//! 因此 `sz_rust_core::pay` 等价于 `sz_rust_pay_facade::pay`。
//! 下游业务包推荐直接依赖 `sz-rust-pay-facade` 以减少编译耦合。

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod pay;

pub use pay::{
    MemoryPayHttpTransport, MemoryPayProvider, PayConfig, PayError, PayHttpTransport, PayOrder,
    PayPlatform, PayProvider, PayResult, RefundOrder,
};
