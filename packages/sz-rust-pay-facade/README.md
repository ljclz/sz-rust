> **中文** | [English](README.en.md)

# sz-rust-pay-facade

支付聚合抽象层 Facade，提取自 `sz-rust-core::pay`。

对齐 PHP [`yansongda/pay`](https://github.com/yansongda/pay)，提供统一的支付抽象，支持多平台（支付宝、微信支付等）扩展。

## 安装

```toml
[dependencies]
sz-rust-pay-facade = "0.3.0"
```

## 快速开始

```rust
use sz_rust_pay_facade::pay::{PayOrder, MemoryPayProvider, PayProvider, PayPlatform};

// 使用内存实现（测试/开发）
let provider = MemoryPayProvider::new();

let order = PayOrder::new()
    .out_trade_no("202401010001")
    .total_amount(8800)   // 单位：分
    .subject("鲜视达商品");

let result = provider.pay(order)?;
println!("支付成功: {}", result.trade_no);
```

## 核心 API

| 类型 | 说明 |
|------|------|
| [`PayProvider`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/trait.PayProvider.html) | 支付提供商 trait |
| [`MemoryPayProvider`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/struct.MemoryPayProvider.html) | 内存实现（测试用） |
| [`PayOrder`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/struct.PayOrder.html) | 支付订单 Builder |
| [`RefundOrder`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/struct.RefundOrder.html) | 退款订单 Builder |
| [`PayConfig`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/struct.PayConfig.html) | 支付配置 Builder |
| [`PayPlatform`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/enum.PayPlatform.html) | 支付平台枚举 |
| [`PayError`](https://docs.rs/sz-rust-pay-facade/latest/sz_rust_pay_facade/pay/enum.PayError.html) | 错误类型 |

## 与 sz-rust-core 的关系

`sz-rust-core` 重导出了本 crate：

```rust
// 以下两条路径等价：
use sz_rust_core::pay::{PayOrder, PayProvider};
use sz_rust_pay_facade::pay::{PayOrder, PayProvider};
```

下游业务包推荐直接依赖 `sz-rust-pay-facade`，以减少编译耦合。

## License

MIT
