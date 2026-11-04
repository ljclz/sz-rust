# sz-rust-pay-facade

> **中文** | [English](README.en.md)

Payment aggregation abstraction Facade, extracted from `sz-rust-core::pay`.

Aligned with PHP [`yansongda/pay`](https://github.com/yansongda/pay), providing unified payment abstraction supporting multi-platform (Alipay, WeChat Pay, etc.) extensions.

## Installation

```toml
[dependencies]
sz-rust-pay-facade = "0.3.0"
```

## Quick Start

```rust
use sz_rust_pay_facade::pay::{PayOrder, MemoryPayProvider, PayProvider, PayPlatform};

// Use in-memory implementation (testing/development)
let provider = MemoryPayProvider::new();

let order = PayOrder::new()
    .out_trade_no("202401010001")
    .total_amount(8800)   // unit: cents
    .subject("Product");

let result = provider.pay(order)?;
println!("Payment success: {}", result.trade_no);
```

## Core API

| Type | Description |
|------|-------------|
| [`PayProvider`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/trait.PayProvider.html) | Payment provider trait |
| [`MemoryPayProvider`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/struct.MemoryPayProvider.html) | In-memory implementation (for testing) |
| [`PayOrder`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/struct.PayOrder.html) | Payment order builder |
| [`RefundOrder`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/struct.RefundOrder.html) | Refund order builder |
| [`PayConfig`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/struct.PayConfig.html) | Payment config builder |
| [`PayPlatform`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/enum.PayPlatform.html) | Payment platform enum |
| [`PayError`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/enum.PayError.html) | Error type |

## Relationship with sz-rust-core

`sz-rust-core` re-exports this crate:

```rust
// These two paths are equivalent:
use sz_rust_core::pay::{PayOrder, PayProvider};
use sz_rust_pay_facade::pay::{PayOrder, PayProvider};
```

Downstream business packages should depend on `sz-rust-pay-facade` directly to reduce compile coupling.

## License

MIT