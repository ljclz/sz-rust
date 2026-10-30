// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 多维度限流（P3-3）
//!
//! 用户/IP/接口/全局四维度独立计数 + 多算法支持。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::error::GatewayError;
use crate::leaky_bucket::LeakyBucket;
use crate::sliding_window::SlidingWindow;

/// 限流维度位掩码
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitDimensions(pub u8);

impl RateLimitDimensions {
    /// 用户维度
    pub const USER: Self = Self(1);
    /// IP 维度
    pub const IP: Self = Self(2);
    /// 接口维度
    pub const API: Self = Self(4);
    /// 全局维度
    pub const GLOBAL: Self = Self(8);
    /// 所有维度
    pub const ALL: Self = Self(15);

    /// 创建空维度
    pub const fn none() -> Self {
        Self(0)
    }

    /// 合并维度
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// 是否包含指定维度
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// 限流算法
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitAlgorithm {
    /// 令牌桶
    TokenBucket,
    /// 滑动窗口
    SlidingWindow,
    /// 漏桶
    LeakyBucket,
}

/// 多维度限流配置
#[derive(Debug, Clone)]
pub struct MultiDimRateLimitConfig {
    /// 启用的维度
    pub dimensions: RateLimitDimensions,
    /// 限流算法
    pub algorithm: RateLimitAlgorithm,
    /// 配额
    pub quota: u32,
    /// 时间窗口
    pub window: Duration,
}

/// 请求上下文
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// 用户 ID
    pub user_id: Option<String>,
    /// 客户端 IP
    pub ip: Option<String>,
    /// API 路径
    pub api_path: Option<String>,
}

impl RequestContext {
    /// 创建请求上下文
    pub fn new() -> Self {
        Self {
            user_id: None,
            ip: None,
            api_path: None,
        }
    }

    /// 设置用户 ID
    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// 设置 IP
    pub fn with_ip(mut self, ip: impl Into<String>) -> Self {
        self.ip = Some(ip.into());
        self
    }

    /// 设置 API 路径
    pub fn with_api(mut self, api_path: impl Into<String>) -> Self {
        self.api_path = Some(api_path.into());
        self
    }
}

impl Default for RequestContext {
    fn default() -> Self {
        Self::new()
    }
}

/// 限流计数器（算法封装）
enum Counter {
    SlidingWindow(SlidingWindow),
    LeakyBucket(LeakyBucket),
}

impl Counter {
    fn new(algorithm: RateLimitAlgorithm, quota: u32, window: Duration) -> Self {
        match algorithm {
            RateLimitAlgorithm::SlidingWindow | RateLimitAlgorithm::TokenBucket => {
                Counter::SlidingWindow(SlidingWindow::new(quota, window))
            }
            RateLimitAlgorithm::LeakyBucket => {
                let leak_rate = quota as f64 / window.as_secs_f64().max(0.001);
                Counter::LeakyBucket(LeakyBucket::new(quota, leak_rate))
            }
        }
    }

    fn try_acquire(&self) -> bool {
        match self {
            Counter::SlidingWindow(sw) => sw.try_acquire(),
            Counter::LeakyBucket(lb) => lb.try_acquire(),
        }
    }
}

/// 多维度限流器
///
/// 用户/IP/接口/全局四维度独立计数，任一维度限流即拒绝。
pub struct MultiDimRateLimit {
    config: MultiDimRateLimitConfig,
    user_counters: Mutex<HashMap<String, Arc<Counter>>>,
    ip_counters: Mutex<HashMap<String, Arc<Counter>>>,
    api_counters: Mutex<HashMap<String, Arc<Counter>>>,
    global_counter: Mutex<Option<Arc<Counter>>>,
}

