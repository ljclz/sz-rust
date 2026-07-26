//! 密码策略（Password Profile）— Phase 6.33
//!
//! 提供 PG 风格的密码策略（Password Profile）功能：
//!
//! - **密码复杂度校验**：长度/大小写/数字/特殊字符/重复字符
//! - **密码有效期**：`lifetime_days` 到期强制修改
//! - **密码历史**：`history_count` + `reuse_interval_days` 防止重用旧密码
//! - **账户锁定**：连续失败 `max_failures` 次后锁定 `lockout_duration_secs` 秒
//! - **密码哈希**：salted SHA-256（PBKDF2 风格多轮迭代）
//! - **Profile 注册表**：命名 Profile 管理（`CREATE PASSWORD PROFILE` 对应 `register`）
//!
//! # 设计
//!
//! - **PasswordProfile** — 策略配置（长度/复杂度/有效期/历史/锁定）
//! - **PasswordProfileRegistry** — 命名 Profile 注册表，预置 `default` Profile
//! - **PasswordHash** — 密码哈希（`sha256$<iterations>$<salt_hex>$<hash_hex>` 格式）
//! - **PasswordHistoryEntry** — 历史记录（hash + changed_at 时间戳）
//! - **UserPasswordState** — 用户密码状态（当前哈希 + 历史 + 失败计数 + 锁定状态）
//!
//! # 与 PG 的关系
//!
//! - PG 通过 `CREATE PROFILE` + `ALTER ROLE ... PASSWORD ...` 实现密码策略
//! - PG 扩展 `passwordcheck` 提供基本复杂度校验
//! - PG `pg_authid` 表存储密码哈希（SCRAM-SHA-256）
//! - 本实现提供程序化 API，未集成到 SQL 解析路径
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **无持久化**：纯内存状态，不落盘
//! - **哈希算法**：salted SHA-256 多轮迭代（非 SCRAM-SHA-256/Argon2/bcrypt）
//! - **时间依赖**：调用方提供 `now`（Unix epoch 秒），便于测试
//! - **单进程**：无分布式锁定（多节点需外部协调）

use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

// =====================================================================
//  常量
// =====================================================================

/// 默认 Profile 名称
pub const DEFAULT_PROFILE_NAME: &str = "default";

/// 默认最小密码长度
pub const DEFAULT_MIN_LENGTH: usize = 8;

/// 默认最大密码长度
pub const DEFAULT_MAX_LENGTH: usize = 128;

/// 默认密码有效期（天），0 = 永不过期
pub const DEFAULT_LIFETIME_DAYS: u32 = 90;

/// 默认密码历史记录数
pub const DEFAULT_HISTORY_COUNT: usize = 5;

/// 默认密码重用间隔（天），0 = 永不可重用（在 history_count 范围内）
pub const DEFAULT_REUSE_INTERVAL_DAYS: u32 = 180;

/// 默认最大连续失败次数
pub const DEFAULT_MAX_FAILURES: u32 = 5;

/// 默认锁定时长（秒）= 30 分钟
pub const DEFAULT_LOCKOUT_DURATION_SECS: u64 = 1800;

/// 哈希迭代轮数
const HASH_ITERATIONS: u32 = 100_000;

/// 盐长度（字节）
const SALT_LEN: usize = 16;

/// 一天的秒数
const SECS_PER_DAY: u64 = 86_400;

/// 特殊字符集合
const SPECIAL_CHARS: &str = "!@#$%^&*()_+-=[]{}|;':\",./<>?`~\\ ";

// =====================================================================
//  错误类型
// =====================================================================

/// 密码策略错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    /// 密码过短
    #[error("password too short: {actual} < min {min}")]
    TooShort { actual: usize, min: usize },
    /// 密码过长
    #[error("password too long: {actual} > max {max}")]
    TooLong { actual: usize, max: usize },
    /// 缺少大写字母
    #[error("password must contain at least {min} uppercase letter(s), got {actual}")]
    NotEnoughUppercase { actual: usize, min: usize },
    /// 缺少小写字母
    #[error("password must contain at least {min} lowercase letter(s), got {actual}")]
    NotEnoughLowercase { actual: usize, min: usize },
    /// 缺少数字
    #[error("password must contain at least {min} digit(s), got {actual}")]
    NotEnoughDigit { actual: usize, min: usize },
    /// 缺少特殊字符
    #[error("password must contain at least {min} special character(s), got {actual}")]
    NotEnoughSpecial { actual: usize, min: usize },
    /// 连续重复字符过多
    #[error("password has too many consecutive repeating chars: {found} > max {max}")]
    TooManyRepeating { found: usize, max: usize },
    /// 密码在历史记录中（不可重用）
    #[error("password has been used recently (within history_count={history_count} or reuse_interval_days={reuse_interval_days})")]
    PasswordInHistory {
        history_count: usize,
        reuse_interval_days: u32,
    },
    /// 密码已过期
    #[error(
        "password expired: last changed {last_changed_secs} ago, lifetime_days={lifetime_days}"
    )]
    PasswordExpired {
        last_changed_secs: u64,
        lifetime_days: u32,
    },
    /// 账户已锁定
    #[error("account locked: unlock at {unlock_at} (now={now}), remaining {remaining_secs}s")]
    AccountLocked {
        unlock_at: u64,
        now: u64,
        remaining_secs: u64,
    },
    /// 连续失败次数过多
    #[error("too many failed attempts: {failures} >= max {max}")]
    TooManyFailures { failures: u32, max: u32 },
    /// 密码不正确（验证失败）
    #[error("incorrect password")]
    IncorrectPassword,
    /// 旧密码不正确
    #[error("old password incorrect")]
    OldPasswordIncorrect,
    /// Profile 名称已存在
    #[error("password profile already exists: {0}")]
    ProfileAlreadyExists(String),
    /// Profile 名称不存在
    #[error("password profile not found: {0}")]
    ProfileNotFound(String),
    /// Profile 名称无效（空或保留字）
    #[error("invalid profile name: {0}")]
    InvalidProfileName(String),
    /// 不能删除默认 Profile
    #[error("cannot remove default profile")]
    CannotRemoveDefault,
    /// 哈希格式错误
    #[error("invalid hash format: {0}")]
    InvalidHashFormat(String),
    /// 新旧密码相同
    #[error("new password must differ from old password")]
    NewPasswordSameAsOld,
}

// =====================================================================
//  PasswordProfile — 密码策略配置
// =====================================================================

/// 密码策略配置
///
/// 定义密码复杂度、有效期、历史、锁定规则。
///
/// # 用法
///
/// ```ignore
/// use szrsql_security::password_profile::*;
///
/// // 使用默认 Profile
/// let profile = PasswordProfile::default();
/// profile.validate("Abc123!@#").unwrap();
///
/// // 自定义 Profile
/// let strict = PasswordProfile::builder("strict")
///     .min_length(16)
///     .max_failures(3)
///     .lockout_duration_secs(3600)
///     .build();
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordProfile {
    /// Profile 名称
    pub name: String,
    /// 最小密码长度
    pub min_length: usize,
    /// 最大密码长度
    pub max_length: usize,
    /// 最少大写字母数
    pub min_uppercase: usize,
    /// 最少小写字母数
    pub min_lowercase: usize,
    /// 最少数字数
    pub min_digit: usize,
    /// 最少特殊字符数
    pub min_special: usize,
    /// 最大连续重复字符数（0 = 不限制）
    pub max_repeating: usize,
    /// 密码有效期（天），0 = 永不过期
    pub lifetime_days: u32,
    /// 密码历史记录数（防止重用最近 N 个密码），0 = 不记录历史
    pub history_count: usize,
    /// 密码重用间隔（天），在间隔内不可重用历史密码，0 = 受 history_count 限制
    pub reuse_interval_days: u32,
    /// 最大连续失败次数
    pub max_failures: u32,
    /// 锁定时长（秒）
    pub lockout_duration_secs: u64,
}

