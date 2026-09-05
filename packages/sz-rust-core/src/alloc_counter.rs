// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Alloc 计数 GlobalAlloc wrapper — P3 性能度量工具
//!
//! 提供 `AllocCounter<A>` 包装全局分配器，统计 alloc/dealloc 次数，
//! 用于 P3 热点路径堆分配次数度量。
//!
//! ## 设计
//!
//! - 包装 `std::alloc::System`（或任意 `GlobalAlloc`），使用 `AtomicUsize` 统计次数
//! - `measure(f)` 执行闭包并返回结果 + 分配次数差值
//! - 通过 `#[cfg(feature = "alloc-count")]` feature gate 控制，默认不启用
//! - `register_alloc_counter!` 宏注册全局分配器
//!
//! ## 用法
//!
//! ```rust,ignore
//! use sz_rust_core::alloc_counter::{AllocCounter, register_alloc_counter};
//!
//! // 注册全局分配器（仅在 bin crate 的 main 之前调用一次）
//! register_alloc_counter!();
//!
//! // 测量闭包的分配次数
//! let (result, count) = AllocCounter::measure(|| {
//!     let s = "hello".to_string();
//!     s + " world"
//! });
//! assert_eq!(result, "hello world");
//! assert!(count > 0);
//! ```
//!
//! ## Safety
//!
//! `GlobalAlloc` trait 要求 `unsafe impl`，此模块在 `alloc-count` feature
//! 启用时允许 unsafe_code（通过 `#[allow(unsafe_code)]`），unsafe 仅限
//! GlobalAlloc 方法实现内部，不暴露到公开 API。

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// 全局 alloc 计数器（alloc 次数）
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 全局 dealloc 计数器（dealloc 次数）
static DEALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 全局 alloc 字节数
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

/// 全局 dealloc 字节数
static DEALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Alloc 计数 GlobalAlloc wrapper
///
/// 包装内层分配器 `A`，在每次 alloc/dealloc 时递增全局计数器。
/// 通过 `register_alloc_counter!` 宏注册为全局分配器。
///
/// ## 类型参数
///
/// - `A`: 内层分配器（通常为 `std::alloc::System`）
pub struct AllocCounter<A: GlobalAlloc + Send + Sync + Default = System> {
    _inner: A,
}

impl<A: GlobalAlloc + Send + Sync + Default> AllocCounter<A> {
    /// 创建 AllocCounter 实例
    pub fn new() -> Self {
        Self {
            _inner: A::default(),
        }
    }
}

// 统计方法不依赖内层分配器类型，仅对默认 System 实现以简化 API。
// 调用方使用 AllocCounter::count() 即可，无需指定类型参数。
impl AllocCounter<System> {
    /// 获取当前 alloc 次数
    pub fn count() -> usize {
        ALLOC_COUNT.load(Ordering::Relaxed)
    }

    /// 获取当前 dealloc 次数
    pub fn dealloc_count() -> usize {
        DEALLOC_COUNT.load(Ordering::Relaxed)
    }

    /// 获取当前 alloc 字节数
    pub fn alloc_bytes() -> usize {
        ALLOC_BYTES.load(Ordering::Relaxed)
    }

    /// 获取当前 dealloc 字节数
    pub fn dealloc_bytes() -> usize {
        DEALLOC_BYTES.load(Ordering::Relaxed)
    }

    /// 获取当前净分配次数（alloc - dealloc）
    pub fn net_count() -> isize {
        let alloc = ALLOC_COUNT.load(Ordering::Relaxed) as isize;
        let dealloc = DEALLOC_COUNT.load(Ordering::Relaxed) as isize;
        alloc - dealloc
    }

    /// 获取当前净分配字节数（alloc_bytes - dealloc_bytes）
    pub fn net_bytes() -> isize {
        let alloc = ALLOC_BYTES.load(Ordering::Relaxed) as isize;
        let dealloc = DEALLOC_BYTES.load(Ordering::Relaxed) as isize;
        alloc - dealloc
    }

    /// 重置所有计数器为零
    pub fn reset() {
        ALLOC_COUNT.store(0, Ordering::Relaxed);
        DEALLOC_COUNT.store(0, Ordering::Relaxed);
        ALLOC_BYTES.store(0, Ordering::Relaxed);
        DEALLOC_BYTES.store(0, Ordering::Relaxed);
    }

