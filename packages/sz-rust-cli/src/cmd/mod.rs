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
//! | `seed` | `think\db\Seed` | 数据填充命令 |
//! | `optimize` | `think\console\command\optimize\*` | 配置/路由缓存优化命令 |

pub mod cache;
pub mod make;
pub mod migrate;
pub mod optimize;
pub mod plugin;
pub mod route;
pub mod scheduler;
pub mod seed;

/// 测试辅助模块 — 提供跨模块共享的全局互斥锁
///
/// `optimize` 与 `make` 测试均通过 `std::env::set_current_dir` 切换工作目录，
/// 该状态是进程级全局的。若两模块各自持有独立 mutex 并行运行，会互相污染工作目录。
/// 本模块提供单一全局锁，所有依赖 `set_current_dir` 的测试都通过它串行化。
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard};

    /// 全局测试互斥锁 — 协调所有使用 `set_current_dir` 的测试，避免跨模块并行竞态
    pub static GLOBAL_TEST_MUTEX: Mutex<()> = Mutex::new(());

    /// 获取全局互斥锁（处理中毒情况：前一个测试 panic 后恢复）
    pub fn acquire_global_lock() -> MutexGuard<'static, ()> {
        GLOBAL_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner())
    }
}
