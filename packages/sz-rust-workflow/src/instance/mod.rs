// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 实例/任务/历史领域模型

pub mod models;

pub use models::{
    ApprovalRecord, FlowInstance, HistoryEntry, HistoryEntryType, InstanceStatus, PageRequest,
    PageResult, Task, TaskAction, TaskStatus,
};
