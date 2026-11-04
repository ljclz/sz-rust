# ADR-035: 插件市场基础设施

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `packages/sz-rust-marketplace/src/payment/`, `packages/sz-rust-marketplace/src/review.rs`, `packages/sz-rust-marketplace/src/service.rs`

## 背景

P2-2 缺口：插件市场缺乏支付集成、CLI/Web 数据同步、完善的审核流程和插件安装原子性保证。

## 决策

1. **支付集成**：PaymentGateway trait + 支付宝/微信支付实现，集成 sz-rust-pay-facade
2. **订阅服务**：SubscriptionService 支持 purchase/subscribe/renew/cancel，交易记录持久化
3. **审核流程**：5 项自动检查（安全扫描/许可证/manifest 格式/编译/版本兼容性）
4. **CLI/Web 同步**：sync_status 方法验证数据一致性
5. **原子安装**：安装任一步骤失败完全回滚，交易记录可追溯

## 替代方案

- **第三方支付 SDK**：依赖外部服务，测试困难
- **手动审核**：效率低，无法规模化

## Bug 定位提示

- `payment/gateway.rs` — PaymentGateway trait 抽象
- `payment/alipay.rs` — 支付宝网关，tokio::time::timeout 包裹
- `payment/subscription.rs` — SubscriptionService 购买/订阅/续订/取消
- `review.rs` — ReviewService 5 项自动审核检查

## 影响

- 73 tests passed（62 lib + 11 integration）
- 支付敏感字段 `#[serde(skip_serializing)]`（铁律 7）
- 搜索 p99 ≤ 500ms 验证通过