impl PasswordProfile {
    /// 创建默认配置的 Profile（使用 DEFAULT_* 常量）
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            min_length: DEFAULT_MIN_LENGTH,
            max_length: DEFAULT_MAX_LENGTH,
            min_uppercase: 1,
            min_lowercase: 1,
            min_digit: 1,
            min_special: 1,
            max_repeating: 0,
            lifetime_days: DEFAULT_LIFETIME_DAYS,
            history_count: DEFAULT_HISTORY_COUNT,
            reuse_interval_days: DEFAULT_REUSE_INTERVAL_DAYS,
            max_failures: DEFAULT_MAX_FAILURES,
            lockout_duration_secs: DEFAULT_LOCKOUT_DURATION_SECS,
        }
    }

    /// 创建 builder
    pub fn builder(name: impl Into<String>) -> PasswordProfileBuilder {
        PasswordProfileBuilder::new(name)
    }

    /// 校验密码复杂度（不检查历史/有效期/锁定）
    ///
    /// 返回 `Ok(())` 表示密码符合复杂度规则。
    pub fn validate(&self, password: &str) -> Result<(), PasswordError> {
        let len = password.chars().count();

        // 长度校验
        if len < self.min_length {
            return Err(PasswordError::TooShort {
                actual: len,
                min: self.min_length,
            });
        }
        if len > self.max_length {
            return Err(PasswordError::TooLong {
                actual: len,
                max: self.max_length,
            });
        }

        // 字符类别计数
        let mut uppercase = 0usize;
        let mut lowercase = 0usize;
        let mut digit = 0usize;
        let mut special = 0usize;

        for c in password.chars() {
            if c.is_uppercase() {
                uppercase += 1;
            } else if c.is_lowercase() {
                lowercase += 1;
            } else if c.is_ascii_digit() {
                digit += 1;
            } else if SPECIAL_CHARS.contains(c) {
                special += 1;
            } else {
                // 非 ASCII 字母（如中文/Unicode）视为特殊字符
                special += 1;
            }
        }

        if uppercase < self.min_uppercase {
            return Err(PasswordError::NotEnoughUppercase {
                actual: uppercase,
                min: self.min_uppercase,
            });
        }
        if lowercase < self.min_lowercase {
            return Err(PasswordError::NotEnoughLowercase {
                actual: lowercase,
                min: self.min_lowercase,
            });
        }
        if digit < self.min_digit {
            return Err(PasswordError::NotEnoughDigit {
                actual: digit,
                min: self.min_digit,
            });
        }
        if special < self.min_special {
            return Err(PasswordError::NotEnoughSpecial {
                actual: special,
                min: self.min_special,
            });
        }

        // 连续重复字符校验
        if self.max_repeating > 0 {
            let max_found = max_consecutive_repeating(password);
            if max_found > self.max_repeating {
                return Err(PasswordError::TooManyRepeating {
                    found: max_found,
                    max: self.max_repeating,
                });
            }
        }

        Ok(())
    }

    /// 检查密码是否已过期
    ///
    /// - `last_changed_secs` — 上次修改时间（Unix epoch 秒）
    /// - `now_secs` — 当前时间（Unix epoch 秒）
    pub fn is_expired(&self, last_changed_secs: u64, now_secs: u64) -> bool {
        if self.lifetime_days == 0 {
            return false;
        }
        let elapsed = now_secs.saturating_sub(last_changed_secs);
        elapsed >= u64::from(self.lifetime_days) * SECS_PER_DAY
    }

    /// 计算过期剩余秒数（0 表示已过期；None 表示永不过期）
    pub fn expires_in_secs(&self, last_changed_secs: u64, now_secs: u64) -> Option<u64> {
        if self.lifetime_days == 0 {
            return None;
        }
        let lifetime_secs = u64::from(self.lifetime_days) * SECS_PER_DAY;
        let elapsed = now_secs.saturating_sub(last_changed_secs);
        if elapsed >= lifetime_secs {
            Some(0)
        } else {
            Some(lifetime_secs - elapsed)
        }
    }
}

impl Default for PasswordProfile {
    fn default() -> Self {
        Self::new(DEFAULT_PROFILE_NAME)
    }
}

// =====================================================================
//  PasswordProfileBuilder
// =====================================================================

/// PasswordProfile 构建器
#[derive(Debug, Clone)]
pub struct PasswordProfileBuilder {
    profile: PasswordProfile,
}

impl PasswordProfileBuilder {
    /// 创建 builder
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            profile: PasswordProfile::new(name),
        }
    }

    /// 设置最小长度
    pub fn min_length(mut self, n: usize) -> Self {
        self.profile.min_length = n;
        self
    }

    /// 设置最大长度
    pub fn max_length(mut self, n: usize) -> Self {
        self.profile.max_length = n;
        self
    }

    /// 设置最少大写字母数
    pub fn min_uppercase(mut self, n: usize) -> Self {
        self.profile.min_uppercase = n;
        self
    }

    /// 设置最少小写字母数
    pub fn min_lowercase(mut self, n: usize) -> Self {
        self.profile.min_lowercase = n;
        self
    }

    /// 设置最少数字数
    pub fn min_digit(mut self, n: usize) -> Self {
        self.profile.min_digit = n;
        self
    }

    /// 设置最少特殊字符数
    pub fn min_special(mut self, n: usize) -> Self {
        self.profile.min_special = n;
        self
    }

    /// 设置最大连续重复字符数
    pub fn max_repeating(mut self, n: usize) -> Self {
        self.profile.max_repeating = n;
        self
    }

    /// 设置密码有效期（天），0 = 永不过期
    pub fn lifetime_days(mut self, days: u32) -> Self {
        self.profile.lifetime_days = days;
        self
    }

    /// 设置密码历史记录数
    pub fn history_count(mut self, n: usize) -> Self {
        self.profile.history_count = n;
        self
    }

    /// 设置密码重用间隔（天）
    pub fn reuse_interval_days(mut self, days: u32) -> Self {
        self.profile.reuse_interval_days = days;
        self
    }

    /// 设置最大连续失败次数
    pub fn max_failures(mut self, n: u32) -> Self {
        self.profile.max_failures = n;
        self
    }

    /// 设置锁定时长（秒）
    pub fn lockout_duration_secs(mut self, secs: u64) -> Self {
        self.profile.lockout_duration_secs = secs;
        self
    }

    /// 构建 Profile
    pub fn build(self) -> PasswordProfile {
        self.profile
    }
}

// =====================================================================
//  PasswordProfileRegistry — Profile 注册表
// =====================================================================

/// 密码 Profile 注册表
///
/// 管理命名 Profile，预置 `default` Profile。
/// 对应 PG `CREATE PROFILE` / `ALTER PROFILE` / `DROP PROFILE`。
///
/// # 用法
///
/// ```ignore
/// use szrsql_security::password_profile::*;
///
/// let mut registry = PasswordProfileRegistry::new();
/// assert!(registry.get("default").is_some());
///
/// let strict = PasswordProfile::builder("strict")
///     .min_length(16)
///     .max_failures(3)
///     .build();
/// registry.register(strict).unwrap();
/// assert!(registry.get("strict").is_some());
/// ```
#[derive(Debug, Clone)]
pub struct PasswordProfileRegistry {
    profiles: HashMap<String, PasswordProfile>,
}

impl PasswordProfileRegistry {
    /// 创建注册表，预置 `default` Profile
    pub fn new() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), PasswordProfile::default());
        Self { profiles }
    }

    /// 注册新 Profile
    ///
    /// - 名称不能为空
    /// - 名称不能与已有 Profile 重复
    pub fn register(&mut self, profile: PasswordProfile) -> Result<(), PasswordError> {
        if profile.name.is_empty() {
            return Err(PasswordError::InvalidProfileName(profile.name));
        }
        if self.profiles.contains_key(&profile.name) {
            return Err(PasswordError::ProfileAlreadyExists(profile.name));
        }
        self.profiles.insert(profile.name.clone(), profile);
        Ok(())
    }

    /// 替换已有 Profile（若不存在则创建）
    pub fn upsert(&mut self, profile: PasswordProfile) -> Result<(), PasswordError> {
        if profile.name.is_empty() {
            return Err(PasswordError::InvalidProfileName(profile.name));
        }
        self.profiles.insert(profile.name.clone(), profile);
        Ok(())
    }

    /// 按 name 查找 Profile
    pub fn get(&self, name: &str) -> Option<&PasswordProfile> {
        self.profiles.get(name)
    }

    /// 按 name 查找 Profile（可变）
    pub fn get_mut(&mut self, name: &str) -> Option<&mut PasswordProfile> {
        self.profiles.get_mut(name)
    }

    /// 删除 Profile（不能删除 `default`）
    pub fn remove(&mut self, name: &str) -> Result<PasswordProfile, PasswordError> {
        if name == DEFAULT_PROFILE_NAME {
            return Err(PasswordError::CannotRemoveDefault);
        }
        self.profiles
            .remove(name)
            .ok_or_else(|| PasswordError::ProfileNotFound(name.to_string()))
    }

    /// 列出所有 Profile 名称（排序）
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self.profiles.keys().cloned().collect();
        names.sort();
        names
    }

    /// Profile 数量
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

impl Default for PasswordProfileRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  PasswordHash — 密码哈希（salted SHA-256 多轮迭代）
// =====================================================================

