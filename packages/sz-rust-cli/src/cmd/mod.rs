//! 命令模块 — 各命令的实现
//!
//! ## 模块结构
//!
//! | 模块 | 对齐 PHP | 说明 |
//! |------|---------|------|
//! | `make` | `think\console\command\make\*` | 代码生成命令 |
//! | `migrate` | `think\console\command\migrate\*` | 数据库迁移命令 |
//! | `route` | `think\console\command\RouteList` | 路由列表命令 |
//! | `cache` | `think\console\command\Cache` | 缓存清理命令 |
//! | `scheduler` | sz-orm-scheduler 接入 | 调度器命令 |

pub mod cache;
pub mod make;
pub mod migrate;
pub mod route;
pub mod scheduler;