    /// 测量闭包执行期间的分配次数
    ///
    /// 返回闭包结果和分配次数差值（闭包执行期间的新增 alloc 次数）。
    ///
    /// ## 用法
    ///
    /// ```rust,ignore
    /// let (result, count) = AllocCounter::measure(|| {
    ///     "hello".to_string() + " world"
    /// });
    /// assert_eq!(result, "hello world");
    /// ```
    pub fn measure<F, R>(f: F) -> (R, usize)
    where
        F: FnOnce() -> R,
    {
        let before = ALLOC_COUNT.load(Ordering::Relaxed);
        let result = f();
        let after = ALLOC_COUNT.load(Ordering::Relaxed);
        (result, after.saturating_sub(before))
    }

    /// 测量闭包执行期间的分配详情
    ///
    /// 返回闭包结果和 `AllocStats`（alloc/dealloc 次数 + 字节数差值）。
    pub fn measure_detailed<F, R>(f: F) -> (R, AllocStats)
    where
        F: FnOnce() -> R,
    {
        let before_alloc = ALLOC_COUNT.load(Ordering::Relaxed);
        let before_dealloc = DEALLOC_COUNT.load(Ordering::Relaxed);
        let before_alloc_bytes = ALLOC_BYTES.load(Ordering::Relaxed);
        let before_dealloc_bytes = DEALLOC_BYTES.load(Ordering::Relaxed);

        let result = f();

        let stats = AllocStats {
            alloc_count: ALLOC_COUNT
                .load(Ordering::Relaxed)
                .saturating_sub(before_alloc),
            dealloc_count: DEALLOC_COUNT
                .load(Ordering::Relaxed)
                .saturating_sub(before_dealloc),
            alloc_bytes: ALLOC_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(before_alloc_bytes),
            dealloc_bytes: DEALLOC_BYTES
                .load(Ordering::Relaxed)
                .saturating_sub(before_dealloc_bytes),
        };
        (result, stats)
    }
}

/// 分配统计详情
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocStats {
    /// alloc 次数
    pub alloc_count: usize,
    /// dealloc 次数
    pub dealloc_count: usize,
    /// alloc 字节数
    pub alloc_bytes: usize,
    /// dealloc 字节数
    pub dealloc_bytes: usize,
}

impl AllocStats {
    /// 净分配次数（alloc - dealloc）
    pub fn net_count(&self) -> isize {
        self.alloc_count as isize - self.dealloc_count as isize
    }

    /// 净分配字节数（alloc_bytes - dealloc_bytes）
    pub fn net_bytes(&self) -> isize {
        self.alloc_bytes as isize - self.dealloc_bytes as isize
    }

    /// 是否无分配
    pub fn is_zero_alloc(&self) -> bool {
        self.alloc_count == 0
    }
}

impl std::fmt::Display for AllocStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "alloc={}/{}, dealloc={}/{}, net={}/{}",
            self.alloc_count,
            self.alloc_bytes,
            self.dealloc_count,
            self.dealloc_bytes,
            self.net_count(),
            self.net_bytes()
        )
    }
}

// ============================================================================
// GlobalAlloc 实现
// ============================================================================

unsafe impl<A: GlobalAlloc + Send + Sync + Default> GlobalAlloc for AllocCounter<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        A::default().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        DEALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        A::default().dealloc(ptr, layout);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        A::default().alloc_zeroed(layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, old_layout: Layout, new_size: usize) -> *mut u8 {
        DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        DEALLOC_BYTES.fetch_add(old_layout.size(), Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size, Ordering::Relaxed);
        A::default().realloc(ptr, old_layout, new_size)
    }
}

// ============================================================================
// 全局分配器注册宏
// ============================================================================