/// 密码哈希（salted SHA-256 多轮迭代）
///
/// 存储格式：`sha256$<iterations>$<salt_hex>$<hash_hex>`
///
/// # 安全性
///
/// - **salt**：16 字节随机盐，防止彩虹表攻击
/// - **iterations**：100,000 轮 SHA-256 迭代，增加暴力破解成本
/// - **算法**：SHA-256（非 SCRAM-SHA-256/Argon2/bcrypt，但足够防止明文存储）
///
/// # 用法
///
/// ```ignore
/// use szrsql_security::password_profile::PasswordHash;
///
/// let hash = PasswordHash::hash("my_password");
/// assert!(hash.verify("my_password"));
/// assert!(!hash.verify("wrong_password"));
///
/// let stored = hash.to_string();
/// let parsed: PasswordHash = stored.parse().unwrap();
/// assert_eq!(hash, parsed);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordHash {
    /// 迭代轮数
    pub iterations: u32,
    /// 盐（十六进制字符串）
    pub salt_hex: String,
    /// 哈希值（十六进制字符串）
    pub hash_hex: String,
}

impl PasswordHash {
    /// 对密码进行哈希
    pub fn hash(password: &str) -> Self {
        let mut salt = [0u8; SALT_LEN];
        rand::rng().fill_bytes(&mut salt);
        let iterations = HASH_ITERATIONS;
        let hash_hex = compute_hash(password, &salt, iterations);
        Self {
            iterations,
            salt_hex: bytes_to_hex(&salt),
            hash_hex,
        }
    }

    /// 使用指定盐和迭代轮数哈希（用于测试/确定性场景）
    pub fn hash_with_salt(password: &str, salt: &[u8], iterations: u32) -> Self {
        let hash_hex = compute_hash(password, salt, iterations);
        Self {
            iterations,
            salt_hex: bytes_to_hex(salt),
            hash_hex,
        }
    }

    /// 验证密码是否匹配哈希
    pub fn verify(&self, password: &str) -> bool {
        let salt = match hex_to_bytes(&self.salt_hex) {
            Some(s) => s,
            None => return false,
        };
        let computed = compute_hash(password, &salt, self.iterations);
        // 常量时间比较（防时序攻击）
        constant_time_eq(computed.as_bytes(), self.hash_hex.as_bytes())
    }
}

impl std::fmt::Display for PasswordHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "sha256${}${}${}",
            self.iterations, self.salt_hex, self.hash_hex
        )
    }
}

impl std::str::FromStr for PasswordHash {
    type Err = PasswordError;

    fn from_str(s: &str) -> Result<Self, PasswordError> {
        let parts: Vec<&str> = s.split('$').collect();
        if parts.len() != 4 || parts[0] != "sha256" {
            return Err(PasswordError::InvalidHashFormat(s.to_string()));
        }
        let iterations: u32 = parts[1]
            .parse()
            .map_err(|_| PasswordError::InvalidHashFormat(s.to_string()))?;
        // 校验 salt 和 hash 为合法十六进制
        if hex_to_bytes(parts[2]).is_none() {
            return Err(PasswordError::InvalidHashFormat(s.to_string()));
        }
        if hex_to_bytes(parts[3]).is_none() {
            return Err(PasswordError::InvalidHashFormat(s.to_string()));
        }
        Ok(Self {
            iterations,
            salt_hex: parts[2].to_string(),
            hash_hex: parts[3].to_string(),
        })
    }
}

// =====================================================================
//  PasswordHistoryEntry — 密码历史记录
// =====================================================================

/// 密码历史记录条目
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordHistoryEntry {
    /// 密码哈希
    pub hash: PasswordHash,
    /// 修改时间（Unix epoch 秒）
    pub changed_at: u64,
}

impl PasswordHistoryEntry {
    /// 构造历史记录
    pub fn new(hash: PasswordHash, changed_at: u64) -> Self {
        Self { hash, changed_at }
    }
}

// =====================================================================
//  UserPasswordState — 用户密码状态
// =====================================================================

/// 用户密码状态
///
/// 跟踪用户的当前密码、历史密码、失败计数和锁定状态。
///
/// # 用法
///
/// ```ignore
/// use szrsql_security::password_profile::*;
///
/// let profile = PasswordProfile::default();
/// let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
///
/// // 验证密码
/// assert!(state.verify("Abc123!@#", &profile, 1000).is_ok());
/// assert!(state.verify("wrong", &profile, 1000).is_err());
///
/// // 失败 5 次后锁定
/// for _ in 0..5 {
///     let _ = state.verify("wrong", &profile, 1000);
/// }
/// assert!(state.is_locked(1000));
///
/// // 修改密码（历史校验）
/// state.change_password("Abc123!@#", "Xyz789#$", &profile, 2000).unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct UserPasswordState {
    /// 当前密码哈希
    pub current_hash: PasswordHash,
    /// 密码历史（最近 N 个，按时间降序）
    pub history: Vec<PasswordHistoryEntry>,
    /// 连续失败次数
    pub failed_attempts: u32,
    /// 锁定截止时间（Unix epoch 秒），None 表示未锁定
    pub locked_until: Option<u64>,
    /// 上次修改时间（Unix epoch 秒）
    pub last_changed_at: u64,
}

impl UserPasswordState {
    /// 创建新用户密码状态（初始密码）
    ///
    /// - `password` — 初始密码
    /// - `profile` — 密码策略
    /// - `now_secs` — 当前时间（Unix epoch 秒）
    pub fn new(
        password: &str,
        profile: &PasswordProfile,
        now_secs: u64,
    ) -> Result<Self, PasswordError> {
        profile.validate(password)?;
        let hash = PasswordHash::hash(password);
        Ok(Self {
            current_hash: hash,
            history: Vec::new(),
            failed_attempts: 0,
            locked_until: None,
            last_changed_at: now_secs,
        })
    }

    /// 验证密码
    ///
    /// - 密码正确 → 重置失败计数，返回 Ok
    /// - 密码错误 → 增加失败计数，达到阈值则锁定，返回 Err
    /// - 账户已锁定 → 返回 Err（即使密码正确也不验证）
    pub fn verify(
        &mut self,
        password: &str,
        profile: &PasswordProfile,
        now_secs: u64,
    ) -> Result<(), PasswordError> {
        // 检查锁定状态
        if let Some(unlock_at) = self.locked_until {
            if now_secs < unlock_at {
                return Err(PasswordError::AccountLocked {
                    unlock_at,
                    now: now_secs,
                    remaining_secs: unlock_at - now_secs,
                });
            } else {
                // 锁定期满，自动解锁并重置失败计数
                self.locked_until = None;
                self.failed_attempts = 0;
            }
        }

        if self.current_hash.verify(password) {
            // 验证成功，重置失败计数
            self.failed_attempts = 0;
            Ok(())
        } else {
            // 验证失败
            self.failed_attempts += 1;
            if self.failed_attempts >= profile.max_failures {
                self.locked_until = Some(now_secs + profile.lockout_duration_secs);
                return Err(PasswordError::TooManyFailures {
                    failures: self.failed_attempts,
                    max: profile.max_failures,
                });
            }
            Err(PasswordError::IncorrectPassword)
        }
    }

    /// 修改密码
    ///
    /// - 验证旧密码
    /// - 校验新密码复杂度
    /// - 检查新密码不与旧密码相同
    /// - 检查新密码不在历史记录中
    /// - 更新当前密码 + 历史记录
    pub fn change_password(
        &mut self,
        old_password: &str,
        new_password: &str,
        profile: &PasswordProfile,
        now_secs: u64,
    ) -> Result<(), PasswordError> {
        // 检查锁定状态
        if let Some(unlock_at) = self.locked_until {
            if now_secs < unlock_at {
                return Err(PasswordError::AccountLocked {
                    unlock_at,
                    now: now_secs,
                    remaining_secs: unlock_at - now_secs,
                });
            } else {
                self.locked_until = None;
                self.failed_attempts = 0;
            }
        }

        // 验证旧密码
        if !self.current_hash.verify(old_password) {
            self.failed_attempts += 1;
            if self.failed_attempts >= profile.max_failures {
                self.locked_until = Some(now_secs + profile.lockout_duration_secs);
            }
            return Err(PasswordError::OldPasswordIncorrect);
        }

        // 新旧密码不能相同
        if old_password == new_password {
            return Err(PasswordError::NewPasswordSameAsOld);
        }

        // 校验新密码复杂度
        profile.validate(new_password)?;

        // 检查历史记录
        if profile.history_count > 0 {
            let new_hash = PasswordHash::hash(new_password);
            // 检查当前密码
            if new_hash == self.current_hash {
                return Err(PasswordError::PasswordInHistory {
                    history_count: profile.history_count,
                    reuse_interval_days: profile.reuse_interval_days,
                });
            }
            // 检查历史记录
            let interval_secs = u64::from(profile.reuse_interval_days) * SECS_PER_DAY;
            for entry in &self.history {
                // 在 reuse_interval 内的密码不可重用
                let in_interval = if profile.reuse_interval_days == 0 {
                    true
                } else {
                    now_secs.saturating_sub(entry.changed_at) < interval_secs
                };
                if in_interval && entry.hash.verify(new_password) {
                    return Err(PasswordError::PasswordInHistory {
                        history_count: profile.history_count,
                        reuse_interval_days: profile.reuse_interval_days,
                    });
                }
            }
        }

        // 将旧密码加入历史
        let old_entry = PasswordHistoryEntry::new(self.current_hash.clone(), self.last_changed_at);
        self.history.insert(0, old_entry);
        // 裁剪历史到 history_count
        if self.history.len() > profile.history_count {
            self.history.truncate(profile.history_count);
        }

        // 更新当前密码
        self.current_hash = PasswordHash::hash(new_password);
        self.last_changed_at = now_secs;
        self.failed_attempts = 0;
        self.locked_until = None;

        Ok(())
    }

