// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 内存池模块 — P3 性能优化
//!
//! 提供区域分配器（bump allocator）用于热点路径零堆分配。
//!
//! ## 模块
//!
//! - [`MemPool`] trait：统一内存池接口
//! - [`StackPool`]：栈分配后端（固定容量 `[u8; CAP]`，无依赖）
//! - [`create_pool`]：工厂函数
//!
//! ## 用法
//!
//! ```rust,ignore
//! use sz_rust_core::mem_pool::{MemPool, StackPool};
//!
//! let pool = StackPool::<1024>::new();
//! let s = unsafe { pool.alloc_str("hello") };
//! assert_eq!(s, "hello");
//! ```

#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

// ============================================================================
// MemPool trait
// ============================================================================

/// 内存池 trait — 区域分配器统一接口
///
/// 用于热点路径零堆分配：在固定容量缓冲区内分配字符串/字节切片，
/// 请求结束后 `reset()` 整体回收。
///
/// ## Safety（unsafe API 契约，2026-08-16 收紧）
///
/// `alloc_str` / `alloc_bytes` 返回的引用在 `reset()` **之前**有效；
/// `reset()` 后所有先前返回的引用**立即失效**（后续 alloc 会覆盖同一区域）。
/// 调用方必须保证：在使用返回引用期间不调用 `reset()`，否则构成 use-after-free。
///
/// 实现者需确保：
/// - `alloc_str` / `alloc_bytes` 返回的引用在 `reset()` 之前有效
/// - `reset()` 后所有先前返回的引用失效
/// - 线程安全：`&self` 方法可被多线程并发调用，并发 `reset` 与 `alloc` 需要调用方同步
///
/// # 为什么是 unsafe fn（设计决策）
///
/// 本 trait 用 `&self` + 生命周期延长实现零拷贝分配，Rust 借用检查器无法
/// 表达"引用在 reset 前有效"这一不变量（`reset` 是共享借用 `&self`）。
/// 若保持 safe fn，Safe Rust 调用方可在不知情时触发 UB（reset 后使用引用）。
/// 因此 `alloc_str`/`alloc_bytes` 为 **unsafe fn**：调用方必须显式承担契约。
pub trait MemPool: Send + Sync + std::fmt::Debug {
    /// 在池内分配字符串切片（零拷贝，返回池内引用）
    ///
    /// 如果池容量不足，回退返回输入切片本身（零分配）。
    ///
    /// ## Safety
    ///
    /// 返回的引用在 `reset()` 之前有效。调用方需保证在使用返回引用期间
    /// 不调用 `reset()`（包括其他线程的 `reset()`）。
    unsafe fn alloc_str<'a>(&self, s: &'a str) -> &'a str;

    /// 在池内分配字节切片（零拷贝，返回池内引用）
    ///
    /// 如果池容量不足，回退返回输入切片本身。
    ///
    /// ## Safety
    ///
    /// 同 [`MemPool::alloc_str`]：返回引用在 `reset()` 前有效，期间不得调用 `reset()`。
    unsafe fn alloc_bytes<'a>(&self, b: &'a [u8]) -> &'a [u8];

    /// 重置池，回收所有内存（整体回收到起始位置）
    fn reset(&self);

    /// 已使用字节数
    fn used_bytes(&self) -> usize;
}

// ============================================================================
// StackPool — 栈分配后端
// ============================================================================

/// 栈分配内存池（固定容量，无依赖）
///
/// 使用 `[u8; CAP]` 数组作为后端，`pos` 跟踪分配位置。
/// 适用于请求级临时分配，请求结束后 `reset()` 回收。
///
/// ## 线程安全
///
/// 使用 `UnsafeCell` + `AtomicUsize` 实现内部可变性，
/// 通过 `AtomicUsize::fetch_add` 原子递增分配位置，
/// 多线程并发分配安全（但分配的引用在 `reset` 后失效）。
pub struct StackPool<const CAP: usize> {
    buffer: UnsafeCell<[u8; CAP]>,
    pos: AtomicUsize,
}

impl<const CAP: usize> StackPool<CAP> {
    /// 创建 StackPool
    pub const fn new() -> Self {
        Self {
            buffer: UnsafeCell::new([0u8; CAP]),
            pos: AtomicUsize::new(0),
        }
    }

