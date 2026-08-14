//! 公共测试工具模块
//!
//! 提供 mock HTTP server、mock 时钟和 mock Provider，
//! 供所有集成测试文件复用。
//!
//! 各 stub 按测试用例按需引用：单独 crate 编译时部分 stub 可能未被
//! 任何用例引用，属共享测试库正常形态，统一 allow(dead_code)。
#![allow(dead_code)]

pub mod mock_clock;
pub mod mock_server;
pub mod providers;
