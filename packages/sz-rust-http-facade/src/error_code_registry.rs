// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 错误码统一注册表 — 全局错误码元信息管理
//!
//! 提供线程安全的错误码注册、查询、列举能力，用于在启动期统一登记
//! 全仓错误码（code / name / description），避免不同模块重复定义冲突。
//!
//! ## 设计
//!
//! - 使用 [`parking_lot::RwLock`] 保证多线程并发读、互斥写。
//! - 注册时若 code 已存在，返回 [`ErrorCodeConflict`]，包含已有 entry 信息。
//! - 不使用 unsafe，符合 workspace `unsafe_code = "forbid"` 约束。
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_http_facade::error_code_registry::ErrorCodeRegistry;
//!
//! let registry = ErrorCodeRegistry::new();
//! registry.register(1001, "USER_NOT_FOUND".to_string(), "用户不存在".to_string()).unwrap();
//! assert!(registry.contains(1001));
//! ```

use std::collections::HashMap;

use parking_lot::RwLock;
use thiserror::Error;

/// 错误码注册表条目
///
/// 描述单个错误码的元信息：数值、名称（大写常量风格）、人类可读描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorCodeEntry {
    /// 错误码数值（对齐 PHP `BaseException::$code`）
    pub code: i32,
    /// 错误码名称（大写常量风格，如 `USER_NOT_FOUND`）
    pub name: String,
    /// 错误码描述（人类可读，用于文档与日志）
    pub description: String,
}

/// 错误码注册冲突错误
///
/// 当 [`ErrorCodeRegistry::register`] 检测到 code 已存在时返回。
/// 携带已有 entry 信息，便于调用方定位冲突来源。
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error(
    "error code {conflict_code} already registered as `{existing_name}`: {existing_description}"
)]
pub struct ErrorCodeConflict {
    /// 冲突的错误码数值
    pub conflict_code: i32,
    /// 已注册 entry 的名称
    pub existing_name: String,
    /// 已注册 entry 的描述
    pub existing_description: String,
}

/// 错误码统一注册表
///
/// 线程安全的全局错误码登记表。启动期由各模块注册自身错误码，
/// 运行期通过 [`get`](Self::get) / [`contains`](Self::contains) 查询。
///
/// ## 并发模型
///
/// - 读操作（`get` / `contains` / `list`）取读锁，多线程并发无阻塞。
/// - 写操作（`register`）取写锁，互斥。
///
/// ## 不变量
///
/// - 同一 code 不可重复注册；重复注册返回 [`ErrorCodeConflict`]。
/// - name / description 不做唯一性约束（仅 code 唯一）。
pub struct ErrorCodeRegistry {
    entries: RwLock<HashMap<i32, ErrorCodeEntry>>,
}

impl ErrorCodeRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }

    /// 注册错误码
    ///
    /// # 参数
    ///
    /// - `code`：错误码数值（唯一键）
    /// - `name`：错误码名称（大写常量风格）
    /// - `description`：人类可读描述
    ///
    /// # 返回
    ///
    /// - `Ok(())`：注册成功
    /// - `Err(ErrorCodeConflict)`：code 已存在，错误中携带已有 entry 信息
    ///
    /// # 并发
    ///
    /// 取写锁，与所有读/写操作互斥。
    pub fn register(
        &self,
        code: i32,
        name: String,
        description: String,
    ) -> Result<(), ErrorCodeConflict> {
        let mut entries = self.entries.write();
        if let Some(existing) = entries.get(&code) {
            return Err(ErrorCodeConflict {
                conflict_code: code,
                existing_name: existing.name.clone(),
                existing_description: existing.description.clone(),
            });
        }
        entries.insert(
            code,
            ErrorCodeEntry {
                code,
                name,
                description,
            },
        );
        Ok(())
    }

    /// 查询错误码条目
    ///
    /// # 返回
    ///
    /// - `Some(&ErrorCodeEntry)`：存在
    /// - `None`：不存在
    ///
    /// # 并发
    ///
    /// 取读锁，多线程可并发查询。
    pub fn get(&self, code: i32) -> Option<ErrorCodeEntry> {
        self.entries.read().get(&code).cloned()
    }

    /// 判断错误码是否已注册
    ///
    /// # 并发
    ///
    /// 取读锁。
    pub fn contains(&self, code: i32) -> bool {
        self.entries.read().contains_key(&code)
    }

    /// 列举所有已注册错误码条目
    ///
    /// 按 code 升序返回，便于文档生成与诊断输出。
    ///
    /// # 并发
    ///
    /// 取读锁。
    pub fn list(&self) -> Vec<ErrorCodeEntry> {
        let entries = self.entries.read();
        let mut all: Vec<ErrorCodeEntry> = entries.values().cloned().collect();
        all.sort_by_key(|e| e.code);
        all
    }

    /// 已注册错误码数量
    ///
    /// # 并发
    ///
    /// 取读锁。
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// 注册表是否为空
    ///
    /// # 并发
    ///
    /// 取读锁。
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }
}

