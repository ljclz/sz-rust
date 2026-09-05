// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 迁移执行器 — 逻辑已内联到 [`super::state_machine::StateMachineEngine`]。
//!
//! 原子迁移顺序：持久化 → 内存 → 事件发布，由 `StateMachineEngine::fire` 实现。