/// 注册全局 alloc 计数分配器
///
/// 在 bin crate 的顶层（main 函数之前）调用此宏，将 `AllocCounter<System>`
/// 注册为全局分配器。仅在 `alloc-count` feature 启用时生效。
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::alloc_counter::register_alloc_counter;
///
/// register_alloc_counter!();
///
/// fn main() {
///     let (s, count) = sz_rust_core::alloc_counter::AllocCounter::measure(|| {
///         "hello".to_string()
///     });
///     println!("result={s}, allocs={count}");
/// }
/// ```
#[macro_export]
macro_rules! register_alloc_counter {
    () => {
        #[global_allocator]
        static GLOBAL_ALLOC: $crate::alloc_counter::AllocCounter<std::alloc::System> =
            $crate::alloc_counter::AllocCounter {
                _inner: std::alloc::System,
            };
    };
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // 全局锁：串行化所有触碰全局 AtomicUsize 计数器的测试，避免并行 reset() 互相干扰。
    static GLOBAL_COUNTER_LOCK: Mutex<()> = Mutex::new(());

    // 测试辅助：手动递增计数器，模拟全局分配器行为
    // （lib 测试中无法注册 #[global_allocator]，需在 bin crate 中注册）
    fn inc_alloc(n: usize, bytes: usize) {
        ALLOC_COUNT.fetch_add(n, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(bytes, Ordering::Relaxed);
    }

    fn inc_dealloc(n: usize, bytes: usize) {
        DEALLOC_COUNT.fetch_add(n, Ordering::Relaxed);
        DEALLOC_BYTES.fetch_add(bytes, Ordering::Relaxed);
    }

    // 捕获 count/dealloc_count/alloc_bytes/dealloc_bytes -> 0/1 变异体
    // 4 组值（含 0、1、5、100）使所有常量替换变异失败
    #[test]
    fn test_alloc_counter_getters_strict() {
        let _lock = GLOBAL_COUNTER_LOCK.lock().unwrap();
        for &(an, ab, dn, db) in &[
            (0usize, 0usize, 0usize, 0usize),
            (1, 8, 0, 0),
            (5, 100, 3, 60),
            (100, 8000, 40, 3200),
        ] {
            AllocCounter::reset();
            inc_alloc(an, ab);
            inc_dealloc(dn, db);
            assert_eq!(AllocCounter::count(), an, "count an={an}");
            assert_eq!(AllocCounter::dealloc_count(), dn, "dealloc_count dn={dn}");
            assert_eq!(AllocCounter::alloc_bytes(), ab, "alloc_bytes ab={ab}");
            assert_eq!(AllocCounter::dealloc_bytes(), db, "dealloc_bytes db={db}");
        }
    }

    // 捕获 net_count/net_bytes -> 0/1/-1 及 `-`→`+`/`/` 变异体
    // 3 组数据使算术变异全部失败（`-`→`+` 与 `-`→`/` 在 3 组数据上无法同时成立）
    #[test]
    fn test_alloc_counter_net_arithmetic_strict() {
        let _lock = GLOBAL_COUNTER_LOCK.lock().unwrap();
        for &(an, ab, dn, db, nc, nb) in &[
            (10usize, 1000usize, 4usize, 400usize, 6isize, 600isize),
            (20, 2000, 5, 500, 15, 1500),
            (7, 700, 2, 200, 5, 500),
        ] {
            AllocCounter::reset();
            inc_alloc(an, ab);
            inc_dealloc(dn, db);
            assert_eq!(AllocCounter::net_count(), nc, "net_count {an}-{dn}");
            assert_eq!(AllocCounter::net_bytes(), nb, "net_bytes {ab}-{db}");
        }
    }

    // 捕获 reset() -> () 变异体：reset 后所有计数器必须归零
    #[test]
    fn test_alloc_counter_reset_strict() {
        let _lock = GLOBAL_COUNTER_LOCK.lock().unwrap();
        inc_alloc(50, 5000);
        inc_dealloc(30, 3000);
        AllocCounter::reset();
        assert_eq!(AllocCounter::count(), 0);
        assert_eq!(AllocCounter::dealloc_count(), 0);
        assert_eq!(AllocCounter::alloc_bytes(), 0);
        assert_eq!(AllocCounter::dealloc_bytes(), 0);
        assert_eq!(AllocCounter::net_count(), 0);
        assert_eq!(AllocCounter::net_bytes(), 0);
    }

    // 捕获 measure -> (Default, 0)/(Default, 1) 变异体
    // 3 组数据（delta=0/7/13）使常量替换变异失败
    #[test]
    fn test_alloc_counter_measure_strict() {
        let _lock = GLOBAL_COUNTER_LOCK.lock().unwrap();

        AllocCounter::reset();
        let (r, c) = AllocCounter::measure(|| 1 + 2);
        assert_eq!((r, c), (3, 0));

        AllocCounter::reset();
        let (r, c) = AllocCounter::measure(|| {
            inc_alloc(7, 700);
            "hello"
        });
        assert_eq!((r, c), ("hello", 7));

        AllocCounter::reset();
        let (r, c) = AllocCounter::measure(|| {
            inc_alloc(13, 1300);
            42i64
        });
        assert_eq!((r, c), (42, 13));
    }

    // 捕获 measure_detailed -> (Default, Default) 变异体
    // 2 组数据使所有字段被验证
    #[test]
    fn test_alloc_counter_measure_detailed_strict() {
        let _lock = GLOBAL_COUNTER_LOCK.lock().unwrap();

        AllocCounter::reset();
        let (r, s) = AllocCounter::measure_detailed(|| {
            inc_alloc(10, 1000);
            inc_dealloc(2, 200);
            99i32
        });
        assert_eq!(r, 99);
        assert_eq!(s.alloc_count, 10);
        assert_eq!(s.dealloc_count, 2);
        assert_eq!(s.alloc_bytes, 1000);
        assert_eq!(s.dealloc_bytes, 200);

        AllocCounter::reset();
        let (r, s) = AllocCounter::measure_detailed(|| {
            inc_alloc(3, 30);
            inc_dealloc(1, 10);
            "ok"
        });
        assert_eq!(r, "ok");
        assert_eq!(s.alloc_count, 3);
        assert_eq!(s.dealloc_count, 1);
        assert_eq!(s.alloc_bytes, 30);
        assert_eq!(s.dealloc_bytes, 10);
    }

    // 捕获 AllocStats::net_count/net_bytes -> 0/1/-1 及 `-`→`+`/`/` 变异体
    // 3 组数据（含 net=0 边界）使算术变异全部失败
    #[test]
    fn test_alloc_stats_arithmetic_strict() {
        for &(ac, ab, dc, db, nc, nb) in &[
            (10usize, 1000usize, 3usize, 300usize, 7isize, 700isize),
            (20, 2000, 5, 500, 15, 1500),
            (8, 80, 8, 80, 0, 0),
        ] {
            let s = AllocStats {
                alloc_count: ac,
                alloc_bytes: ab,
                dealloc_count: dc,
                dealloc_bytes: db,
            };
            assert_eq!(s.net_count(), nc, "net_count {ac}-{dc}");
            assert_eq!(s.net_bytes(), nb, "net_bytes {ab}-{db}");
        }
    }

    // 捕获 is_zero_alloc -> true/false 及 == -> != 变异体
    // 3 个实例（alloc_count=0/1/100）使常量替换与比较运算符变异失败
    #[test]
    fn test_alloc_stats_is_zero_alloc_strict() {
        let zero = AllocStats {
            alloc_count: 0,
            alloc_bytes: 0,
            dealloc_count: 0,
            dealloc_bytes: 0,
        };
        assert!(zero.is_zero_alloc());

        let one = AllocStats {
            alloc_count: 1,
            alloc_bytes: 8,
            dealloc_count: 0,
            dealloc_bytes: 0,
        };
        assert!(!one.is_zero_alloc());

        let large = AllocStats {
            alloc_count: 100,
            alloc_bytes: 8000,
            dealloc_count: 50,
            dealloc_bytes: 4000,
        };
        assert!(!large.is_zero_alloc());
    }

    // 捕获 Display::fmt -> Ok(Default::default()) 变异体
    // 精确字符串比较（而非 contains）使空输出与格式错误均失败
    #[test]
    fn test_alloc_stats_display_strict() {
        let s = AllocStats {
            alloc_count: 5,
            alloc_bytes: 500,
            dealloc_count: 2,
            dealloc_bytes: 200,
        };
        assert_eq!(format!("{s}"), "alloc=5/500, dealloc=2/200, net=3/300");

        let s2 = AllocStats {
            alloc_count: 10,
            alloc_bytes: 1000,
            dealloc_count: 4,
            dealloc_bytes: 400,
        };
        assert_eq!(format!("{s2}"), "alloc=10/1000, dealloc=4/400, net=6/600");
    }
}
