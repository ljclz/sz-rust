// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据填充（Seed）— 对齐 PHP `think\db\Seed`
//!
//! ## PHP 对齐
//!
//! PHP `db:seed` 通过 `Seeder` 类的 `run()` 方法执行数据填充：
//! ```php
//! class DatabaseSeeder extends Seeder {
//!     public function run() {
//!         $this->call('UserSeeder');
//!         $this->call('RoleSeeder');
//!     }
//! }
//! ```
//!
//! Rust 端通过 [`Seeder`] trait 抽象填充器，业务实现 `run()` 方法即可。
//! 由于 Rust 为静态编译语言，无法像 PHP 那样按类名动态加载，
//! CLI 层（`sz-rust-cli`）通过加载 `seeds/` 目录下的 SQL 文件实现离线填充。
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_core::orm::Connection;
//! use sz_rust_core::seed::Seeder;
//!
//! struct UserSeeder;
//!
//! #[async_trait::async_trait]
//! impl Seeder for UserSeeder {
//!     fn name(&self) -> &str { "UserSeeder" }
//!
//!     async fn run(&self, conn: &mut Box<dyn Connection>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!         conn.execute("INSERT INTO users (name, email) VALUES ('admin', 'admin@example.com')").await?;
//!         Ok(())
//!     }
//! }
//! ```

use std::future::Future;
use std::pin::Pin;

use crate::orm::Connection;

/// 填充器执行结果：Boxed Future，成功返回 `()`，失败返回可发送的错误
pub type SeederResult<'a> =
    Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + 'a>>;

/// 填充器 trait — 对齐 PHP `think\db\Seeder`
///
/// 业务实现该 trait，在 `run()` 中执行数据插入。
/// CLI 层通过加载 `seeds/` 目录下的 SQL 文件实现离线填充，
/// 程序化场景下可直接实现该 trait 并通过 [`SeedRunner`] 执行。
pub trait Seeder: Send + Sync {
    /// 填充器名称（用于日志输出）
    fn name(&self) -> &str;

    /// 执行数据填充
    ///
    /// # 参数
    ///
    /// - `conn`：数据库连接（已建立）
    ///
    /// # 错误
    ///
    /// 返回 `Err` 表示填充失败，调用方决定是否回滚。
    fn run<'a>(&'a self, conn: &'a mut Box<dyn Connection>) -> SeederResult<'a>;
}

/// 填充器运行器 — 管理多个填充器的顺序执行
///
/// 对齐 PHP `DatabaseSeeder::run()` 中通过 `$this->call()` 串联多个子填充器的模式。
///
/// ## 使用示例
///
/// ```ignore
/// let mut runner = SeedRunner::new();
/// runner.register(Box::new(UserSeeder));
/// runner.register(Box::new(RoleSeeder));
/// runner.execute(&mut conn).await?;
/// ```
pub struct SeedRunner {
    seeders: Vec<Box<dyn Seeder>>,
}

impl SeedRunner {
    /// 创建空的填充器运行器
    pub fn new() -> Self {
        Self {
            seeders: Vec::new(),
        }
    }

    /// 注册填充器（按注册顺序执行）
    pub fn register(&mut self, seeder: Box<dyn Seeder>) -> &mut Self {
        self.seeders.push(seeder);
        self
    }

    /// 按注册顺序执行所有填充器
    ///
    /// # 错误
    ///
    /// 任一填充器失败即返回错误，后续填充器不再执行。
    pub async fn execute(
        &self,
        conn: &mut Box<dyn Connection>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for seeder in &self.seeders {
            println!("[Seed] Running {}...", seeder.name());
            seeder.run(conn).await?;
            println!("[Seed] {} completed.", seeder.name());
        }
        Ok(())
    }

    /// 返回已注册的填充器数量
    pub fn len(&self) -> usize {
        self.seeders.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.seeders.is_empty()
    }
}

impl Default for SeedRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orm::DbError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use sz_orm_core::QueryRows;

    /// 测试用的计数填充器
    struct CounterSeeder {
        name: String,
        counter: Arc<AtomicUsize>,
    }

    impl Seeder for CounterSeeder {
        fn name(&self) -> &str {
            &self.name
        }

        fn run<'a>(&'a self, _conn: &'a mut Box<dyn Connection>) -> SeederResult<'a> {
            Box::pin(async move {
                self.counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    #[test]
    fn test_seed_runner_register_and_len() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut runner = SeedRunner::new();
        assert_eq!(runner.len(), 0);
        assert!(runner.is_empty());

        runner.register(Box::new(CounterSeeder {
            name: "A".to_string(),
            counter: counter.clone(),
        }));
        runner.register(Box::new(CounterSeeder {
            name: "B".to_string(),
            counter: counter.clone(),
        }));
        assert_eq!(runner.len(), 2);
        assert!(!runner.is_empty());
    }

    #[test]
    fn test_seed_runner_default() {
        let runner = SeedRunner::default();
        assert!(runner.is_empty());
    }

    /// 测试用 Mock 连接：所有操作返回 Ok，用于验证 SeedRunner::execute 调用 seeder
    struct MockConnection {
        execute_count: AtomicUsize,
    }

    impl MockConnection {
        fn new() -> Self {
            Self {
                execute_count: AtomicUsize::new(0),
            }
        }
    }

    impl Connection for MockConnection {
        fn execute<'a>(
            &'a mut self,
            _sql: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
            self.execute_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(0) })
        }
        fn query<'a>(
            &'a mut self,
            _sql: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
            Box::pin(async move { Ok(Vec::new()) })
        }
        fn begin_transaction<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn commit<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn rollback<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
        fn is_connected(&self) -> bool {
            true
        }
        fn ping<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            Box::pin(async move { true })
        }
        fn close<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async move { Ok(()) })
        }
    }

    // 捕获 execute -> Ok(()) 变异体：若 execute 不调用 seeder，counter 仍为 0
    #[tokio::test]
    async fn test_execute_calls_all_seeders() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut runner = SeedRunner::new();
        runner.register(Box::new(CounterSeeder {
            name: "A".to_string(),
            counter: counter.clone(),
        }));
        runner.register(Box::new(CounterSeeder {
            name: "B".to_string(),
            counter: counter.clone(),
        }));
        let mut conn: Box<dyn Connection> = Box::new(MockConnection::new());
        runner.execute(&mut conn).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