    /// 管理员重置密码（不需要旧密码，但记录历史）
    ///
    /// - 校验新密码复杂度
    /// - 检查历史记录
    /// - 重置失败计数和锁定状态
    pub fn admin_reset(
        &mut self,
        new_password: &str,
        profile: &PasswordProfile,
        now_secs: u64,
    ) -> Result<(), PasswordError> {
        // 校验新密码复杂度
        profile.validate(new_password)?;

        // 检查与当前密码相同
        if self.current_hash.verify(new_password) {
            return Err(PasswordError::NewPasswordSameAsOld);
        }

        // 检查历史记录
        if profile.history_count > 0 {
            let interval_secs = u64::from(profile.reuse_interval_days) * SECS_PER_DAY;
            for entry in &self.history {
                let in_interval = if profile.reuse_interval_days == 0 {
                    true
                } else {
                    now_secs.saturating_sub(entry.changed_at) < interval_secs
                };
                if in_interval && entry.hash.verify(new_password) {
                    return Err(PasswordError::PasswordInHistory {
                        history_count: profile.history_count,
                        reuse_interval_days: profile.reuse_interval_days,
                    });
                }
            }
        }

        // 将旧密码加入历史
        let old_entry = PasswordHistoryEntry::new(self.current_hash.clone(), self.last_changed_at);
        self.history.insert(0, old_entry);
        if self.history.len() > profile.history_count {
            self.history.truncate(profile.history_count);
        }

        // 更新当前密码
        self.current_hash = PasswordHash::hash(new_password);
        self.last_changed_at = now_secs;
        self.failed_attempts = 0;
        self.locked_until = None;

        Ok(())
    }

    /// 是否已锁定
    pub fn is_locked(&self, now_secs: u64) -> bool {
        self.locked_until
            .is_some_and(|unlock_at| now_secs < unlock_at)
    }

    /// 是否已过期
    pub fn is_expired(&self, profile: &PasswordProfile, now_secs: u64) -> bool {
        profile.is_expired(self.last_changed_at, now_secs)
    }

    /// 手动解锁（管理员操作）
    pub fn unlock(&mut self) {
        self.locked_until = None;
        self.failed_attempts = 0;
    }

    /// 重置失败计数（管理员操作）
    pub fn reset_failures(&mut self) {
        self.failed_attempts = 0;
    }

    /// 获取剩余锁定秒数（0 表示未锁定或已过期）
    pub fn remaining_lock_secs(&self, now_secs: u64) -> u64 {
        self.locked_until
            .map_or(0, |unlock_at| unlock_at.saturating_sub(now_secs))
    }
}

// =====================================================================
//  内部辅助函数
// =====================================================================

/// 计算密码哈希（salted SHA-256 多轮迭代）
fn compute_hash(password: &str, salt: &[u8], iterations: u32) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(password.as_bytes());
    let mut hash = hasher.finalize();

    for _ in 0..iterations {
        let mut h = Sha256::new();
        h.update(salt);
        h.update(hash);
        hash = h.finalize();
    }

    bytes_to_hex(&hash)
}

/// 字节转十六进制字符串
fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 十六进制字符串转字节
fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16).ok()?;
        bytes.push(byte);
    }
    Some(bytes)
}

/// 常量时间比较（防时序攻击）
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

