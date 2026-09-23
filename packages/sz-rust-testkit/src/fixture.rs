// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Fixture：测试前填充数据、测试后自动清理。

use async_trait::async_trait;

/// Fixture trait：测试前填充、测试后清理。
#[async_trait]
pub trait Fixture: Send + Sync {
    /// 测试前填充数据。
    async fn set_up(&self);

    /// 测试后清理数据。
    async fn tear_down(&self);
}

/// Fixture 管理器：批量执行多个 Fixture。
pub struct FixtureManager {
    fixtures: Vec<Box<dyn Fixture>>,
}

impl Default for FixtureManager {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureManager {
    /// 创建空管理器。
    pub fn new() -> Self {
        Self {
            fixtures: Vec::new(),
        }
    }

    /// 注册 Fixture。
    pub fn register(&mut self, fixture: impl Fixture + 'static) {
        self.fixtures.push(Box::new(fixture));
    }

    /// 执行所有 Fixture 的 set_up。
    pub async fn set_up_all(&self) {
        for f in &self.fixtures {
            f.set_up().await;
        }
    }

    /// 执行所有 Fixture 的 tear_down（逆序）。
    pub async fn tear_down_all(&self) {
        for f in self.fixtures.iter().rev() {
            f.tear_down().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct CounterFixture {
        counter: Arc<AtomicU32>,
        value: u32,
    }

    #[async_trait]
    impl Fixture for CounterFixture {
        async fn set_up(&self) {
            self.counter.fetch_add(self.value, Ordering::SeqCst);
        }

        async fn tear_down(&self) {
            self.counter.fetch_sub(self.value, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn fixture_manager_set_up_and_tear_down() {
        let counter = Arc::new(AtomicU32::new(0));
        let mut manager = FixtureManager::new();
        manager.register(CounterFixture {
            counter: counter.clone(),
            value: 10,
        });
        manager.register(CounterFixture {
            counter: counter.clone(),
            value: 20,
        });

        manager.set_up_all().await;
        assert_eq!(counter.load(Ordering::SeqCst), 30);

        manager.tear_down_all().await;
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn fixture_manager_empty() {
        let manager = FixtureManager::new();
        manager.set_up_all().await;
        manager.tear_down_all().await;
    }
}