    /// 池总容量
    pub const fn capacity() -> usize {
        CAP
    }

    /// 剩余可用字节数
    pub fn remaining(&self) -> usize {
        CAP - self.pos.load(Ordering::Acquire)
    }
}

// Safety: StackPool 使用 AtomicUsize 管理分配位置，
// UnsafeCell 的内容通过原子操作安全访问。
unsafe impl<const CAP: usize> Send for StackPool<CAP> {}
unsafe impl<const CAP: usize> Sync for StackPool<CAP> {}

impl<const CAP: usize> MemPool for StackPool<CAP> {
    unsafe fn alloc_str<'a>(&self, s: &'a str) -> &'a str {
        let bytes = s.as_bytes();
        let len = bytes.len();
        if len == 0 {
            return "";
        }

        let start = self.pos.fetch_add(len, Ordering::AcqRel);
        if start + len > CAP {
            self.pos.fetch_sub(len, Ordering::AcqRel);
            return s;
        }

        // Safety: start + len <= CAP，buffer 有效
        let buf = unsafe { &mut *self.buffer.get() };
        buf[start..start + len].copy_from_slice(bytes);

        // Safety: 从 buf 切片构造 &str，字节来自有效 UTF-8 字符串。
        // 生命周期延长为 'a：区域分配器语义保证引用在 reset 前有效
        // （调用方通过 unsafe 块显式承担该契约，见 trait Safety 文档）。
        unsafe { std::str::from_utf8_unchecked(&buf[start..start + len]) }
    }

    unsafe fn alloc_bytes<'a>(&self, b: &'a [u8]) -> &'a [u8] {
        let len = b.len();
        if len == 0 {
            return &[];
        }

        let start = self.pos.fetch_add(len, Ordering::AcqRel);
        if start + len > CAP {
            self.pos.fetch_sub(len, Ordering::AcqRel);
            return b;
        }

        let buf = unsafe { &mut *self.buffer.get() };
        buf[start..start + len].copy_from_slice(b);
        &buf[start..start + len]
    }

    fn reset(&self) {
        self.pos.store(0, Ordering::Release);
    }

    fn used_bytes(&self) -> usize {
        self.pos.load(Ordering::Acquire)
    }
}

impl<const CAP: usize> Default for StackPool<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAP: usize> std::fmt::Debug for StackPool<CAP> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StackPool<{CAP}> used={}/{}", self.used_bytes(), CAP)
    }
}

// ============================================================================
// BumpaloPool — bumpalo 后端（可选 feature）
// ============================================================================

#[cfg(feature = "bumpalo-pool")]
mod bumpalo_backend {
    use super::*;
    use bumpalo::Bump;
    use std::sync::Mutex;

    /// bumpalo 内存池（可选 feature `bumpalo-pool`）
    ///
    /// 包装 `bumpalo::Bump`，提供与 [`StackPool`] 相同的 [`MemPool`] 接口。
    /// 适用于不确定容量的场景。
    pub struct BumpaloPool {
        bump: Mutex<Bump>,
        used: AtomicUsize,
    }

    impl BumpaloPool {
        /// 创建 BumpaloPool
        pub fn new() -> Self {
            Self {
                bump: Mutex::new(Bump::new()),
                used: AtomicUsize::new(0),
            }
        }
    }

    impl Default for BumpaloPool {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MemPool for BumpaloPool {
        unsafe fn alloc_str<'a>(&self, s: &'a str) -> &'a str {
            let bump = self.bump.lock().unwrap_or_else(|e| e.into_inner());
            let allocated = bump.alloc_str(s);
            self.used.fetch_add(s.len(), Ordering::Relaxed);
            // Safety: 分配的引用在 reset 前有效，bump 不会移动已分配内存
            // （调用方通过 unsafe 块显式承担契约，见 trait Safety 文档）。
            unsafe { std::mem::transmute::<&str, &'a str>(allocated) }
        }

        unsafe fn alloc_bytes<'a>(&self, b: &'a [u8]) -> &'a [u8] {
            let bump = self.bump.lock().unwrap_or_else(|e| e.into_inner());
            let allocated = bump.alloc_slice_copy(b);
            self.used.fetch_add(b.len(), Ordering::Relaxed);
            unsafe { std::mem::transmute::<&[u8], &'a [u8]>(allocated) }
        }