/// 计算最大连续重复字符数
fn max_consecutive_repeating(s: &str) -> usize {
    let mut max = 0usize;
    let mut current = 0usize;
    let mut prev: Option<char> = None;
    for c in s.chars() {
        if Some(c) == prev {
            current += 1;
        } else {
            current = 1;
        }
        if current > max {
            max = current;
        }
        prev = Some(c);
    }
    max
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  PasswordError 变体测试
    // -----------------------------------------------------------------

    #[test]
    fn test_password_error_to_string() {
        assert_eq!(
            PasswordError::TooShort { actual: 5, min: 8 }.to_string(),
            "password too short: 5 < min 8"
        );
        assert_eq!(
            PasswordError::TooLong {
                actual: 200,
                max: 128
            }
            .to_string(),
            "password too long: 200 > max 128"
        );
        assert_eq!(
            PasswordError::NotEnoughUppercase { actual: 0, min: 1 }.to_string(),
            "password must contain at least 1 uppercase letter(s), got 0"
        );
        assert_eq!(
            PasswordError::AccountLocked {
                unlock_at: 2000,
                now: 1000,
                remaining_secs: 1000
            }
            .to_string(),
            "account locked: unlock at 2000 (now=1000), remaining 1000s"
        );
        assert_eq!(
            PasswordError::IncorrectPassword.to_string(),
            "incorrect password"
        );
        assert_eq!(
            PasswordError::ProfileNotFound("foo".to_string()).to_string(),
            "password profile not found: foo"
        );
    }

    // -----------------------------------------------------------------
    //  PasswordProfile 构造与校验
    // -----------------------------------------------------------------

    #[test]
    fn test_password_profile_default() {
        let p = PasswordProfile::default();
        assert_eq!(p.name, "default");
        assert_eq!(p.min_length, DEFAULT_MIN_LENGTH);
        assert_eq!(p.max_length, DEFAULT_MAX_LENGTH);
        assert_eq!(p.min_uppercase, 1);
        assert_eq!(p.min_lowercase, 1);
        assert_eq!(p.min_digit, 1);
        assert_eq!(p.min_special, 1);
        assert_eq!(p.lifetime_days, DEFAULT_LIFETIME_DAYS);
        assert_eq!(p.history_count, DEFAULT_HISTORY_COUNT);
        assert_eq!(p.max_failures, DEFAULT_MAX_FAILURES);
        assert_eq!(p.lockout_duration_secs, DEFAULT_LOCKOUT_DURATION_SECS);
    }

    #[test]
    fn test_password_profile_new_custom_name() {
        let p = PasswordProfile::new("strict");
        assert_eq!(p.name, "strict");
        assert_eq!(p.min_length, DEFAULT_MIN_LENGTH);
    }

    #[test]
    fn test_password_profile_builder() {
        let p = PasswordProfile::builder("custom")
            .min_length(16)
            .max_length(64)
            .min_uppercase(2)
            .min_lowercase(2)
            .min_digit(2)
            .min_special(2)
            .max_repeating(3)
            .lifetime_days(30)
            .history_count(10)
            .reuse_interval_days(365)
            .max_failures(3)
            .lockout_duration_secs(3600)
            .build();
        assert_eq!(p.name, "custom");
        assert_eq!(p.min_length, 16);
        assert_eq!(p.max_length, 64);
        assert_eq!(p.min_uppercase, 2);
        assert_eq!(p.min_lowercase, 2);
        assert_eq!(p.min_digit, 2);
        assert_eq!(p.min_special, 2);
        assert_eq!(p.max_repeating, 3);
        assert_eq!(p.lifetime_days, 30);
        assert_eq!(p.history_count, 10);
        assert_eq!(p.reuse_interval_days, 365);
        assert_eq!(p.max_failures, 3);
        assert_eq!(p.lockout_duration_secs, 3600);
    }

    #[test]
    fn test_validate_valid_password() {
        let p = PasswordProfile::default();
        assert!(p.validate("Abc123!@#").is_ok());
        assert!(p.validate("XyZ789()abc").is_ok());
    }

    #[test]
    fn test_validate_too_short() {
        let p = PasswordProfile::default();
        let err = p.validate("Ab1!").unwrap_err();
        assert!(matches!(err, PasswordError::TooShort { actual: 4, min: 8 }));
    }

    #[test]
    fn test_validate_too_long() {
        let p = PasswordProfile::builder("test").max_length(5).build();
        let err = p.validate("Abcde1!@#").unwrap_err();
        assert!(matches!(err, PasswordError::TooLong { actual: 9, max: 5 }));
    }

    #[test]
    fn test_validate_missing_uppercase() {
        let p = PasswordProfile::default();
        let err = p.validate("abc123!@#").unwrap_err();
        assert!(matches!(
            err,
            PasswordError::NotEnoughUppercase { actual: 0, min: 1 }
        ));
    }

    #[test]
    fn test_validate_missing_lowercase() {
        let p = PasswordProfile::default();
        let err = p.validate("ABC123!@#").unwrap_err();
        assert!(matches!(
            err,
            PasswordError::NotEnoughLowercase { actual: 0, min: 1 }
        ));
    }

    #[test]
    fn test_validate_missing_digit() {
        let p = PasswordProfile::default();
        let err = p.validate("AbcDef!@#").unwrap_err();
        assert!(matches!(
            err,
            PasswordError::NotEnoughDigit { actual: 0, min: 1 }
        ));
    }

    #[test]
    fn test_validate_missing_special() {
        let p = PasswordProfile::default();
        let err = p.validate("Abc12345").unwrap_err();
        assert!(matches!(
            err,
            PasswordError::NotEnoughSpecial { actual: 0, min: 1 }
        ));
    }

    #[test]
    fn test_validate_unicode_treated_as_special() {
        let p = PasswordProfile::default();
        // 中文视为特殊字符
        assert!(p.validate("Abc123密码").is_ok());
    }

    #[test]
    fn test_validate_max_repeating() {
        let p = PasswordProfile::builder("test").max_repeating(3).build();
        assert!(p.validate("Aaab123!").is_ok()); // 2 个连续 a，未超
        let err = p.validate("Aaaaab123!").unwrap_err(); // 4 个连续 a，超
        assert!(matches!(
            err,
            PasswordError::TooManyRepeating { found: 4, max: 3 }
        ));
    }

    #[test]
    fn test_validate_no_max_repeating() {
        let p = PasswordProfile::default();
        // max_repeating=0 表示不限制
        assert!(p.validate("Aaaaaaaa1!").is_ok());
    }

    #[test]
    fn test_validate_multiple_requirements() {
        let p = PasswordProfile::builder("test")
            .min_uppercase(2)
            .min_lowercase(2)
            .min_digit(2)
            .min_special(2)
            .build();
        assert!(p.validate("ABab12!@").is_ok());
        assert!(p.validate("Aaab12!@").is_err()); // 只 1 个大写
    }

    // -----------------------------------------------------------------
    //  PasswordProfile 过期检查
    // -----------------------------------------------------------------

    #[test]
    fn test_is_expired_never() {
        let p = PasswordProfile::builder("test").lifetime_days(0).build();
        assert!(!p.is_expired(0, 1_000_000_000));
    }

    #[test]
    fn test_is_expired_not_yet() {
        let p = PasswordProfile::builder("test").lifetime_days(90).build();
        let now: u64 = 100_000_000;
        let last = now - 80 * SECS_PER_DAY; // 80 天前
        assert!(!p.is_expired(last, now));
    }

    #[test]
    fn test_is_expired_expired() {
        let p = PasswordProfile::builder("test").lifetime_days(90).build();
        let now = 100_000_000;
        let last = now - 91 * SECS_PER_DAY; // 91 天前
        assert!(p.is_expired(last, now));
    }

    #[test]
    fn test_is_expired_exact_boundary() {
        let p = PasswordProfile::builder("test").lifetime_days(90).build();
        let now = 90 * SECS_PER_DAY;
        let last = 0;
        // 90 天整 → 过期
        assert!(p.is_expired(last, now));
    }

    #[test]
    fn test_expires_in_secs_never() {
        let p = PasswordProfile::builder("test").lifetime_days(0).build();
        assert_eq!(p.expires_in_secs(0, 1000), None);
    }

    #[test]
    fn test_expires_in_secs_remaining() {
        let p = PasswordProfile::builder("test").lifetime_days(10).build();
        let now = 3 * SECS_PER_DAY;
        let last = 0;
        // 剩余 7 天
        assert_eq!(p.expires_in_secs(last, now), Some(7 * SECS_PER_DAY));
    }

    #[test]
    fn test_expires_in_secs_already_expired() {
        let p = PasswordProfile::builder("test").lifetime_days(10).build();
        let now = 20 * SECS_PER_DAY;
        let last = 0;
        assert_eq!(p.expires_in_secs(last, now), Some(0));
    }

    // -----------------------------------------------------------------
    //  PasswordProfileRegistry
    // -----------------------------------------------------------------

    #[test]
    fn test_registry_new_has_default() {
        let reg = PasswordProfileRegistry::new();
        assert!(reg.get("default").is_some());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn test_registry_register_success() {
        let mut reg = PasswordProfileRegistry::new();
        let p = PasswordProfile::builder("strict").min_length(16).build();
        reg.register(p).unwrap();
        assert_eq!(reg.len(), 2);
        assert!(reg.get("strict").is_some());
    }

    #[test]
    fn test_registry_register_duplicate() {
        let mut reg = PasswordProfileRegistry::new();
        let p = PasswordProfile::new("strict");
        reg.register(p).unwrap();
        let p2 = PasswordProfile::new("strict");
        let err = reg.register(p2).unwrap_err();
        assert!(matches!(
            err,
            PasswordError::ProfileAlreadyExists(name) if name == "strict"
        ));
    }

    #[test]
    fn test_registry_register_empty_name() {
        let mut reg = PasswordProfileRegistry::new();
        let p = PasswordProfile::new("");
        let err = reg.register(p).unwrap_err();
        assert!(matches!(err, PasswordError::InvalidProfileName(_)));
    }

    #[test]
    fn test_registry_upsert() {
        let mut reg = PasswordProfileRegistry::new();
        let p = PasswordProfile::builder("test").min_length(10).build();
        reg.upsert(p).unwrap();
        assert_eq!(reg.get("test").unwrap().min_length, 10);

        let p2 = PasswordProfile::builder("test").min_length(20).build();
        reg.upsert(p2).unwrap();
        assert_eq!(reg.get("test").unwrap().min_length, 20);
    }

    #[test]
    fn test_registry_remove_success() {
        let mut reg = PasswordProfileRegistry::new();
        let p = PasswordProfile::new("temp");
        reg.register(p).unwrap();
        reg.remove("temp").unwrap();
        assert!(!reg.get("temp").is_some());
    }

    #[test]
    fn test_registry_remove_default_fails() {
        let mut reg = PasswordProfileRegistry::new();
        let err = reg.remove("default").unwrap_err();
        assert!(matches!(err, PasswordError::CannotRemoveDefault));
    }

    #[test]
    fn test_registry_remove_not_found() {
        let mut reg = PasswordProfileRegistry::new();
        let err = reg.remove("nonexistent").unwrap_err();
        assert!(matches!(
            err,
            PasswordError::ProfileNotFound(name) if name == "nonexistent"
        ));
    }

    #[test]
    fn test_registry_list_sorted() {
        let mut reg = PasswordProfileRegistry::new();
        reg.register(PasswordProfile::new("zeta")).unwrap();
        reg.register(PasswordProfile::new("alpha")).unwrap();
        reg.register(PasswordProfile::new("beta")).unwrap();
        let list = reg.list();
        assert_eq!(list, vec!["alpha", "beta", "default", "zeta"]);
    }

    #[test]
    fn test_registry_get_mut() {
        let mut reg = PasswordProfileRegistry::new();
        reg.register(PasswordProfile::new("temp")).unwrap();
        {
            let p = reg.get_mut("temp").unwrap();
            p.min_length = 20;
        }
        assert_eq!(reg.get("temp").unwrap().min_length, 20);
    }

    #[test]
    fn test_registry_is_empty() {
        let reg = PasswordProfileRegistry::new();
        assert!(!reg.is_empty());
    }

    // -----------------------------------------------------------------
    //  PasswordHash
    // -----------------------------------------------------------------

    #[test]
    fn test_hash_and_verify_success() {
        let hash = PasswordHash::hash("my_secret_password");
        assert!(hash.verify("my_secret_password"));
    }

    #[test]
    fn test_hash_and_verify_failure() {
        let hash = PasswordHash::hash("my_secret_password");
        assert!(!hash.verify("wrong_password"));
    }

    #[test]
    fn test_hash_unique_salt() {
        let h1 = PasswordHash::hash("same_password");
        let h2 = PasswordHash::hash("same_password");
        // 不同的盐 → 不同的哈希
        assert_ne!(h1.salt_hex, h2.salt_hex);
        assert_ne!(h1.hash_hex, h2.hash_hex);
        // 但都能验证同一密码
        assert!(h1.verify("same_password"));
        assert!(h2.verify("same_password"));
    }

    #[test]
    fn test_hash_with_salt_deterministic() {
        let salt = [0u8; 16];
        let h1 = PasswordHash::hash_with_salt("test", &salt, 1000);
        let h2 = PasswordHash::hash_with_salt("test", &salt, 1000);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_with_salt_different_iterations() {
        let salt = [0u8; 16];
        let h1 = PasswordHash::hash_with_salt("test", &salt, 1000);
        let h2 = PasswordHash::hash_with_salt("test", &salt, 2000);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_to_string_and_from_str_roundtrip() {
        let hash = PasswordHash::hash("roundtrip_test");
        let s = hash.to_string();
        let parsed: PasswordHash = s.parse().unwrap();
        assert_eq!(hash, parsed);
    }

    #[test]
    fn test_hash_to_string_format() {
        let salt = [0xAB; 16];
        let hash = PasswordHash::hash_with_salt("test", &salt, 1000);
        let s = hash.to_string();
        assert!(s.starts_with("sha256$1000$"));
        let parts: Vec<&str> = s.split('$').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "sha256");
    }

    #[test]
    fn test_hash_from_str_invalid_format() {
        let err: PasswordError = "invalid".parse::<PasswordHash>().unwrap_err();
        assert!(matches!(err, PasswordError::InvalidHashFormat(_)));

        let err = "md5$1000$salt$hash".parse::<PasswordHash>().unwrap_err();
        assert!(matches!(err, PasswordError::InvalidHashFormat(_)));

        let err = "sha256$abc$salt$hash".parse::<PasswordHash>().unwrap_err();
        assert!(matches!(err, PasswordError::InvalidHashFormat(_)));

        let err = "sha256$1000$nothex$hash"
            .parse::<PasswordHash>()
            .unwrap_err();
        assert!(matches!(err, PasswordError::InvalidHashFormat(_)));
    }

    #[test]
    fn test_hash_display() {
        let hash = PasswordHash::hash_with_salt("test", &[0u8; 16], 100);
        let s = format!("{hash}");
        assert!(s.starts_with("sha256$100$"));
    }

    #[test]
    fn test_hash_verify_empty_password() {
        let hash = PasswordHash::hash("");
        assert!(hash.verify(""));
        assert!(!hash.verify("x"));
    }

    #[test]
    fn test_hash_verify_unicode_password() {
        let hash = PasswordHash::hash("密码密码密码");
        assert!(hash.verify("密码密码密码"));
        assert!(!hash.verify("密码密码"));
    }

    // -----------------------------------------------------------------
    //  UserPasswordState — 基础
    // -----------------------------------------------------------------

    #[test]
    fn test_user_state_new_success() {
        let profile = PasswordProfile::default();
        let state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        assert_eq!(state.failed_attempts, 0);
        assert_eq!(state.locked_until, None);
        assert_eq!(state.last_changed_at, 1000);
        assert!(state.history.is_empty());
    }

    #[test]
    fn test_user_state_new_invalid_password() {
        let profile = PasswordProfile::default();
        let err = UserPasswordState::new("short", &profile, 1000).unwrap_err();
        assert!(matches!(err, PasswordError::TooShort { .. }));
    }

    #[test]
    fn test_user_state_verify_correct() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        assert!(state.verify("Abc123!@#", &profile, 1000).is_ok());
        assert_eq!(state.failed_attempts, 0);
    }

    #[test]
    fn test_user_state_verify_wrong() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        let err = state.verify("wrong", &profile, 1000).unwrap_err();
        assert!(matches!(err, PasswordError::IncorrectPassword));
        assert_eq!(state.failed_attempts, 1);
    }

    #[test]
    fn test_user_state_verify_resets_failures_on_success() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        state.verify("wrong", &profile, 1000).err();
        state.verify("wrong", &profile, 1000).err();
        assert_eq!(state.failed_attempts, 2);
        state.verify("Abc123!@#", &profile, 1000).unwrap();
        assert_eq!(state.failed_attempts, 0);
    }

    // -----------------------------------------------------------------
    //  UserPasswordState — 锁定
    // -----------------------------------------------------------------

    #[test]
    fn test_lockout_after_max_failures() {
        let profile = PasswordProfile::builder("test")
            .max_failures(3)
            .lockout_duration_secs(600)
            .build();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        // 失败 2 次（未锁定）
        for _ in 0..2 {
            let err = state.verify("wrong", &profile, 1000).unwrap_err();
            assert!(matches!(err, PasswordError::IncorrectPassword));
        }
        assert!(!state.is_locked(1000));

        // 第 3 次失败 → 锁定
        let err = state.verify("wrong", &profile, 1000).unwrap_err();
        assert!(matches!(
            err,
            PasswordError::TooManyFailures {
                failures: 3,
                max: 3
            }
        ));
        assert!(state.is_locked(1000));
        assert_eq!(state.locked_until, Some(1600));
    }

    #[test]
    fn test_lockout_verify_when_locked() {
        let profile = PasswordProfile::builder("test")
            .max_failures(2)
            .lockout_duration_secs(600)
            .build();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        // 触发锁定
        for _ in 0..2 {
            state.verify("wrong", &profile, 1000).err();
        }
        assert!(state.is_locked(1000));

        // 即使密码正确，锁定期间也不能验证
        let err = state.verify("Abc123!@#", &profile, 1000).unwrap_err();
        assert!(matches!(
            err,
            PasswordError::AccountLocked {
                unlock_at: 1600,
                now: 1000,
                remaining_secs: 600
            }
        ));
    }

    #[test]
    fn test_lockout_auto_unlock_after_duration() {
        let profile = PasswordProfile::builder("test")
            .max_failures(2)
            .lockout_duration_secs(600)
            .build();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        // 触发锁定
        for _ in 0..2 {
            state.verify("wrong", &profile, 1000).err();
        }
        assert!(state.is_locked(1000));

        // 锁定期满 → 自动解锁
        assert!(state.verify("Abc123!@#", &profile, 1601).is_ok());
        assert!(!state.is_locked(1601));
        assert_eq!(state.failed_attempts, 0);
    }

    #[test]
    fn test_manual_unlock() {
        let profile = PasswordProfile::builder("test")
            .max_failures(2)
            .lockout_duration_secs(600)
            .build();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        for _ in 0..2 {
            state.verify("wrong", &profile, 1000).err();
        }
        assert!(state.is_locked(1000));

        state.unlock();
        assert!(!state.is_locked(1000));
        assert_eq!(state.failed_attempts, 0);
    }

    #[test]
    fn test_remaining_lock_secs() {
        let profile = PasswordProfile::builder("test")
            .max_failures(1)
            .lockout_duration_secs(600)
            .build();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        state.verify("wrong", &profile, 1000).err();
        assert!(state.is_locked(1000));
        assert_eq!(state.remaining_lock_secs(1000), 600);
        assert_eq!(state.remaining_lock_secs(1300), 300);
        assert_eq!(state.remaining_lock_secs(1700), 0);
    }

    #[test]
    fn test_lockout_not_triggered_when_below_threshold() {
        let profile = PasswordProfile::builder("test").max_failures(5).build();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        for _ in 0..4 {
            state.verify("wrong", &profile, 1000).err();
        }
        assert_eq!(state.failed_attempts, 4);
        assert!(!state.is_locked(1000));
    }

    // -----------------------------------------------------------------
    //  UserPasswordState — 修改密码
    // -----------------------------------------------------------------

    #[test]
    fn test_change_password_success() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        state
            .change_password("Abc123!@#", "Xyz789#$", &profile, 2000)
            .unwrap();

        assert!(state.verify("Xyz789#$", &profile, 2000).is_ok());
        assert!(!state.current_hash.verify("Abc123!@#"));
        assert_eq!(state.last_changed_at, 2000);
        assert_eq!(state.history.len(), 1);
    }

    #[test]
    fn test_change_password_wrong_old() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        let err = state
            .change_password("wrong", "Xyz789#$", &profile, 2000)
            .unwrap_err();
        assert!(matches!(err, PasswordError::OldPasswordIncorrect));
    }

    #[test]
    fn test_change_password_same_as_old() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        let err = state
            .change_password("Abc123!@#", "Abc123!@#", &profile, 2000)
            .unwrap_err();
        assert!(matches!(err, PasswordError::NewPasswordSameAsOld));
    }

    #[test]
    fn test_change_password_new_invalid() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        let err = state
            .change_password("Abc123!@#", "short", &profile, 2000)
            .unwrap_err();
        assert!(matches!(err, PasswordError::TooShort { .. }));
    }

    #[test]
    fn test_change_password_history_prevents_reuse() {
        let profile = PasswordProfile::builder("test")
            .history_count(3)
            .reuse_interval_days(365)
            .build();
        let mut state = UserPasswordState::new("Pass1!aa", &profile, 1000).unwrap();

        // Pass1!aa → Pass2!bb
        state
            .change_password("Pass1!aa", "Pass2!bb", &profile, 2000)
            .unwrap();
        // Pass2!bb → Pass3!cc
        state
            .change_password("Pass2!bb", "Pass3!cc", &profile, 3000)
            .unwrap();
        // Pass3!cc → Pass4!dd
        state
            .change_password("Pass3!cc", "Pass4!dd", &profile, 4000)
            .unwrap();

        // 尝试改回 Pass1!aa → 应被拒绝（在 history_count=3 内）
        let err = state
            .change_password("Pass4!dd", "Pass1!aa", &profile, 5000)
            .unwrap_err();
        assert!(matches!(err, PasswordError::PasswordInHistory { .. }));

        // 尝试改回 Pass2!bb → 应被拒绝
        let err = state
            .change_password("Pass4!dd", "Pass2!bb", &profile, 5000)
            .unwrap_err();
        assert!(matches!(err, PasswordError::PasswordInHistory { .. }));
    }

    #[test]
    fn test_change_password_history_no_reuse_when_zero_count() {
        let profile = PasswordProfile::builder("test").history_count(0).build();
        let mut state = UserPasswordState::new("Pass1!aa", &profile, 1000).unwrap();

        // history_count=0 → 不记录历史
        state
            .change_password("Pass1!aa", "Pass2!bb", &profile, 2000)
            .unwrap();
        // 可以改回 Pass1!aa
        state
            .change_password("Pass2!bb", "Pass1!aa", &profile, 3000)
            .unwrap();
    }

    #[test]
    fn test_change_password_history_expired_after_interval() {
        let profile = PasswordProfile::builder("test")
            .history_count(5)
            .reuse_interval_days(10)
            .build();
        let mut state = UserPasswordState::new("Pass1!aa", &profile, 1000).unwrap();

        state
            .change_password("Pass1!aa", "Pass2!bb", &profile, 2000)
            .unwrap();

        // 在 interval 内（10 天 = 864000 秒）→ 拒绝
        let err = state
            .change_password("Pass2!bb", "Pass1!aa", &profile, 2000 + 5 * SECS_PER_DAY)
            .unwrap_err();
        assert!(matches!(err, PasswordError::PasswordInHistory { .. }));

        // 超过 interval → 允许
        state
            .change_password("Pass2!bb", "Pass1!aa", &profile, 2000 + 11 * SECS_PER_DAY)
            .unwrap();
    }

    #[test]
    fn test_change_password_history_truncated() {
        let profile = PasswordProfile::builder("test").history_count(2).build();
        let mut state = UserPasswordState::new("Pass1!aa", &profile, 1000).unwrap();

        state
            .change_password("Pass1!aa", "Pass2!bb", &profile, 2000)
            .unwrap();
        assert_eq!(state.history.len(), 1);

        state
            .change_password("Pass2!bb", "Pass3!cc", &profile, 3000)
            .unwrap();
        assert_eq!(state.history.len(), 2);

        state
            .change_password("Pass3!cc", "Pass4!dd", &profile, 4000)
            .unwrap();
        // history_count=2 → 裁剪到 2
        assert_eq!(state.history.len(), 2);

        // Pass1!aa 已被裁出历史 → 可重用（假设也在 interval 外）
        state
            .change_password("Pass4!dd", "Pass1!aa", &profile, 4000 + 365 * SECS_PER_DAY)
            .unwrap();
    }

    #[test]
    fn test_change_password_resets_failures() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        state.verify("wrong", &profile, 1000).err();
        state.verify("wrong", &profile, 1000).err();
        assert_eq!(state.failed_attempts, 2);

        state
            .change_password("Abc123!@#", "Xyz789#$", &profile, 2000)
            .unwrap();
        assert_eq!(state.failed_attempts, 0);
        assert_eq!(state.locked_until, None);
    }

    #[test]
    fn test_change_password_when_locked() {
        let profile = PasswordProfile::builder("test")
            .max_failures(2)
            .lockout_duration_secs(600)
            .build();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        // 触发锁定
        for _ in 0..2 {
            state.verify("wrong", &profile, 1000).err();
        }
        assert!(state.is_locked(1000));

        // 锁定期间不能修改密码
        let err = state
            .change_password("Abc123!@#", "Xyz789#$", &profile, 1000)
            .unwrap_err();
        assert!(matches!(err, PasswordError::AccountLocked { .. }));
    }

    // -----------------------------------------------------------------
    //  UserPasswordState — 管理员重置
    // -----------------------------------------------------------------

    #[test]
    fn test_admin_reset_success() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        state.admin_reset("Xyz789#$", &profile, 2000).unwrap();

        assert!(state.verify("Xyz789#$", &profile, 2000).is_ok());
        assert_eq!(state.last_changed_at, 2000);
        assert_eq!(state.failed_attempts, 0);
    }

    #[test]
    fn test_admin_reset_invalid_password() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        let err = state.admin_reset("short", &profile, 2000).unwrap_err();
        assert!(matches!(err, PasswordError::TooShort { .. }));
    }

    #[test]
    fn test_admin_reset_same_as_current() {
        let profile = PasswordProfile::default();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        let err = state.admin_reset("Abc123!@#", &profile, 2000).unwrap_err();
        assert!(matches!(err, PasswordError::NewPasswordSameAsOld));
    }

    #[test]
    fn test_admin_reset_when_locked() {
        let profile = PasswordProfile::builder("test")
            .max_failures(2)
            .lockout_duration_secs(600)
            .build();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        for _ in 0..2 {
            state.verify("wrong", &profile, 1000).err();
        }
        assert!(state.is_locked(1000));

        // admin_reset 不检查锁定状态，可以直接重置
        state.admin_reset("Xyz789#$", &profile, 2000).unwrap();
        assert!(!state.is_locked(2000));
        assert_eq!(state.failed_attempts, 0);
    }

    #[test]
    fn test_admin_reset_unlocks_account() {
        let profile = PasswordProfile::builder("test")
            .max_failures(2)
            .lockout_duration_secs(600)
            .build();
        let mut state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();

        for _ in 0..2 {
            state.verify("wrong", &profile, 1000).err();
        }
        assert!(state.is_locked(1000));

        state.admin_reset("Xyz789#$", &profile, 1100).unwrap();
        assert!(!state.is_locked(1100));
    }

    // -----------------------------------------------------------------
    //  UserPasswordState — 过期检查
    // -----------------------------------------------------------------

    #[test]
    fn test_user_state_is_expired_false() {
        let profile = PasswordProfile::builder("test").lifetime_days(90).build();
        let state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        assert!(!state.is_expired(&profile, 1000 + 80 * SECS_PER_DAY));
    }

    #[test]
    fn test_user_state_is_expired_true() {
        let profile = PasswordProfile::builder("test").lifetime_days(90).build();
        let state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        assert!(state.is_expired(&profile, 1000 + 91 * SECS_PER_DAY));
    }

    #[test]
    fn test_user_state_is_expired_never() {
        let profile = PasswordProfile::builder("test").lifetime_days(0).build();
        let state = UserPasswordState::new("Abc123!@#", &profile, 1000).unwrap();
        assert!(!state.is_expired(&profile, 1_000_000_000));
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_bytes_to_hex() {
        assert_eq!(bytes_to_hex(&[0x00, 0xff, 0xab]), "00ffab");
        assert_eq!(bytes_to_hex(&[]), "");
    }

    #[test]
    fn test_hex_to_bytes_valid() {
        assert_eq!(hex_to_bytes("00ffab"), Some(vec![0x00, 0xff, 0xab]));
        assert_eq!(hex_to_bytes(""), Some(vec![]));
    }

    #[test]
    fn test_hex_to_bytes_invalid() {
        assert_eq!(hex_to_bytes("abc"), None); // 奇数长度
        assert_eq!(hex_to_bytes("xy"), None); // 非法字符
    }

    #[test]
    fn test_constant_time_eq_equal() {
        assert!(constant_time_eq(b"hello", b"hello"));
    }

    #[test]
    fn test_constant_time_eq_not_equal() {
        assert!(!constant_time_eq(b"hello", b"world"));
        assert!(!constant_time_eq(b"hello", b"hell"));
        assert!(!constant_time_eq(b"hello", b"helloo"));
    }

    #[test]
    fn test_max_consecutive_repeating() {
        assert_eq!(max_consecutive_repeating("aabbbcccc"), 4);
        assert_eq!(max_consecutive_repeating("abcdef"), 1);
        assert_eq!(max_consecutive_repeating(""), 0);
        assert_eq!(max_consecutive_repeating("aaaa"), 4);
        assert_eq!(max_consecutive_repeating("ababab"), 1);
    }

    // -----------------------------------------------------------------
    //  E2E 场景测试
    // -----------------------------------------------------------------

    #[test]
    fn test_e2e_create_password_profile_and_validate() {
        // 对应 spec: CREATE PASSWORD PROFILE → 密码长度/复杂度/有效期校验
        let mut registry = PasswordProfileRegistry::new();

        // 创建自定义 Profile
        let profile = PasswordProfile::builder("corp_policy")
            .min_length(12)
            .min_uppercase(2)
            .min_lowercase(2)
            .min_digit(2)
            .min_special(1)
            .max_repeating(3)
            .lifetime_days(60)
            .history_count(5)
            .max_failures(5)
            .lockout_duration_secs(1800)
            .build();
        registry.register(profile).unwrap();

        let p = registry.get("corp_policy").unwrap();

        // 合法密码（长度 12，满足 min_length=12）
        assert!(p.validate("ABab12!xyz99").is_ok());

        // 长度不足
        assert!(matches!(
            p.validate("ABab12!"),
            Err(PasswordError::TooShort { .. })
        ));

        // 大写不足
        assert!(matches!(
            p.validate("abab12!xyzab"),
            Err(PasswordError::NotEnoughUppercase { .. })
        ));

        // 连续重复过多
        assert!(matches!(
            p.validate("AAaab12!aaaa"),
            Err(PasswordError::TooManyRepeating { .. })
        ));

        // 有效期校验
        assert!(!p.is_expired(0, 59 * SECS_PER_DAY));
        assert!(p.is_expired(0, 61 * SECS_PER_DAY));
    }

    #[test]
    fn test_e2e_lockout_after_5_failures() {
        // 对应 spec: 连续失败 5 次锁定
        let profile = PasswordProfile::builder("test")
            .max_failures(5)
            .lockout_duration_secs(1800)
            .build();
        let mut state = UserPasswordState::new("Pass1!abc", &profile, 1000).unwrap();

        // 连续失败 4 次（未锁定）
        for i in 0..4 {
            let err = state.verify("wrong", &profile, 1000 + i).unwrap_err();
            assert!(matches!(err, PasswordError::IncorrectPassword));
        }
        assert!(!state.is_locked(1000));

        // 第 5 次失败 → 锁定
        let err = state.verify("wrong", &profile, 1000).unwrap_err();
        assert!(matches!(
            err,
            PasswordError::TooManyFailures {
                failures: 5,
                max: 5
            }
        ));
        assert!(state.is_locked(1000));
        assert_eq!(state.remaining_lock_secs(1000), 1800);
    }

    #[test]
    fn test_e2e_password_history_not_reusable() {
        // 对应 spec: 密码历史不可重用
        let profile = PasswordProfile::builder("test")
            .history_count(5)
            .reuse_interval_days(365)
            .build();
        let mut state = UserPasswordState::new("Pass1!aa", &profile, 1000).unwrap();

        // 循环修改 5 个密码
        let passwords = ["Pass2!bb", "Pass3!cc", "Pass4!dd", "Pass5!ee", "Pass6!ff"];
        let mut current = "Pass1!aa";
        let mut time = 1000u64;
        for new in &passwords {
            state.change_password(current, new, &profile, time).unwrap();
            current = new;
            time += 1000;
        }

        // 所有旧密码都不可重用
        for old in &["Pass1!aa", "Pass2!bb", "Pass3!cc", "Pass4!dd", "Pass5!ee"] {
            let err = state
                .change_password(current, old, &profile, time)
                .unwrap_err();
            assert!(
                matches!(err, PasswordError::PasswordInHistory { .. }),
                "should reject reuse of {old}"
            );
        }

        // 新密码可以使用
        state
            .change_password(current, "NewPass7!gg", &profile, time)
            .unwrap();
    }

    #[test]
    fn test_e2e_full_lifecycle() {
        // 完整生命周期：创建 → 验证 → 失败 → 锁定 → 解锁 → 修改 → 过期 → 重置
        let mut registry = PasswordProfileRegistry::new();
        let profile = PasswordProfile::builder("lifecycle")
            .min_length(8)
            .max_failures(3)
            .lockout_duration_secs(300)
            .lifetime_days(1)
            .history_count(3)
            .reuse_interval_days(7)
            .build();
        registry.register(profile).unwrap();
        let p = registry.get("lifecycle").unwrap();

        // 1. 创建用户
        let mut state = UserPasswordState::new("Init1!ab", p, 1000).unwrap();

        // 2. 验证成功
        assert!(state.verify("Init1!ab", p, 1000).is_ok());

        // 3. 失败 3 次 → 锁定
        for _ in 0..3 {
            state.verify("wrong", p, 1000).err();
        }
        assert!(state.is_locked(1000));

        // 4. 管理员解锁
        state.unlock();
        assert!(!state.is_locked(1000));

        // 5. 修改密码
        state
            .change_password("Init1!ab", "NewPwd2!cd", p, 2000)
            .unwrap();
        assert!(state.verify("NewPwd2!cd", p, 2000).is_ok());

        // 6. 密码过期（1 天后）
        assert!(state.is_expired(p, 2000 + 2 * SECS_PER_DAY));

        // 7. 管理员重置
        state.admin_reset("ResetPwd3!ef", p, 3000).unwrap();
        assert!(state.verify("ResetPwd3!ef", p, 3000).is_ok());
        assert!(!state.is_expired(p, 3000));

        // 8. 旧密码不可重用
        let err = state
            .change_password("ResetPwd3!ef", "NewPwd2!cd", p, 3000)
            .unwrap_err();
        assert!(matches!(err, PasswordError::PasswordInHistory { .. }));
    }

    #[test]
    fn test_e2e_multiple_profiles() {
        let mut registry = PasswordProfileRegistry::new();

        // 创建不同强度的 Profile
        registry
            .register(
                PasswordProfile::builder("weak")
                    .min_length(4)
                    .min_uppercase(0)
                    .min_lowercase(1)
                    .min_digit(0)
                    .min_special(0)
                    .max_failures(10)
                    .build(),
            )
            .unwrap();

        registry
            .register(
                PasswordProfile::builder("strong")
                    .min_length(16)
                    .min_uppercase(2)
                    .min_lowercase(2)
                    .min_digit(2)
                    .min_special(2)
                    .max_failures(3)
                    .lockout_duration_secs(3600)
                    .lifetime_days(30)
                    .history_count(10)
                    .build(),
            )
            .unwrap();

        let weak = registry.get("weak").unwrap();
        let strong = registry.get("strong").unwrap();

        // weak 接受简单密码
        assert!(weak.validate("abcd").is_ok());
        // strong 拒绝简单密码
        assert!(strong.validate("abcd").is_err());
        // strong 接受复杂密码（长度 16，满足 min_length=16）
        assert!(strong.validate("ABab12!@xyz98765").is_ok());

        // 不同 Profile 不同锁定策略
        let mut weak_user = UserPasswordState::new("abcd", weak, 1000).unwrap();
        for _ in 0..9 {
            weak_user.verify("wrong", weak, 1000).err();
        }
        assert!(!weak_user.is_locked(1000)); // 9 < 10

        let mut strong_user = UserPasswordState::new("ABab12!@xyz98765", strong, 1000).unwrap();
        for _ in 0..3 {
            strong_user.verify("wrong", strong, 1000).err();
        }
        assert!(strong_user.is_locked(1000)); // 3 == 3
    }

    #[test]
    #[ignore = "压力测试：1000 次密码修改耗时较长，使用 --ignored 手动触发"]
    fn test_e2e_stress_1000_password_changes() {
        // 压力测试：1000 次密码修改
        let profile = PasswordProfile::builder("stress")
            .history_count(5)
            .reuse_interval_days(0) // 仅受 history_count 限制
            .build();
        let mut state = UserPasswordState::new("Init1!aa", &profile, 0).unwrap();

        // 生成 1000 个合法密码
        let passwords: Vec<String> = (0..1000).map(|i| format!("Pwd{i:04}!Xx")).collect();

        let mut current = "Init1!aa".to_string();
        for (i, new) in passwords.iter().enumerate() {
            // 复杂度校验：Pwd0000!Xx → 长度 10，含大写/小写/数字/特殊字符
            state
                .change_password(&current, new, &profile, i as u64 * 1000)
                .unwrap_or_else(|e| panic!("change to {new} failed: {e}"));
            current = new.clone();
        }

        // 验证最终密码
        assert!(state.verify(&passwords[999], &profile, 1_000_000).is_ok());

        // 最近 5 个密码不可重用
        for i in 995..999 {
            let err = state
                .change_password(&passwords[999], &passwords[i], &profile, 1_000_001)
                .unwrap_err();
            assert!(matches!(err, PasswordError::PasswordInHistory { .. }));
        }

        // history 应裁剪到 5
        assert_eq!(state.history.len(), 5);
    }
}
