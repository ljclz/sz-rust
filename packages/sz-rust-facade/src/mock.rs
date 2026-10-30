// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Mock 注入（feature = "test-utils"）
//!
//! 提供 `replace_*` 函数注入 Mock 实例，`MockGuard` RAII Drop 时自动还原。
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_facade::mock::replace_cache;
//! use sz_rust_cache_facade::Cache;
//!
//! let mock_cache = Cache::new();
//! let guard = replace_cache(mock_cache);
//! // 测试中使用 mock_cache
//! drop(guard); // 自动还原
//! ```

use std::sync::Arc;

use arc_swap::ArcSwapOption;

use sz_rust_cache_facade::Cache as CacheInner;

static CACHE_MOCK: ArcSwapOption<CacheInner> = ArcSwapOption::const_empty();

/// MockGuard RAII 守卫
///
/// Drop 时自动清除 Mock override，还原到真实实例。
pub struct MockGuard {
    kind: MockKind,
}

enum MockKind {
    Cache,
}

impl Drop for MockGuard {
    fn drop(&mut self) {
        match self.kind {
            MockKind::Cache => {
                CACHE_MOCK.store(None);
            }
        }
    }
}

/// 替换 Cache 门面为 Mock 实例
///
/// 返回 [`MockGuard`]，Drop 时自动还原。
pub fn replace_cache(mock: CacheInner) -> MockGuard {
    CACHE_MOCK.store(Some(Arc::new(mock)));
    MockGuard {
        kind: MockKind::Cache,
    }
}

/// 获取 Cache Mock override（如有）
pub fn cache_mock() -> Option<Arc<CacheInner>> {
    CACHE_MOCK.load_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mock_cache_replace_and_restore() {
        // 注入 mock
        let mock = CacheInner::new();
        mock.register_default(sz_rust_cache_facade::MemoryCacheDriver::new());
        mock.set("mock_key", &json!("mock_value"), None).unwrap();

        let guard = replace_cache(mock);

        // 验证 mock 生效
        let mock_ref = cache_mock().unwrap();
        let val: Option<serde_json::Value> = mock_ref.get("mock_key").unwrap();
        assert_eq!(val, Some(json!("mock_value")));

        // Drop guard → 自动还原
        drop(guard);
        assert!(cache_mock().is_none());
    }
}
