// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! TestCase 基类：setUp/tearDown 自动事务包裹。

use async_trait::async_trait;

/// 测试用例 trait。
///
/// 实现 `setUp` 和 `tearDown` 方法，测试前初始化环境，测试后清理。
///
/// ```
/// use sz_rust_testkit::TestCase;
/// use async_trait::async_trait;
///
/// struct MyTest;
///
/// #[async_trait]
/// impl TestCase for MyTest {
///     async fn set_up(&self) {}
///     async fn tear_down(&self) {}
/// }
/// ```
#[async_trait]
pub trait TestCase: Send + Sync {
    /// 测试前初始化。
    async fn set_up(&self) {}

    /// 测试后清理。
    async fn tear_down(&self) {}

    /// 执行测试（包裹 setUp/tearDown）。
    async fn run<F, Fut, R>(&self, f: F) -> R
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
        R: Send,
    {
        self.set_up().await;
        let result = f().await;
        self.tear_down().await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct CounterTest {
        counter: Arc<AtomicU32>,
    }

    #[async_trait]
    impl TestCase for CounterTest {
        async fn set_up(&self) {
            self.counter.store(0, Ordering::SeqCst);
        }

        async fn tear_down(&self) {
            self.counter.fetch_add(100, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn run_executes_set_up_and_tear_down() {
        let counter = Arc::new(AtomicU32::new(42));
        let test = CounterTest {
            counter: counter.clone(),
        };

        let result = test
            .run(|| async {
                let val = counter.load(Ordering::SeqCst);
                val + 1
            })
            .await;

        assert_eq!(result, 1);
        assert_eq!(counter.load(Ordering::SeqCst), 100);
    }

    #[tokio::test]
    async fn default_set_up_tear_down_are_noop() {
        struct NoopTest;
        #[async_trait]
        impl TestCase for NoopTest {}

        let test = NoopTest;
        let result = test.run(|| async { 42 }).await;
        assert_eq!(result, 42);
    }
}
