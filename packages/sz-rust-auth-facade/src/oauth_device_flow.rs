// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! OAuth2 设备码流程（spec 5.13.1 规则 5-8）
//!
//! 颁发 device_code + user_code 供无浏览器设备授权，
//! 支持轮询状态机 + 过期处理 + slow_down 错误。

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rand::rngs::OsRng;
use rand::RngCore;
use thiserror::Error;

/// OAuth2 设备码错误
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeviceCodeError {
    /// 设备码不存在或已过期
    #[error("expired_token")]
    ExpiredToken,
    /// 授权待完成
    #[error("authorization_pending")]
    AuthorizationPending,
    /// 轮询过频
    #[error("slow_down")]
    SlowDown,
    /// 设备码已被消费
    #[error("device_code already consumed")]
    Consumed,
}

/// 设备码配置（spec 5.13.1 规则 7/8）
#[derive(Debug, Clone)]
pub struct DeviceCodeConfig {
    /// 设备码过期时间（默认 900s，上限 1800s）
    pub expiry: Duration,
    /// 轮询间隔（默认 5s）
    pub poll_interval: Duration,
    /// 验证 URI（用户在浏览器中访问）
    pub verification_uri: String,
}

impl Default for DeviceCodeConfig {
    fn default() -> Self {
        Self {
            expiry: Duration::from_secs(900),
            poll_interval: Duration::from_secs(5),
            verification_uri: "https://example.com/device".to_string(),
        }
    }
}

/// 设备码响应（spec 5.13.1 规则 5）
#[derive(Debug, Clone)]
pub struct DeviceCodeResponse {
    /// 设备码（供设备轮询用）
    pub device_code: String,
    /// 用户码（供用户在浏览器输入）
    pub user_code: String,
    /// 验证 URI
    pub verification_uri: String,
    /// 轮询间隔（秒）
    pub poll_interval: u64,
    /// 过期时间（秒）
    pub expires_in: u64,
}

/// 轮询结果（spec 5.13.1 规则 6/7/8）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollResult {
    /// 授权完成，返回令牌
    Token {
        /// 访问令牌
        access_token: String,
        /// 刷新令牌（可选）
        refresh_token: Option<String>,
    },
    /// 待授权
    AuthorizationPending,
    /// 轮询过频
    SlowDown,
    /// 设备码过期
    ExpiredToken,
}

/// 设备码内部状态
#[derive(Debug)]
struct DeviceCodeEntry {
    user_code: String,
    #[allow(dead_code)]
    client_id: String,
    #[allow(dead_code)]
    scope: Vec<String>,
    created_at: Instant,
    last_poll: Mutex<Option<Instant>>,
    status: Mutex<DeviceCodeStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeviceCodeStatus {
    Pending,
    Authorized {
        access_token: String,
        refresh_token: Option<String>,
    },
    Consumed,
}

/// 设备码存储（内存实现，Redis 可后续扩展）
struct DeviceCodeStore {
    entries: Mutex<HashMap<String, DeviceCodeEntry>>,
}

impl DeviceCodeStore {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

/// 设备码流程
pub struct DeviceCodeFlow {
    config: DeviceCodeConfig,
    store: DeviceCodeStore,
}

impl DeviceCodeFlow {
    /// 创建设备码流程
    pub fn new(config: DeviceCodeConfig) -> Self {
        Self {
            config,
            store: DeviceCodeStore::new(),
        }
    }

    /// 颁发设备码（spec 5.13.1 规则 5）
    pub fn issue(&self, client_id: &str, scope: &[String]) -> DeviceCodeResponse {
        let device_code = generate_random_string(40);
        let user_code = generate_user_code();

        let entry = DeviceCodeEntry {
            user_code: user_code.clone(),
            client_id: client_id.to_string(),
            scope: scope.to_vec(),
            created_at: Instant::now(),
            last_poll: Mutex::new(None),
            status: Mutex::new(DeviceCodeStatus::Pending),
        };

        self.store.entries.lock().insert(device_code.clone(), entry);

        DeviceCodeResponse {
            device_code,
            user_code,
            verification_uri: self.config.verification_uri.clone(),
            poll_interval: self.config.poll_interval.as_secs(),
            expires_in: self.config.expiry.as_secs(),
        }
    }

    /// 轮询授权状态（spec 5.13.1 规则 6/7/8）
    pub fn poll(&self, device_code: &str) -> Result<PollResult, DeviceCodeError> {
        let entries = self.store.entries.lock();
        let entry = entries
            .get(device_code)
            .ok_or(DeviceCodeError::ExpiredToken)?;

        // 过期检查
        if entry.created_at.elapsed() >= self.config.expiry {
            return Ok(PollResult::ExpiredToken);
        }

        let mut status = entry.status.lock();
        match &*status {
            DeviceCodeStatus::Authorized {
                access_token,
                refresh_token,
            } => {
                let result = PollResult::Token {
                    access_token: access_token.clone(),
                    refresh_token: refresh_token.clone(),
                };
                *status = DeviceCodeStatus::Consumed;
                Ok(result)
            }
            DeviceCodeStatus::Consumed => Err(DeviceCodeError::Consumed),
            DeviceCodeStatus::Pending => {
                drop(status);
                let mut last_poll = entry.last_poll.lock();
                if let Some(last) = *last_poll {
                    if last.elapsed() < self.config.poll_interval {
                        return Ok(PollResult::SlowDown);
                    }
                }
                *last_poll = Some(Instant::now());
                Ok(PollResult::AuthorizationPending)
            }
        }
    }