impl MultiDimRateLimit {
    /// 创建多维度限流器
    pub fn new(config: MultiDimRateLimitConfig) -> Self {
        Self {
            config,
            user_counters: Mutex::new(HashMap::new()),
            ip_counters: Mutex::new(HashMap::new()),
            api_counters: Mutex::new(HashMap::new()),
            global_counter: Mutex::new(None),
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &MultiDimRateLimitConfig {
        &self.config
    }

    /// 检查请求是否允许通过
    ///
    /// 对启用的每个维度独立检查，任一维度限流即拒绝。
    pub fn check(&self, ctx: &RequestContext) -> Result<(), GatewayError> {
        let dims = self.config.dimensions;

        if dims.contains(RateLimitDimensions::USER) {
            if let Some(uid) = &ctx.user_id {
                if !self.check_dimension(&self.user_counters, uid) {
                    return Err(GatewayError::RateLimited(format!("user={uid}")));
                }
            }
        }

        if dims.contains(RateLimitDimensions::IP) {
            if let Some(ip) = &ctx.ip {
                if !self.check_dimension(&self.ip_counters, ip) {
                    return Err(GatewayError::RateLimited(format!("ip={ip}")));
                }
            }
        }

        if dims.contains(RateLimitDimensions::API) {
            if let Some(api) = &ctx.api_path {
                if !self.check_dimension(&self.api_counters, api) {
                    return Err(GatewayError::RateLimited(format!("api={api}")));
                }
            }
        }

        if dims.contains(RateLimitDimensions::GLOBAL) {
            let mut global = self.global_counter.lock();
            let counter = global.get_or_insert_with(|| {
                Arc::new(Counter::new(
                    self.config.algorithm,
                    self.config.quota,
                    self.config.window,
                ))
            });
            if !counter.try_acquire() {
                return Err(GatewayError::RateLimited("global".to_string()));
            }
        }

        Ok(())
    }

    fn check_dimension(&self, counters: &Mutex<HashMap<String, Arc<Counter>>>, key: &str) -> bool {
        let mut map = counters.lock();
        let counter = map.entry(key.to_string()).or_insert_with(|| {
            Arc::new(Counter::new(
                self.config.algorithm,
                self.config.quota,
                self.config.window,
            ))
        });
        counter.try_acquire()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(dims: RateLimitDimensions, algo: RateLimitAlgorithm) -> MultiDimRateLimitConfig {
        MultiDimRateLimitConfig {
            dimensions: dims,
            algorithm: algo,
            quota: 2,
            window: Duration::from_secs(10),
        }
    }

    #[test]
    fn test_dimensions_union_and_contains() {
        let dims = RateLimitDimensions::USER.union(RateLimitDimensions::IP);
        assert!(dims.contains(RateLimitDimensions::USER));
        assert!(dims.contains(RateLimitDimensions::IP));
        assert!(!dims.contains(RateLimitDimensions::API));
    }

    #[test]
    fn test_dimensions_union_same_bit() {
        // 相同位合并：`self.0 | other.0` 的 `|`→`^` 变异体会得到 0，
        // 导致 contains(USER) 为 false，从而被杀死。
        let dims = RateLimitDimensions::USER.union(RateLimitDimensions::USER);
        assert!(
            dims.contains(RateLimitDimensions::USER),
            "USER 与自身合并应仍是 USER"
        );
    }

    #[test]
    fn test_user_dimension_independent() {
        let rl = MultiDimRateLimit::new(config(
            RateLimitDimensions::USER,
            RateLimitAlgorithm::SlidingWindow,
        ));

        let ctx1 = RequestContext::new().with_user("alice");
        let ctx2 = RequestContext::new().with_user("bob");

        assert!(rl.check(&ctx1).is_ok());
        assert!(rl.check(&ctx1).is_ok());
        assert!(rl.check(&ctx1).is_err(), "alice should be limited");

        assert!(rl.check(&ctx2).is_ok(), "bob should be independent");
    }

    #[test]
    fn test_ip_dimension_independent() {
        let rl = MultiDimRateLimit::new(config(
            RateLimitDimensions::IP,
            RateLimitAlgorithm::SlidingWindow,
        ));

        let ctx1 = RequestContext::new().with_ip("10.0.0.1");
        let ctx2 = RequestContext::new().with_ip("10.0.0.2");

        assert!(rl.check(&ctx1).is_ok());
        assert!(rl.check(&ctx1).is_ok());
        assert!(rl.check(&ctx1).is_err());

        assert!(rl.check(&ctx2).is_ok(), "different IP independent");
    }

    #[test]
    fn test_api_dimension_independent() {
        let rl = MultiDimRateLimit::new(config(
            RateLimitDimensions::API,
            RateLimitAlgorithm::SlidingWindow,
        ));

        let ctx1 = RequestContext::new().with_api("/api/v1/users");
        let ctx2 = RequestContext::new().with_api("/api/v1/orders");

        assert!(rl.check(&ctx1).is_ok());
        assert!(rl.check(&ctx1).is_ok());
        assert!(rl.check(&ctx1).is_err());

        assert!(rl.check(&ctx2).is_ok(), "different API independent");
    }

    #[test]
    fn test_global_dimension() {
        let rl = MultiDimRateLimit::new(config(
            RateLimitDimensions::GLOBAL,
            RateLimitAlgorithm::SlidingWindow,
        ));

        let ctx = RequestContext::new().with_user("alice");
        let ctx2 = RequestContext::new().with_user("bob");

        assert!(rl.check(&ctx).is_ok());
        assert!(rl.check(&ctx2).is_ok());
        assert!(rl.check(&ctx).is_err(), "global limit reached");
    }

    #[test]
    fn test_multiple_dimensions_any_limit() {
        let dims = RateLimitDimensions::USER.union(RateLimitDimensions::IP);
        let rl = MultiDimRateLimit::new(config(dims, RateLimitAlgorithm::SlidingWindow));

        let ctx = RequestContext::new().with_user("alice").with_ip("10.0.0.1");

        assert!(rl.check(&ctx).is_ok());
        assert!(rl.check(&ctx).is_ok());
        assert!(rl.check(&ctx).is_err(), "either user or ip limit triggered");
    }

    #[test]
    fn test_leaky_bucket_algorithm() {
        let rl = MultiDimRateLimit::new(config(
            RateLimitDimensions::USER,
            RateLimitAlgorithm::LeakyBucket,
        ));

        let ctx = RequestContext::new().with_user("alice");
        assert!(rl.check(&ctx).is_ok());
        assert!(rl.check(&ctx).is_ok());
        assert!(rl.check(&ctx).is_err());
    }

    #[test]
    fn test_no_dimensions_allows_all() {
        let rl = MultiDimRateLimit::new(config(
            RateLimitDimensions::none(),
            RateLimitAlgorithm::SlidingWindow,
        ));

        let ctx = RequestContext::new().with_user("alice");
        for _ in 0..100 {
            assert!(rl.check(&ctx).is_ok());
        }
    }

    #[test]
    fn test_missing_context_field_skips_dimension() {
        let rl = MultiDimRateLimit::new(config(
            RateLimitDimensions::USER,
            RateLimitAlgorithm::SlidingWindow,
        ));

        let ctx = RequestContext::new();
        assert!(rl.check(&ctx).is_ok(), "no user_id → skip user dimension");
    }
}