impl Default for ErrorCodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ErrorCodeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let entries = self.entries.read();
        f.debug_struct("ErrorCodeRegistry")
            .field("count", &entries.len())
            .field("codes", &{
                let mut codes: Vec<i32> = entries.keys().copied().collect();
                codes.sort_unstable();
                codes
            })
            .finish()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_get() {
        let registry = ErrorCodeRegistry::new();
        assert!(registry.is_empty());
        assert!(registry
            .register(1001, "USER_NOT_FOUND".to_string(), "用户不存在".to_string())
            .is_ok());
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);

        let entry = registry.get(1001).expect("1001 should be registered");
        assert_eq!(entry.code, 1001);
        assert_eq!(entry.name, "USER_NOT_FOUND");
        assert_eq!(entry.description, "用户不存在");
    }

    #[test]
    fn test_register_conflict() {
        let registry = ErrorCodeRegistry::new();
        assert!(registry
            .register(1002, "USER_DISABLED".to_string(), "用户已禁用".to_string())
            .is_ok());
        let err = registry
            .register(1002, "DUPLICATE_CODE".to_string(), "重复注册".to_string())
            .expect_err("duplicate registration should fail");
        assert_eq!(err.conflict_code, 1002);
        assert_eq!(err.existing_name, "USER_DISABLED");
        assert_eq!(err.existing_description, "用户已禁用");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_contains() {
        let registry = ErrorCodeRegistry::new();
        registry
            .register(404, "NOT_FOUND".to_string(), "资源不存在".to_string())
            .unwrap();
        assert!(registry.contains(404));
        assert!(!registry.contains(405));
    }

    #[test]
    fn test_get_nonexistent() {
        let registry = ErrorCodeRegistry::new();
        assert!(registry.get(9999).is_none());
    }

    #[test]
    fn test_list_sorted_by_code() {
        let registry = ErrorCodeRegistry::new();
        registry
            .register(3, "C".to_string(), "third".to_string())
            .unwrap();
        registry
            .register(1, "A".to_string(), "first".to_string())
            .unwrap();
        registry
            .register(2, "B".to_string(), "second".to_string())
            .unwrap();
        let all = registry.list();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].code, 1);
        assert_eq!(all[1].code, 2);
        assert_eq!(all[2].code, 3);
    }

    #[test]
    fn test_default_is_empty() {
        let registry = ErrorCodeRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_negative_code() {
        let registry = ErrorCodeRegistry::new();
        assert!(registry
            .register(-1, "NOT_LOGIN".to_string(), "未登录".to_string())
            .is_ok());
        assert!(registry.contains(-1));
        let entry = registry.get(-1).expect("-1 should be registered");
        assert_eq!(entry.code, -1);
    }

    #[test]
    fn test_concurrent_register() {
        use std::sync::Arc;
        use std::thread;

        let registry = Arc::new(ErrorCodeRegistry::new());
        let mut handles = Vec::new();
        for i in 0..100i32 {
            let registry = Arc::clone(&registry);
            handles.push(thread::spawn(move || {
                registry
                    .register(i, format!("CODE_{i}"), format!("description {i}"))
                    .is_ok()
            }));
        }
        let ok_count: usize = handles
            .into_iter()
            .map(|h| h.join().unwrap() as usize)
            .sum();
        assert_eq!(ok_count, 100);
        assert_eq!(registry.len(), 100);
    }

    #[test]
    fn test_concurrent_read_while_write() {
        use std::sync::Arc;
        use std::thread;

        let registry = Arc::new(ErrorCodeRegistry::new());
        for i in 0..50i32 {
            registry
                .register(i, format!("CODE_{i}"), format!("description {i}"))
                .unwrap();
        }
        let mut handles = Vec::new();
        for _ in 0..10 {
            let registry = Arc::clone(&registry);
            handles.push(thread::spawn(move || {
                let mut found = 0;
                for i in 0..50i32 {
                    if registry.contains(i) {
                        found += 1;
                    }
                }
                found
            }));
        }
        for h in handles {
            assert_eq!(h.join().unwrap(), 50);
        }
    }

    #[test]
    fn test_conflict_error_display() {
        let err = ErrorCodeConflict {
            conflict_code: 1003,
            existing_name: "EXISTING".to_string(),
            existing_description: "已存在".to_string(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("1003"));
        assert!(msg.contains("EXISTING"));
        assert!(msg.contains("已存在"));
    }

    #[test]
    fn test_debug_format() {
        let registry = ErrorCodeRegistry::new();
        registry
            .register(1, "A".to_string(), "first".to_string())
            .unwrap();
        registry
            .register(2, "B".to_string(), "second".to_string())
            .unwrap();
        let debug = format!("{registry:?}");
        assert!(debug.contains("ErrorCodeRegistry"));
        assert!(debug.contains("count: 2"));
    }

    #[test]
    fn test_entry_equality() {
        let e1 = ErrorCodeEntry {
            code: 1,
            name: "A".to_string(),
            description: "desc".to_string(),
        };
        let e2 = ErrorCodeEntry {
            code: 1,
            name: "A".to_string(),
            description: "desc".to_string(),
        };
        assert_eq!(e1, e2);
    }
}