    /// 用户授权完成（外部调用，模拟用户在浏览器中授权）
    pub fn authorize(
        &self,
        device_code: &str,
        access_token: &str,
        refresh_token: Option<&str>,
    ) -> Result<(), DeviceCodeError> {
        let entries = self.store.entries.lock();
        let entry = entries
            .get(device_code)
            .ok_or(DeviceCodeError::ExpiredToken)?;

        if entry.created_at.elapsed() >= self.config.expiry {
            return Err(DeviceCodeError::ExpiredToken);
        }

        let mut status = entry.status.lock();
        if *status != DeviceCodeStatus::Pending {
            return Err(DeviceCodeError::Consumed);
        }

        *status = DeviceCodeStatus::Authorized {
            access_token: access_token.to_string(),
            refresh_token: refresh_token.map(|s| s.to_string()),
        };
        Ok(())
    }

    /// 获取用户码（供验证页面展示）
    pub fn user_code(&self, device_code: &str) -> Option<String> {
        let entries = self.store.entries.lock();
        entries.get(device_code).map(|e| e.user_code.clone())
    }
}

/// 生成随机字符串
fn generate_random_string(len: usize) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut bytes = vec![0u8; len];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| CHARSET[*b as usize % CHARSET.len()] as char)
        .collect()
}

/// 生成用户码（8 字符，大写字母+数字，中间有短横线便于阅读）
fn generate_user_code() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    let chars: String = bytes
        .iter()
        .map(|b| CHARSET[*b as usize % CHARSET.len()] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_issue_device_code() {
        let flow = DeviceCodeFlow::new(DeviceCodeConfig::default());
        let resp = flow.issue("client1", &["read".to_string(), "write".to_string()]);

        assert!(!resp.device_code.is_empty(), "device_code 不应为空");
        assert!(!resp.user_code.is_empty(), "user_code 不应为空");
        assert!(resp.user_code.contains('-'), "user_code 应含短横线");
        assert_eq!(resp.poll_interval, 5);
        assert_eq!(resp.expires_in, 900);
    }

    #[test]
    fn test_poll_pending() {
        let flow = DeviceCodeFlow::new(DeviceCodeConfig::default());
        let resp = flow.issue("client1", &[]);

        let result = flow.poll(&resp.device_code).unwrap();
        assert_eq!(result, PollResult::AuthorizationPending);
    }

    #[test]
    fn test_poll_authorized() {
        let flow = DeviceCodeFlow::new(DeviceCodeConfig::default());
        let resp = flow.issue("client1", &[]);

        flow.authorize(&resp.device_code, "access123", Some("refresh456"))
            .unwrap();

        let result = flow.poll(&resp.device_code).unwrap();
        assert_eq!(
            result,
            PollResult::Token {
                access_token: "access123".to_string(),
                refresh_token: Some("refresh456".to_string()),
            }
        );
    }

    #[test]
    fn test_poll_consumed_after_authorized() {
        let flow = DeviceCodeFlow::new(DeviceCodeConfig::default());
        let resp = flow.issue("client1", &[]);

        flow.authorize(&resp.device_code, "access123", None)
            .unwrap();

        let _ = flow.poll(&resp.device_code).unwrap();
        let result = flow.poll(&resp.device_code);
        assert_eq!(result, Err(DeviceCodeError::Consumed));
    }

    #[test]
    fn test_poll_expired() {
        let config = DeviceCodeConfig {
            expiry: Duration::from_millis(1),
            poll_interval: Duration::from_millis(0),
            verification_uri: "https://test.com".to_string(),
        };
        let flow = DeviceCodeFlow::new(config);
        let resp = flow.issue("client1", &[]);

        std::thread::sleep(Duration::from_millis(10));

        let result = flow.poll(&resp.device_code).unwrap();
        assert_eq!(result, PollResult::ExpiredToken);
    }

    #[test]
    fn test_poll_slow_down() {
        let config = DeviceCodeConfig {
            expiry: Duration::from_secs(900),
            poll_interval: Duration::from_secs(10),
            verification_uri: "https://test.com".to_string(),
        };
        let flow = DeviceCodeFlow::new(config);
        let resp = flow.issue("client1", &[]);

        let r1 = flow.poll(&resp.device_code).unwrap();
        assert_eq!(r1, PollResult::AuthorizationPending);

        let r2 = flow.poll(&resp.device_code).unwrap();
        assert_eq!(r2, PollResult::SlowDown);
    }

    #[test]
    fn test_poll_nonexistent_device_code() {
        let flow = DeviceCodeFlow::new(DeviceCodeConfig::default());
        let result = flow.poll("nonexistent");
        assert_eq!(result, Err(DeviceCodeError::ExpiredToken));
    }

    #[test]
    fn test_authorize_expired() {
        let config = DeviceCodeConfig {
            expiry: Duration::from_millis(1),
            poll_interval: Duration::from_secs(5),
            verification_uri: "https://test.com".to_string(),
        };
        let flow = DeviceCodeFlow::new(config);
        let resp = flow.issue("client1", &[]);

        std::thread::sleep(Duration::from_millis(10));

        let result = flow.authorize(&resp.device_code, "token", None);
        assert_eq!(result, Err(DeviceCodeError::ExpiredToken));
    }

    #[test]
    fn test_user_code_retrieval() {
        let flow = DeviceCodeFlow::new(DeviceCodeConfig::default());
        let resp = flow.issue("client1", &[]);

        let user_code = flow.user_code(&resp.device_code).unwrap();
        assert_eq!(user_code, resp.user_code);
    }
}
