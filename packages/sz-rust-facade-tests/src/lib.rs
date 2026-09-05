// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! sz-rust-facade-tests — 跨 Facade 集成测试（P9-FACADE 系列）
//!
//! 本 crate 不包含业务代码，仅承载集成测试，验证 7 个 facade crate 之间的协作：
//!
//! | 测试文件 | 覆盖场景 |
//! |---------|---------|
//! | `tests/cache_state_integration.rs` | cache + state（session/env/event）联动 |
//! | `tests/orm_pay_http_integration.rs` | orm 参数化查询 + pay + http 响应组装 |
//! | `tests/auth_infra_integration.rs` | auth（wechat 签名/JWT）+ infra（路径安全/MIME） |
//! | `tests/end_to_end_flow.rs` | 端到端业务流（下单 → 缓存 → 事件 → 支付 → 响应） |
//!
//! 设计原则：
//! - 全部使用纯内存 / 临时文件实现，不依赖外部服务（DB / Redis / 网络）
//! - 每个测试至少跨 2 个 facade，验证依赖方向与数据流正确
//! - 覆盖规则 10（测试覆盖率）中"facade 间集成测试"盲区
