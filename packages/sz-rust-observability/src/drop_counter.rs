// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Drop 计数器（P4-4）
//!
//! 验证资源是否被正确 Drop，未 Drop 的资源判定为泄漏。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Drop 计数器
///
/// 记录资源创建和 Drop 次数，用于验证资源是否被正确释放。
pub struct DropCounter {
    /// 创建计数
    created: AtomicUsize,
    /// Drop 计数
    dropped: AtomicUsize,
    /// 资源名称
    name: String,
}

impl DropCounter {
    /// 创建 Drop 计数器
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            created: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
            name: name.into(),
        }
    }

    /// 记录资源创建
    pub fn inc_created(&self) {
        self.created.fetch_add(1, Ordering::SeqCst);
    }

    /// 记录资源 Drop
    pub fn inc_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }

    /// 获取创建数
    pub fn created(&self) -> usize {
        self.created.load(Ordering::SeqCst)
    }

    /// 获取 Drop 数
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }

    /// 未 Drop 的资源数
    pub fn leaked(&self) -> usize {
        self.created().saturating_sub(self.dropped())
    }

    /// 是否有泄漏
    pub fn has_leak(&self) -> bool {
        self.leaked() > 0
    }

    /// 资源名称
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// 受 Drop 计数器追踪的资源
///
/// 创建时 `inc_created`，Drop 时 `inc_dropped`。
pub struct TrackedResource<T> {
    value: Option<T>,
    counter: Arc<DropCounter>,
}

impl<T> TrackedResource<T> {
    /// 创建受追踪的资源
    pub fn new(value: T, counter: Arc<DropCounter>) -> Self {
        counter.inc_created();
        Self {
            value: Some(value),
            counter,
        }
    }

    /// 获取内部值引用
    pub fn get(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// 取出内部值（不再追踪 Drop）
    ///
    /// `take` 按值消费 `self`，`value` 仅在 `new` 中置为 `Some` 且 `get` 不转移所有权，
    /// 因此此处 `None` 分支在类型层面不可达；`expect` 仅作不变式断言，panic 实际不会触发。
    pub fn take(mut self) -> T {
        self.value.take().expect("TrackedResource 值已被取出过")
    }
}

impl<T> Drop for TrackedResource<T> {
    fn drop(&mut self) {
        if self.value.is_some() {
            self.counter.inc_dropped();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drop_counter_basic() {
        let counter = DropCounter::new("test");
        counter.inc_created();
        counter.inc_created();
        counter.inc_dropped();
        assert_eq!(counter.created(), 2);
        assert_eq!(counter.dropped(), 1);
        assert_eq!(counter.leaked(), 1);
        assert!(counter.has_leak());
    }

    #[test]
    fn test_drop_counter_no_leak() {
        let counter = DropCounter::new("test");
        counter.inc_created();
        counter.inc_dropped();
        assert!(!counter.has_leak());
        assert_eq!(counter.leaked(), 0);
    }

    #[test]
    fn test_tracked_resource_drop() {
        let counter = Arc::new(DropCounter::new("resource"));
        {
            let _r = TrackedResource::new(42, counter.clone());
            assert_eq!(counter.created(), 1);
            assert_eq!(counter.dropped(), 0);
        }
        assert_eq!(counter.dropped(), 1);
        assert!(!counter.has_leak());
    }

    #[test]
    fn test_tracked_resource_leak() {
        let counter = Arc::new(DropCounter::new("resource"));
        let _leaked = TrackedResource::new(42, counter.clone());
        assert_eq!(counter.created(), 1);
        assert_eq!(counter.dropped(), 0);
        assert!(counter.has_leak());
        assert_eq!(counter.leaked(), 1);
    }

    #[test]
    fn test_tracked_resource_take() {
        let counter = Arc::new(DropCounter::new("resource"));
        let r = TrackedResource::new(42, counter.clone());
        let val = r.take();
        assert_eq!(val, 42);
        assert_eq!(counter.created(), 1);
        assert_eq!(counter.dropped(), 0, "take() should not count as drop");
    }

    #[test]
    fn test_multiple_resources() {
        let counter = Arc::new(DropCounter::new("multi"));
        let r1 = TrackedResource::new(1, counter.clone());
        let r2 = TrackedResource::new(2, counter.clone());
        let r3 = TrackedResource::new(3, counter.clone());
        assert_eq!(counter.created(), 3);

        drop(r1);
        assert_eq!(counter.dropped(), 1);
        assert_eq!(counter.leaked(), 2);

        drop(r2);
        drop(r3);
        assert_eq!(counter.dropped(), 3);
        assert!(!counter.has_leak());
    }
}
