// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 灰度规则匹配引擎（T018）

use std::net::IpAddr;

use crate::source::GrayRule;

/// 灰度匹配引擎
pub struct GrayMatcher;

impl GrayMatcher {
    /// 判断灰度规则是否匹配当前实例
    ///
    /// # 参数
    ///
    /// - `rule`: 灰度规则
    /// - `ip`: 当前实例 IP
    /// - `tenant`: 当前租户 ID
    /// - `hash_key`: 一致性哈希键（用于百分比灰度）
    pub fn matches(
        rule: &GrayRule,
        ip: Option<&IpAddr>,
        tenant: Option<&str>,
        hash_key: Option<&str>,
    ) -> bool {
        match rule {
            GrayRule::ByIp(nets) => {
                let Some(ip) = ip else {
                    return false;
                };
                nets.iter().any(|net| net.contains(ip))
            }
            GrayRule::ByTenant(tenants) => {
                let Some(tenant) = tenant else {
                    return false;
                };
                tenants.iter().any(|t| t == tenant)
            }
            GrayRule::ByPercent(percent) => {
                let Some(key) = hash_key else {
                    return false;
                };
                let hash = Self::hash(key);
                let threshold = (*percent * u64::MAX as f64) as u64;
                hash < threshold
            }
        }
    }

    /// 简单 FNV-1a 哈希
    fn hash(s: &str) -> u64 {
        let mut h: u64 = 14695981039346656037;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(1099511628211);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_match_by_ip_in_range() {
        let rule = GrayRule::ByIp(vec!["192.168.1.0/24".parse().unwrap()]);
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        assert!(GrayMatcher::matches(&rule, Some(&ip), None, None));
    }

    #[test]
    fn test_match_by_ip_out_of_range() {
        let rule = GrayRule::ByIp(vec!["192.168.1.0/24".parse().unwrap()]);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert!(!GrayMatcher::matches(&rule, Some(&ip), None, None));
    }

    #[test]
    fn test_match_by_ip_none() {
        let rule = GrayRule::ByIp(vec!["192.168.1.0/24".parse().unwrap()]);
        assert!(!GrayMatcher::matches(&rule, None, None, None));
    }

    #[test]
    fn test_match_by_tenant_match() {
        let rule = GrayRule::ByTenant(vec!["tenant_a".to_string()]);
        assert!(GrayMatcher::matches(&rule, None, Some("tenant_a"), None));
    }

    #[test]
    fn test_match_by_tenant_no_match() {
        let rule = GrayRule::ByTenant(vec!["tenant_a".to_string()]);
        assert!(!GrayMatcher::matches(&rule, None, Some("tenant_b"), None));
    }

    #[test]
    fn test_match_by_tenant_none() {
        let rule = GrayRule::ByTenant(vec!["tenant_a".to_string()]);
        assert!(!GrayMatcher::matches(&rule, None, None, None));
    }

    #[test]
    fn test_match_by_percent() {
        let rule = GrayRule::ByPercent(1.0);
        assert!(GrayMatcher::matches(&rule, None, None, Some("any_key")));
    }

    #[test]
    fn test_match_by_percent_zero() {
        let rule = GrayRule::ByPercent(0.0);
        assert!(!GrayMatcher::matches(&rule, None, None, Some("any_key")));
    }

    #[test]
    fn test_match_by_percent_none_key() {
        let rule = GrayRule::ByPercent(0.5);
        assert!(!GrayMatcher::matches(&rule, None, None, None));
    }

    #[test]
    fn test_hash_deterministic() {
        assert_eq!(GrayMatcher::hash("test"), GrayMatcher::hash("test"));
        assert_ne!(GrayMatcher::hash("test"), GrayMatcher::hash("other"));
    }
}
