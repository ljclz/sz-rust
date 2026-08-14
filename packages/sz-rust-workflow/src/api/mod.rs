//! 设计器 HTTP API

pub mod designer;
pub mod version_manager;

pub use designer::DesignerApi;
pub use version_manager::{DefinitionSummary, VersionManager};
