// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 持久化 Repository trait + InMemory 实现

pub mod definition_repo;
pub mod history_repo;
pub mod in_memory;
pub mod instance_repo;
pub mod task_repo;

pub use definition_repo::{DefinitionId, DefinitionRepository};
pub use history_repo::HistoryRepository;
pub use in_memory::{
    InMemoryDefinitionRepository, InMemoryHistoryRepository, InMemoryInstanceRepository,
    InMemoryTaskRepository,
};
pub use instance_repo::InstanceRepository;
pub use task_repo::TaskRepository;