        fn reset(&self) {
            let mut bump = self.bump.lock().unwrap_or_else(|e| e.into_inner());
            bump.reset();
            self.used.store(0, Ordering::Release);
        }

        fn used_bytes(&self) -> usize {
            self.used.load(Ordering::Acquire)
        }
    }

    impl std::fmt::Debug for BumpaloPool {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "BumpaloPool used={}", self.used_bytes())
        }
    }
}

#[cfg(feature = "bumpalo-pool")]
pub use bumpalo_backend::BumpaloPool;

// ============================================================================
// 配置与工厂函数
// ============================================================================

/// 内存池类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemPoolType {
    /// bumpalo 后端（需 `bumpalo-pool` feature）
    Bumpalo,
    /// 栈分配后端
    Stack,
    /// 不使用内存池
    None,
}

/// 内存池配置
#[derive(Debug, Clone)]
pub struct MemPoolConfig {
    /// 池类型
    pub pool_type: MemPoolType,
    /// 容量（字节，仅 Stack 类型有效）
    pub capacity: usize,
}

impl Default for MemPoolConfig {
    fn default() -> Self {
        Self {
            pool_type: MemPoolType::Stack,
            capacity: 4096,
        }
    }
}

/// 工厂函数：根据配置创建内存池
pub fn create_pool(config: &MemPoolConfig) -> Option<Box<dyn MemPool>> {
    match config.pool_type {
        MemPoolType::Stack => match config.capacity {
            1024 => Some(Box::new(StackPool::<1024>::new())),
            2048 => Some(Box::new(StackPool::<2048>::new())),
            4096 => Some(Box::new(StackPool::<4096>::new())),
            8192 => Some(Box::new(StackPool::<8192>::new())),
            16384 => Some(Box::new(StackPool::<16384>::new())),
            32768 => Some(Box::new(StackPool::<32768>::new())),
            65536 => Some(Box::new(StackPool::<65536>::new())),
            _ => Some(Box::new(StackPool::<4096>::new())),
        },
        #[cfg(feature = "bumpalo-pool")]
        MemPoolType::Bumpalo => Some(Box::new(BumpaloPool::new())),
        #[cfg(not(feature = "bumpalo-pool"))]
        MemPoolType::Bumpalo => None,
        MemPoolType::None => None,
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stack_pool_alloc_str() {
        let pool = StackPool::<256>::new();
        let s1 = unsafe { pool.alloc_str("hello") };
        let s2 = unsafe { pool.alloc_str("world") };
        assert_eq!(s1, "hello");
        assert_eq!(s2, "world");
        assert_eq!(pool.used_bytes(), 10);
    }

    #[test]
    fn test_stack_pool_alloc_bytes() {
        let pool = StackPool::<256>::new();
        let b1 = unsafe { pool.alloc_bytes(&[1, 2, 3]) };
        let b2 = unsafe { pool.alloc_bytes(&[4, 5]) };
        assert_eq!(b1, &[1, 2, 3]);
        assert_eq!(b2, &[4, 5]);
        assert_eq!(pool.used_bytes(), 5);
    }

    #[test]
    fn test_stack_pool_capacity_overflow() {
        let pool = StackPool::<8>::new();
        let s1 = unsafe { pool.alloc_str("hello") };
        assert_eq!(s1, "hello");
        let s2 = unsafe { pool.alloc_str("world") };
        assert_eq!(s2, "world");
        assert_eq!(pool.used_bytes(), 5);
    }

    #[test]
    fn test_stack_pool_reset() {
        let pool = StackPool::<256>::new();
        let _ = unsafe { pool.alloc_str("hello") };
        assert_eq!(pool.used_bytes(), 5);
        pool.reset();
        assert_eq!(pool.used_bytes(), 0);
    }

    #[test]
    fn test_stack_pool_used_bytes() {
        let pool = StackPool::<256>::new();
        assert_eq!(pool.used_bytes(), 0);
        let _ = unsafe { pool.alloc_str("abc") };
        assert_eq!(pool.used_bytes(), 3);
        let _ = unsafe { pool.alloc_bytes(&[1, 2]) };
        assert_eq!(pool.used_bytes(), 5);
    }

    #[test]
    fn test_stack_pool_empty_alloc() {
        let pool = StackPool::<256>::new();
        let s = unsafe { pool.alloc_str("") };
        assert_eq!(s, "");
        assert_eq!(pool.used_bytes(), 0);
        let b = unsafe { pool.alloc_bytes(&[][..]) };
        assert!(b.is_empty());
        assert_eq!(pool.used_bytes(), 0);
    }

    #[test]
    fn test_stack_pool_remaining() {
        let pool = StackPool::<256>::new();
        assert_eq!(pool.remaining(), 256);
        let _ = unsafe { pool.alloc_str("hello") };
        assert_eq!(pool.remaining(), 251);
    }

    #[test]
    fn test_stack_pool_capacity() {
        assert_eq!(StackPool::<1024>::capacity(), 1024);
        assert_eq!(StackPool::<4096>::capacity(), 4096);
    }

    #[test]
    fn test_stack_pool_request_isolation() {
        let pool = StackPool::<256>::new();
        let s1 = unsafe { pool.alloc_str("request1") };
        assert_eq!(s1, "request1");
        pool.reset();
        let s2 = unsafe { pool.alloc_str("request2") };
        assert_eq!(s2, "request2");
        assert_eq!(pool.used_bytes(), 8);
    }

    #[test]
    fn test_create_pool_stack() {
        let config = MemPoolConfig::default();
        let pool = create_pool(&config).unwrap();
        let s = unsafe { pool.alloc_str("hello") };
        assert_eq!(s, "hello");
    }

    #[test]
    fn test_create_pool_none() {
        let config = MemPoolConfig {
            pool_type: MemPoolType::None,
            capacity: 0,
        };
        assert!(create_pool(&config).is_none());
    }

    #[test]
    fn test_create_pool_various_capacities() {
        for &cap in &[1024, 2048, 4096, 8192, 16384, 32768, 65536] {
            let config = MemPoolConfig {
                pool_type: MemPoolType::Stack,
                capacity: cap,
            };
            let pool = create_pool(&config).unwrap();
            let s = unsafe { pool.alloc_str("test") };
            assert_eq!(s, "test");
        }
    }

    #[test]
    fn test_mempool_config_default() {
        let config = MemPoolConfig::default();
        assert_eq!(config.pool_type, MemPoolType::Stack);
        assert_eq!(config.capacity, 4096);
    }

    // 捕获 StackPool Debug -> Ok(Default::default()) 变异体（missed.txt 第 55 行）
    #[test]
    fn test_stack_pool_debug_format() {
        let pool = StackPool::<256>::new();
        unsafe {
            pool.alloc_str("hello");
        }
        let s = format!("{:?}", pool);
        assert!(s.contains("StackPool<256>"), "debug={s}");
        assert!(s.contains("used=5"), "debug={s}");
    }

    // 捕获 create_pool -> None/Some(Default) 与 7 个 delete match arm 变异体（missed.txt 第 60-68 行）
    #[test]
    fn test_create_pool_capacity_matches_each_arm() {
        for c in [1024, 2048, 4096, 8192, 16384, 32768, 65536] {
            let config = MemPoolConfig {
                pool_type: MemPoolType::Stack,
                capacity: c,
            };
            let pool = create_pool(&config).expect("capacity");
            let s = format!("{:?}", pool);
            assert!(
                s.contains(&format!("StackPool<{c}>")),
                "capacity={c} debug={s}"
            );
        }
    }

    // ===== 第二轮变异测试补强：多组值严格断言 =====

    // 捕获 capacity() -> 0/1 变异体：8 组 CAP 值（含 256、65536 等非 0/1 值）
    #[test]
    fn test_stack_pool_capacity_strict() {
        assert_eq!(StackPool::<256>::capacity(), 256);
        assert_eq!(StackPool::<1024>::capacity(), 1024);
        assert_eq!(StackPool::<2048>::capacity(), 2048);
        assert_eq!(StackPool::<4096>::capacity(), 4096);
        assert_eq!(StackPool::<8192>::capacity(), 8192);
        assert_eq!(StackPool::<16384>::capacity(), 16384);
        assert_eq!(StackPool::<32768>::capacity(), 32768);
        assert_eq!(StackPool::<65536>::capacity(), 65536);
    }

    // 捕获 remaining() -> 0/1 及 `-`→`+`/`/` 变异体
    // 多组使用量使常量替换与算术变异失败
    #[test]
    fn test_stack_pool_remaining_strict() {
        let pool = StackPool::<256>::new();
        // 初始 remaining = 256 - 0 = 256
        assert_eq!(pool.remaining(), 256);
        // 分配 5 字节 → 256 - 5 = 251
        unsafe {
            pool.alloc_str("hello");
        }
        assert_eq!(pool.remaining(), 251);
        // 再分配 10 字节 → 256 - 15 = 241
        unsafe {
            pool.alloc_bytes(&[1u8; 10]);
        }
        assert_eq!(pool.remaining(), 241);
        // reset → 256 - 0 = 256
        pool.reset();
        assert_eq!(pool.remaining(), 256);
    }

    // 捕获 reset() -> () 变异体：reset 后 used_bytes 必须为 0、remaining 必须为 CAP
    #[test]
    fn test_stack_pool_reset_strict() {
        let pool = StackPool::<512>::new();
        unsafe {
            pool.alloc_str("abcdefghij"); // 10 字节
            pool.alloc_bytes(&[0u8; 20]);
        }
        assert_eq!(pool.used_bytes(), 30);
        pool.reset();
        assert_eq!(pool.used_bytes(), 0, "reset 后 used_bytes 必须为 0");
        assert_eq!(pool.remaining(), 512, "reset 后 remaining 必须为 CAP");
        // reset 后可重新分配（验证 reset 真正清零而非 no-op）
        unsafe {
            pool.alloc_str("xyz");
        }
        assert_eq!(pool.used_bytes(), 3);
    }

    // 捕获 used_bytes() -> 0/1 变异体：多组分配量
    #[test]
    fn test_stack_pool_used_bytes_strict() {
        let pool = StackPool::<1024>::new();
        assert_eq!(pool.used_bytes(), 0);
        unsafe {
            pool.alloc_str("ab"); // 2
        }
        assert_eq!(pool.used_bytes(), 2);
        unsafe {
            pool.alloc_bytes(&[1u8; 8]); // 8
        }
        assert_eq!(pool.used_bytes(), 10);
        unsafe {
            pool.alloc_str("hello world"); // 11
        }
        assert_eq!(pool.used_bytes(), 21);
    }

    // 捕获 Debug::fmt -> Ok(Default::default()) 变异体：精确格式比较
    #[test]
    fn test_stack_pool_debug_strict() {
        let pool = StackPool::<1024>::new();
        assert_eq!(format!("{:?}", pool), "StackPool<1024> used=0/1024");
        unsafe {
            pool.alloc_str("hello"); // 5
        }
        assert_eq!(format!("{:?}", pool), "StackPool<1024> used=5/1024");
    }

    // 捕获 create_pool 每个 match arm 删除变异体：验证返回池的 capacity 与请求一致
    // 通过 Debug 格式中的 <CAP> 确认每个 arm 返回正确容量的 StackPool
    #[test]
    fn test_create_pool_each_arm_capacity_strict() {
        for cap in [1024, 2048, 4096, 8192, 16384, 32768, 65536] {
            let config = MemPoolConfig {
                pool_type: MemPoolType::Stack,
                capacity: cap,
            };
            let pool = create_pool(&config).expect("Stack pool should be created");
            let debug = format!("{:?}", pool);
            // 精确验证 Debug 输出包含 "StackPool<cap> used=0/cap"
            assert_eq!(
                debug,
                format!("StackPool<{cap}> used=0/{cap}"),
                "capacity={cap} 的池应返回 StackPool<{cap}>"
            );
        }
    }
}

// 捕获 BumpaloPool reset/used_bytes/Debug 变异体（missed.txt 第 56-59 行）
#[cfg(all(test, feature = "bumpalo-pool"))]
mod bumpalo_tests {
    use super::*;

    #[test]
    fn test_bumpalo_pool_lifecycle() {
        let pool = BumpaloPool::new();
        unsafe {
            pool.alloc_str("abc");
        }
        assert!(pool.used_bytes() > 0);
        let s = format!("{:?}", pool);
        assert!(s.contains("BumpaloPool"), "debug={s}");
        pool.reset();
        assert_eq!(pool.used_bytes(), 0);
    }
}
