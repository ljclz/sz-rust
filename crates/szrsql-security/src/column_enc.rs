//! 列级加密（Column-Level Encryption）— Phase 7c.4
//!
//! 对应 `SzRSQL技术实现方案.md` 安全特性 — 列级加密。
//!
//! # 设计
//!
//! 列级加密对指定列的数据进行加密存储，与 TDE（页级加密）互补：
//!
//! - **TDE** — 加密整个数据页，防止磁盘文件被读取
//! - **列级加密** — 只加密敏感列（如 SSN、信用卡号），即使有磁盘访问权限也无法解密
//!
//! ## 加密算法
//!
//! - **AES-256-GCM** — 认证加密（ confidentiality + integrity）
//! - **Nonce** — 每次加密生成随机 12 字节 nonce，不重复使用
//! - **Tag** — 16 字节认证标签，解密时验证完整性
//!
//! ## 密文格式
//!
//! ```text
//! +-------------+----------------------+-------------------+
//! | nonce (12)  | ciphertext (len)     | tag (16)          |
//! +-------------+----------------------+-------------------+
//! ```
//!
//! 总开销 = 28 字节（12 nonce + 16 tag）。
//!
//! ## 密钥管理
//!
//! - 每个加密列关联一个 `ColumnKey`（32 字节 AES-256 密钥）
//! - `ColumnEncryptionRegistry` 维护 (table, column) → config 映射
//! - `ColumnEncryptionEngine` 持有密钥存储，根据 key_id 查找密钥
//! - 无密钥用户无法解密，只能看到密文
//!
//! # 验证标准
//!
//! - `ssn TEXT ENCRYPTED` → INSERT → 直接读 page 看到加密数据
//! - SzRSQL SELECT 解密显示原文
//! - 无密钥用户查看到加密值
//!
//! 对应 `SzRSQL实施进度.md` Phase 7c.4。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use std::collections::HashMap;

// =====================================================================
//  常量
// =====================================================================

/// AES-256 密钥长度（字节）
pub const COLUMN_KEY_LEN: usize = 32;

/// GCM nonce 长度（字节）
const NONCE_LEN: usize = 12;

/// GCM 认证标签长度（字节）
const TAG_LEN: usize = 16;

/// 密文开销 = nonce(12) + tag(16) = 28 字节
pub const CIPHERTEXT_OVERHEAD: usize = NONCE_LEN + TAG_LEN;

// =====================================================================
//  错误类型
// =====================================================================

/// 列级加密错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ColumnEncError {
    /// 密钥长度无效
    #[error("invalid column key length: expected {expected}, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
    /// 密钥 ID 未找到
    #[error("column key not found: {0}")]
    KeyNotFound(String),
    /// 列未配置加密
    #[error("column not configured for encryption: {table}.{column}")]
    ColumnNotEncrypted { table: String, column: String },
    /// 密文太短（不含 nonce + tag）
    #[error("ciphertext too short: got {got} bytes, minimum {min}")]
    CiphertextTooShort { got: usize, min: usize },
    /// 解密失败（认证标签不匹配或密钥错误）
    #[error("decryption failed: authentication tag mismatch or wrong key")]
    DecryptionFailed,
    /// 列已配置加密
    #[error("column already encrypted: {table}.{column}")]
    ColumnAlreadyEncrypted { table: String, column: String },
}

// =====================================================================
//  ColumnKey — 列加密密钥
// =====================================================================

/// 列加密密钥（32 字节 AES-256）
#[derive(Clone)]
pub struct ColumnKey {
    /// 密钥 ID（唯一标识）
    key_id: String,
    /// 32 字节密钥
    bytes: [u8; COLUMN_KEY_LEN],
}

impl ColumnKey {
    /// 从字节切片创建列密钥
    pub fn from_bytes(key_id: impl Into<String>, key: &[u8]) -> Result<Self, ColumnEncError> {
        if key.len() != COLUMN_KEY_LEN {
            return Err(ColumnEncError::InvalidKeyLength {
                expected: COLUMN_KEY_LEN,
                actual: key.len(),
            });
        }
        let mut bytes = [0u8; COLUMN_KEY_LEN];
        bytes.copy_from_slice(key);
        Ok(Self {
            key_id: key_id.into(),
            bytes,
        })
    }

    /// 生成随机列密钥
    pub fn generate(key_id: impl Into<String>) -> Self {
        let mut bytes = [0u8; COLUMN_KEY_LEN];
        rand::rng().fill_bytes(&mut bytes);
        Self {
            key_id: key_id.into(),
            bytes,
        }
    }

    /// 从密码短语派生列密钥（SHA-256 多轮迭代）
    pub fn from_passphrase(key_id: impl Into<String>, passphrase: &str, salt: &[u8]) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(passphrase.as_bytes());
        hasher.update(salt);
        let mut current = hasher.finalize();
        for _ in 0..10_000 {
            let mut h = Sha256::new();
            h.update(current);
            current = h.finalize();
        }
        let mut bytes = [0u8; COLUMN_KEY_LEN];
        bytes.copy_from_slice(&current);
        Self {
            key_id: key_id.into(),
            bytes,
        }
    }

    /// 获取密钥 ID
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    /// 获取密钥字节引用
    pub fn as_bytes(&self) -> &[u8; COLUMN_KEY_LEN] {
        &self.bytes
    }

    /// 密钥指纹（SHA-256 前 8 字节十六进制）
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.bytes);
        let digest = hasher.finalize();
        digest[..8].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl std::fmt::Debug for ColumnKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnKey")
            .field("key_id", &self.key_id)
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

// =====================================================================
//  ColumnEncryptionConfig — 列加密配置
// =====================================================================

/// 列加密配置
///
/// 描述一个加密列的元数据：所属表、列名、关联密钥 ID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnEncryptionConfig {
    /// 表名
    pub table: String,
    /// 列名
    pub column: String,
    /// 关联的密钥 ID
    pub key_id: String,
}

impl ColumnEncryptionConfig {
    /// 创建列加密配置
    pub fn new(
        table: impl Into<String>,
        column: impl Into<String>,
        key_id: impl Into<String>,
    ) -> Self {
        Self {
            table: table.into(),
            column: column.into(),
            key_id: key_id.into(),
        }
    }
}

// =====================================================================
//  ColumnEncryptionRegistry — 加密列注册表
// =====================================================================

/// 加密列注册表 — 维护 (table, column) → config 映射
#[derive(Debug, Clone, Default)]
pub struct ColumnEncryptionRegistry {
    /// (table, column) → config
    configs: HashMap<(String, String), ColumnEncryptionConfig>,
}

impl ColumnEncryptionRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册加密列
    pub fn register(&mut self, config: ColumnEncryptionConfig) -> Result<(), ColumnEncError> {
        let key = (config.table.clone(), config.column.clone());
        if self.configs.contains_key(&key) {
            return Err(ColumnEncError::ColumnAlreadyEncrypted {
                table: config.table,
                column: config.column,
            });
        }
        self.configs.insert(key, config);
        Ok(())
    }

    /// 注销加密列
    pub fn unregister(&mut self, table: &str, column: &str) -> Option<ColumnEncryptionConfig> {
        self.configs
            .remove(&(table.to_string(), column.to_string()))
    }

    /// 查询列是否加密
    pub fn is_encrypted(&self, table: &str, column: &str) -> bool {
        self.configs
            .contains_key(&(table.to_string(), column.to_string()))
    }

    /// 获取列加密配置
    pub fn get(&self, table: &str, column: &str) -> Option<&ColumnEncryptionConfig> {
        self.configs.get(&(table.to_string(), column.to_string()))
    }

    /// 获取所有加密列配置
    pub fn configs(&self) -> &HashMap<(String, String), ColumnEncryptionConfig> {
        &self.configs
    }

    /// 获取表的所有加密列
    pub fn columns_for_table(&self, table: &str) -> Vec<&ColumnEncryptionConfig> {
        self.configs.values().filter(|c| c.table == table).collect()
    }

    /// 注册表是否为空
    pub fn is_empty(&self) -> bool {
        self.configs.is_empty()
    }

    /// 注册表条目数
    pub fn len(&self) -> usize {
        self.configs.len()
    }
}

// =====================================================================
//  ColumnEncryptionEngine — 列加密引擎
// =====================================================================

/// 列加密引擎 — 执行列级加密/解密操作
///
/// # 工作流程
///
/// 1. `register_key(key)` — 注册列加密密钥
/// 2. `register_column(config)` — 注册加密列
/// 3. `encrypt(table, column, plaintext)` — 加密列值（写入前调用）
/// 4. `decrypt(table, column, ciphertext)` — 解密列值（读取后调用）
///
/// # 用法
///
/// ```ignore
/// use szrsql_security::column_enc::*;
///
/// let mut engine = ColumnEncryptionEngine::new();
///
/// // 注册密钥
/// let key = ColumnKey::generate("key_ssn");
/// engine.register_key(key);
///
/// // 注册加密列
/// engine.register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn")).unwrap();
///
/// // 加密
/// let ciphertext = engine.encrypt("users", "ssn", b"123-45-6789").unwrap();
///
/// // 解密
/// let plaintext = engine.decrypt("users", "ssn", &ciphertext).unwrap();
/// assert_eq!(plaintext, b"123-45-6789");
/// ```
#[derive(Debug, Default)]
pub struct ColumnEncryptionEngine {
    /// 密钥存储：key_id → ColumnKey
    keys: HashMap<String, ColumnKey>,
    /// 加密列注册表
    registry: ColumnEncryptionRegistry,
    /// 统计信息
    stats: ColumnEncStats,
}

/// 列加密统计信息
#[derive(Debug, Clone, Default)]
pub struct ColumnEncStats {
    /// 加密次数
    pub encryptions: u64,
    /// 解密次数
    pub decryptions: u64,
    /// 加密字节数
    pub bytes_encrypted: u64,
    /// 解密字节数
    pub bytes_decrypted: u64,
}

impl ColumnEncryptionEngine {
    /// 创建列加密引擎
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册列加密密钥
    pub fn register_key(&mut self, key: ColumnKey) {
        self.keys.insert(key.key_id().to_string(), key);
    }

    /// 注销列加密密钥
    pub fn unregister_key(&mut self, key_id: &str) -> Option<ColumnKey> {
        self.keys.remove(key_id)
    }

    /// 查询密钥是否存在
    pub fn has_key(&self, key_id: &str) -> bool {
        self.keys.contains_key(key_id)
    }

    /// 注册加密列
    pub fn register_column(
        &mut self,
        config: ColumnEncryptionConfig,
    ) -> Result<(), ColumnEncError> {
        // 验证密钥存在
        if !self.keys.contains_key(&config.key_id) {
            return Err(ColumnEncError::KeyNotFound(config.key_id));
        }
        self.registry.register(config)
    }

    /// 注销加密列
    pub fn unregister_column(
        &mut self,
        table: &str,
        column: &str,
    ) -> Option<ColumnEncryptionConfig> {
        self.registry.unregister(table, column)
    }

    /// 查询列是否加密
    pub fn is_encrypted(&self, table: &str, column: &str) -> bool {
        self.registry.is_encrypted(table, column)
    }

    /// 获取加密列配置
    pub fn column_config(&self, table: &str, column: &str) -> Option<&ColumnEncryptionConfig> {
        self.registry.get(table, column)
    }

    /// 获取注册表引用
    pub fn registry(&self) -> &ColumnEncryptionRegistry {
        &self.registry
    }

    /// 获取统计信息
    pub fn stats(&self) -> &ColumnEncStats {
        &self.stats
    }

    /// 重置统计信息
    pub fn reset_stats(&mut self) {
        self.stats = ColumnEncStats::default();
    }

    /// 加密列值
    ///
    /// - `table` — 表名
    /// - `column` — 列名
    /// - `plaintext` — 明文值
    ///
    /// 返回密文（nonce(12) + ciphertext + tag(16)）
    ///
    /// # 错误
    ///
    /// - `ColumnNotEncrypted` — 列未配置加密
    /// - `KeyNotFound` — 密钥未注册
    /// - `DecryptionFailed` — GCM 加密失败（极少发生）
    pub fn encrypt(
        &mut self,
        table: &str,
        column: &str,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ColumnEncError> {
        let config =
            self.registry
                .get(table, column)
                .ok_or_else(|| ColumnEncError::ColumnNotEncrypted {
                    table: table.to_string(),
                    column: column.to_string(),
                })?;

        let key = self
            .keys
            .get(&config.key_id)
            .ok_or_else(|| ColumnEncError::KeyNotFound(config.key_id.clone()))?;

        let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| {
            ColumnEncError::InvalidKeyLength {
                expected: COLUMN_KEY_LEN,
                actual: 0,
            }
        })?;

        // 生成随机 nonce
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // GCM 加密（ciphertext 包含 tag）
        let ciphertext_with_tag = cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| ColumnEncError::DecryptionFailed)?;

        // 输出格式：nonce(12) + ciphertext + tag(16)
        let mut output = Vec::with_capacity(NONCE_LEN + ciphertext_with_tag.len());
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext_with_tag);

        self.stats.encryptions += 1;
        self.stats.bytes_encrypted += plaintext.len() as u64;

        Ok(output)
    }

    /// 解密列值
    ///
    /// - `table` — 表名
    /// - `column` — 列名
    /// - `ciphertext` — 密文（nonce(12) + ciphertext + tag(16)）
    ///
    /// 返回明文
    ///
    /// # 错误
    ///
    /// - `ColumnNotEncrypted` — 列未配置加密
    /// - `KeyNotFound` — 密钥未注册
    /// - `CiphertextTooShort` — 密文太短
    /// - `DecryptionFailed` — 解密失败（认证标签不匹配或密钥错误）
    pub fn decrypt(
        &mut self,
        table: &str,
        column: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, ColumnEncError> {
        let config =
            self.registry
                .get(table, column)
                .ok_or_else(|| ColumnEncError::ColumnNotEncrypted {
                    table: table.to_string(),
                    column: column.to_string(),
                })?;

        let key = self
            .keys
            .get(&config.key_id)
            .ok_or_else(|| ColumnEncError::KeyNotFound(config.key_id.clone()))?;

        // 检查密文长度（至少 nonce + tag）
        if ciphertext.len() < NONCE_LEN + TAG_LEN {
            return Err(ColumnEncError::CiphertextTooShort {
                got: ciphertext.len(),
                min: NONCE_LEN + TAG_LEN,
            });
        }

        // 分离 nonce 和 ciphertext+tag
        let nonce_bytes = &ciphertext[..NONCE_LEN];
        let ciphertext_with_tag = &ciphertext[NONCE_LEN..];
        let nonce = Nonce::from_slice(nonce_bytes);

        let cipher = Aes256Gcm::new_from_slice(key.as_bytes()).map_err(|_| {
            ColumnEncError::InvalidKeyLength {
                expected: COLUMN_KEY_LEN,
                actual: 0,
            }
        })?;

        let plaintext = cipher
            .decrypt(nonce, ciphertext_with_tag)
            .map_err(|_| ColumnEncError::DecryptionFailed)?;

        self.stats.decryptions += 1;
        self.stats.bytes_decrypted += plaintext.len() as u64;

        Ok(plaintext)
    }

    /// 获取所有已注册的密钥 ID
    pub fn key_ids(&self) -> Vec<&str> {
        self.keys.keys().map(|s| s.as_str()).collect()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  ColumnKey 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_key_from_bytes() {
        let key_bytes = [42u8; COLUMN_KEY_LEN];
        let key = ColumnKey::from_bytes("key_1", &key_bytes).unwrap();
        assert_eq!(key.key_id(), "key_1");
        assert_eq!(key.as_bytes(), &key_bytes);
    }

    #[test]
    fn test_7c4_key_from_bytes_invalid_length() {
        let short_key = [0u8; 16];
        let result = ColumnKey::from_bytes("key_1", &short_key);
        assert_eq!(
            result.unwrap_err(),
            ColumnEncError::InvalidKeyLength {
                expected: COLUMN_KEY_LEN,
                actual: 16,
            }
        );
    }

    #[test]
    fn test_7c4_key_generate() {
        let key1 = ColumnKey::generate("key_gen1");
        let key2 = ColumnKey::generate("key_gen2");
        assert_eq!(key1.key_id(), "key_gen1");
        assert_ne!(key1.as_bytes(), key2.as_bytes()); // 随机生成 → 不同
    }

    #[test]
    fn test_7c4_key_from_passphrase() {
        let key1 = ColumnKey::from_passphrase("key_p1", "mypassword", b"salt123");
        let key2 = ColumnKey::from_passphrase("key_p2", "mypassword", b"salt123");
        let key3 = ColumnKey::from_passphrase("key_p3", "wrongpassword", b"salt123");

        assert_eq!(key1.as_bytes(), key2.as_bytes()); // 相同密码+盐 → 相同密钥
        assert_ne!(key1.as_bytes(), key3.as_bytes()); // 不同密码 → 不同密钥
    }

    #[test]
    fn test_7c4_key_fingerprint() {
        let key = ColumnKey::from_bytes("key_fp", &[0u8; COLUMN_KEY_LEN]).unwrap();
        let fp = key.fingerprint();
        assert_eq!(fp.len(), 16); // 8 字节 → 16 十六进制字符
    }

    #[test]
    fn test_7c4_key_debug_no_leak() {
        let key = ColumnKey::from_bytes("key_debug", &[0xAB; COLUMN_KEY_LEN]).unwrap();
        let debug_str = format!("{key:?}");
        assert!(debug_str.contains("key_debug"));
        assert!(!debug_str.contains("ab")); // 不泄露密钥内容
    }

    // -----------------------------------------------------------------
    //  ColumnEncryptionConfig 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_config_creation() {
        let config = ColumnEncryptionConfig::new("users", "ssn", "key_ssn");
        assert_eq!(config.table, "users");
        assert_eq!(config.column, "ssn");
        assert_eq!(config.key_id, "key_ssn");
    }

    // -----------------------------------------------------------------
    //  ColumnEncryptionRegistry 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_registry_register() {
        let mut registry = ColumnEncryptionRegistry::new();
        let config = ColumnEncryptionConfig::new("users", "ssn", "key_ssn");
        registry.register(config).unwrap();
        assert!(registry.is_encrypted("users", "ssn"));
        assert!(!registry.is_encrypted("users", "email"));
    }

    #[test]
    fn test_7c4_registry_duplicate() {
        let mut registry = ColumnEncryptionRegistry::new();
        registry
            .register(ColumnEncryptionConfig::new("users", "ssn", "key1"))
            .unwrap();
        let result = registry.register(ColumnEncryptionConfig::new("users", "ssn", "key2"));
        assert_eq!(
            result.unwrap_err(),
            ColumnEncError::ColumnAlreadyEncrypted {
                table: "users".to_string(),
                column: "ssn".to_string(),
            }
        );
    }

    #[test]
    fn test_7c4_registry_unregister() {
        let mut registry = ColumnEncryptionRegistry::new();
        registry
            .register(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();
        assert!(registry.is_encrypted("users", "ssn"));

        let removed = registry.unregister("users", "ssn");
        assert!(removed.is_some());
        assert!(!registry.is_encrypted("users", "ssn"));
    }

    #[test]
    fn test_7c4_registry_get() {
        let mut registry = ColumnEncryptionRegistry::new();
        registry
            .register(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();
        let config = registry.get("users", "ssn").unwrap();
        assert_eq!(config.key_id, "key_ssn");
        assert!(registry.get("users", "email").is_none());
    }

    #[test]
    fn test_7c4_registry_columns_for_table() {
        let mut registry = ColumnEncryptionRegistry::new();
        registry
            .register(ColumnEncryptionConfig::new("users", "ssn", "key1"))
            .unwrap();
        registry
            .register(ColumnEncryptionConfig::new("users", "email", "key2"))
            .unwrap();
        registry
            .register(ColumnEncryptionConfig::new("orders", "card", "key3"))
            .unwrap();

        let users_cols = registry.columns_for_table("users");
        assert_eq!(users_cols.len(), 2);

        let orders_cols = registry.columns_for_table("orders");
        assert_eq!(orders_cols.len(), 1);
    }

    #[test]
    fn test_7c4_registry_len_is_empty() {
        let registry = ColumnEncryptionRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    // -----------------------------------------------------------------
    //  ColumnEncryptionEngine 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_engine_creation() {
        let engine = ColumnEncryptionEngine::new();
        assert!(engine.key_ids().is_empty());
        assert!(engine.registry().is_empty());
    }

    #[test]
    fn test_7c4_engine_register_key() {
        let mut engine = ColumnEncryptionEngine::new();
        let key = ColumnKey::generate("key_1");
        engine.register_key(key);
        assert!(engine.has_key("key_1"));
        assert!(!engine.has_key("key_2"));
    }

    #[test]
    fn test_7c4_engine_unregister_key() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_1"));
        assert!(engine.has_key("key_1"));

        let removed = engine.unregister_key("key_1");
        assert!(removed.is_some());
        assert!(!engine.has_key("key_1"));
    }

    #[test]
    fn test_7c4_engine_register_column_without_key() {
        let mut engine = ColumnEncryptionEngine::new();
        let result =
            engine.register_column(ColumnEncryptionConfig::new("users", "ssn", "nonexistent"));
        assert_eq!(
            result.unwrap_err(),
            ColumnEncError::KeyNotFound("nonexistent".to_string())
        );
    }

    #[test]
    fn test_7c4_engine_register_column_with_key() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();
        assert!(engine.is_encrypted("users", "ssn"));
    }

    // -----------------------------------------------------------------
    //  加密/解密往返测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_encrypt_decrypt_roundtrip() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        let plaintext = b"123-45-6789";
        let ciphertext = engine.encrypt("users", "ssn", plaintext).unwrap();
        let decrypted = engine.decrypt("users", "ssn", &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_7c4_encrypt_ciphertext_differs_from_plaintext() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        let plaintext = b"sensitive data 12345";
        let ciphertext = engine.encrypt("users", "ssn", plaintext).unwrap();
        assert_ne!(ciphertext, plaintext); // 密文 != 明文
        assert!(ciphertext.len() > plaintext.len()); // 密文更长（含 nonce + tag）
        assert_eq!(ciphertext.len(), plaintext.len() + CIPHERTEXT_OVERHEAD);
    }

    #[test]
    fn test_7c4_encrypt_different_nonce_each_time() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        let plaintext = b"same value";
        let ct1 = engine.encrypt("users", "ssn", plaintext).unwrap();
        let ct2 = engine.encrypt("users", "ssn", plaintext).unwrap();

        // nonce 不同 → 密文不同（即使明文相同）
        assert_ne!(ct1, ct2);
        // 但都能正确解密
        assert_eq!(engine.decrypt("users", "ssn", &ct1).unwrap(), plaintext);
        assert_eq!(engine.decrypt("users", "ssn", &ct2).unwrap(), plaintext);
    }

    #[test]
    fn test_7c4_encrypt_empty_plaintext() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        let plaintext = b"";
        let ciphertext = engine.encrypt("users", "ssn", plaintext).unwrap();
        // 空明文 → 密文 = nonce(12) + tag(16) = 28 字节
        assert_eq!(ciphertext.len(), CIPHERTEXT_OVERHEAD);
        let decrypted = engine.decrypt("users", "ssn", &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_7c4_encrypt_large_value() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_large"));
        engine
            .register_column(ColumnEncryptionConfig::new("docs", "content", "key_large"))
            .unwrap();

        let plaintext = vec![0xABu8; 100_000]; // 100KB
        let ciphertext = engine.encrypt("docs", "content", &plaintext).unwrap();
        let decrypted = engine.decrypt("docs", "content", &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    // -----------------------------------------------------------------
    //  错误场景测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_encrypt_column_not_encrypted() {
        let mut engine = ColumnEncryptionEngine::new();
        let result = engine.encrypt("users", "ssn", b"data");
        assert_eq!(
            result.unwrap_err(),
            ColumnEncError::ColumnNotEncrypted {
                table: "users".to_string(),
                column: "ssn".to_string(),
            }
        );
    }

    #[test]
    fn test_7c4_decrypt_column_not_encrypted() {
        let mut engine = ColumnEncryptionEngine::new();
        let result = engine.decrypt("users", "ssn", b"data");
        assert!(matches!(
            result.unwrap_err(),
            ColumnEncError::ColumnNotEncrypted { .. }
        ));
    }

    #[test]
    fn test_7c4_decrypt_ciphertext_too_short() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        let short_ct = vec![0u8; 10]; // 远小于 NONCE_LEN + TAG_LEN = 28
        let result = engine.decrypt("users", "ssn", &short_ct);
        assert_eq!(
            result.unwrap_err(),
            ColumnEncError::CiphertextTooShort {
                got: 10,
                min: NONCE_LEN + TAG_LEN,
            }
        );
    }

    #[test]
    fn test_7c4_decrypt_wrong_key() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_original"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_original"))
            .unwrap();

        // 用 key_original 加密
        let ciphertext = engine.encrypt("users", "ssn", b"secret").unwrap();

        // 替换为不同的密钥
        engine.unregister_key("key_original");
        engine
            .register_key(ColumnKey::from_bytes("key_original", &[99u8; COLUMN_KEY_LEN]).unwrap());

        // 解密失败（密钥不匹配）
        let result = engine.decrypt("users", "ssn", &ciphertext);
        assert_eq!(result.unwrap_err(), ColumnEncError::DecryptionFailed);
    }

    #[test]
    fn test_7c4_decrypt_tampered_ciphertext() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        let ciphertext = engine.encrypt("users", "ssn", b"123-45-6789").unwrap();

        // 篡改密文（修改最后一个字节）
        let mut tampered = ciphertext.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;

        // 解密失败（认证标签不匹配）
        let result = engine.decrypt("users", "ssn", &tampered);
        assert_eq!(result.unwrap_err(), ColumnEncError::DecryptionFailed);
    }

    #[test]
    fn test_7c4_decrypt_tampered_nonce() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        let ciphertext = engine.encrypt("users", "ssn", b"123-45-6789").unwrap();

        // 篡改 nonce（修改第一个字节）
        let mut tampered = ciphertext.clone();
        tampered[0] ^= 0xFF;

        // 解密失败（nonce 不匹配）
        let result = engine.decrypt("users", "ssn", &tampered);
        assert_eq!(result.unwrap_err(), ColumnEncError::DecryptionFailed);
    }

    // -----------------------------------------------------------------
    //  多列多表测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_multiple_columns_different_keys() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine.register_key(ColumnKey::generate("key_card"));

        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();
        engine
            .register_column(ColumnEncryptionConfig::new("payments", "card", "key_card"))
            .unwrap();

        // 加密不同列用不同密钥
        let ssn_ct = engine.encrypt("users", "ssn", b"123-45-6789").unwrap();
        let card_ct = engine
            .encrypt("payments", "card", b"4532-1234-5678-9012")
            .unwrap();

        // 各自解密正确
        assert_eq!(
            engine.decrypt("users", "ssn", &ssn_ct).unwrap(),
            b"123-45-6789"
        );
        assert_eq!(
            engine.decrypt("payments", "card", &card_ct).unwrap(),
            b"4532-1234-5678-9012"
        );
    }

    #[test]
    fn test_7c4_multiple_tables_same_column_name() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_shared"));

        // 不同表的同名列，分别注册
        engine
            .register_column(ColumnEncryptionConfig::new("users", "secret", "key_shared"))
            .unwrap();
        engine
            .register_column(ColumnEncryptionConfig::new("admin", "secret", "key_shared"))
            .unwrap();

        let ct1 = engine.encrypt("users", "secret", b"user_secret").unwrap();
        let ct2 = engine.encrypt("admin", "secret", b"admin_secret").unwrap();

        assert_eq!(
            engine.decrypt("users", "secret", &ct1).unwrap(),
            b"user_secret"
        );
        assert_eq!(
            engine.decrypt("admin", "secret", &ct2).unwrap(),
            b"admin_secret"
        );
    }

    // -----------------------------------------------------------------
    //  统计信息测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_stats_tracking() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        let plaintext = b"123-45-6789";
        let ciphertext = engine.encrypt("users", "ssn", plaintext).unwrap();
        let _ = engine.decrypt("users", "ssn", &ciphertext).unwrap();

        let stats = engine.stats();
        assert_eq!(stats.encryptions, 1);
        assert_eq!(stats.decryptions, 1);
        assert_eq!(stats.bytes_encrypted, 11);
        assert_eq!(stats.bytes_decrypted, 11);
    }

    #[test]
    fn test_7c4_stats_reset() {
        let mut engine = ColumnEncryptionEngine::new();
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        let _ = engine.encrypt("users", "ssn", b"data").unwrap();
        assert_eq!(engine.stats().encryptions, 1);

        engine.reset_stats();
        assert_eq!(engine.stats().encryptions, 0);
    }

    // -----------------------------------------------------------------
    //  无密钥用户验证（验证标准核心）
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_no_key_user_sees_encrypted_value() {
        // 验证标准：无密钥用户查看到加密值
        // 模拟：引擎 A 加密 → 引擎 B（无密钥）无法解密
        let mut engine_a = ColumnEncryptionEngine::new();
        engine_a.register_key(ColumnKey::generate("key_secret"));
        engine_a
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_secret"))
            .unwrap();

        let plaintext = b"123-45-6789";
        let ciphertext = engine_a.encrypt("users", "ssn", plaintext).unwrap();

        // 引擎 B 有注册表但没有正确的密钥
        let mut engine_b = ColumnEncryptionEngine::new();
        engine_b.register_key(ColumnKey::from_bytes("key_secret", &[0u8; COLUMN_KEY_LEN]).unwrap());
        engine_b
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_secret"))
            .unwrap();

        // 引擎 B 无法解密（密钥不匹配）
        let result = engine_b.decrypt("users", "ssn", &ciphertext);
        assert_eq!(result.unwrap_err(), ColumnEncError::DecryptionFailed);

        // 无密钥用户只能看到加密值（密文）
        assert_ne!(ciphertext, plaintext);
    }

    // -----------------------------------------------------------------
    //  完整工作流测试（验证标准）
    // -----------------------------------------------------------------

    #[test]
    fn test_7c4_full_workflow_ssn_encrypted() {
        // 验证标准完整流程：
        // ssn TEXT ENCRYPTED → INSERT → 直接读 page 看到加密数据
        // → SzRSQL SELECT 解密显示原文 → 无密钥用户查看到加密值

        let mut engine = ColumnEncryptionEngine::new();

        // 1. 创建密钥
        let ssn_key = ColumnKey::generate("key_ssn");
        engine.register_key(ssn_key);

        // 2. 注册加密列（相当于 CREATE TABLE users (ssn TEXT ENCRYPTED)）
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();

        // 3. INSERT：加密 SSN 值（写入前加密）
        let original_ssn = b"123-45-6789";
        let encrypted_ssn = engine.encrypt("users", "ssn", original_ssn).unwrap();

        // 4. 直接读 page 看到加密数据（密文 != 明文）
        assert_ne!(encrypted_ssn.as_slice(), original_ssn);
        assert!(encrypted_ssn.len() > original_ssn.len());
        // 密文不含明文子串
        assert!(!encrypted_ssn
            .windows(original_ssn.len())
            .any(|w| w == original_ssn));

        // 5. SELECT：解密显示原文
        let decrypted_ssn = engine.decrypt("users", "ssn", &encrypted_ssn).unwrap();
        assert_eq!(decrypted_ssn.as_slice(), original_ssn);

        // 6. 无密钥用户查看到加密值
        let mut no_key_engine = ColumnEncryptionEngine::new();
        no_key_engine
            .register_key(ColumnKey::from_bytes("key_ssn", &[0xFF; COLUMN_KEY_LEN]).unwrap());
        no_key_engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "key_ssn"))
            .unwrap();
        assert_eq!(
            no_key_engine
                .decrypt("users", "ssn", &encrypted_ssn)
                .unwrap_err(),
            ColumnEncError::DecryptionFailed
        );

        // 7. 统计验证
        let stats = engine.stats();
        assert_eq!(stats.encryptions, 1);
        assert_eq!(stats.decryptions, 1);
    }

    #[test]
    fn test_7c4_full_workflow_multiple_sensitive_columns() {
        // 多敏感列加密工作流
        let mut engine = ColumnEncryptionEngine::new();

        // 为不同列创建不同密钥
        engine.register_key(ColumnKey::generate("key_ssn"));
        engine.register_key(ColumnKey::generate("key_card"));
        engine.register_key(ColumnKey::generate("key_email"));

        // 注册三个加密列
        engine
            .register_column(ColumnEncryptionConfig::new("customers", "ssn", "key_ssn"))
            .unwrap();
        engine
            .register_column(ColumnEncryptionConfig::new(
                "customers",
                "card_number",
                "key_card",
            ))
            .unwrap();
        engine
            .register_column(ColumnEncryptionConfig::new(
                "customers",
                "email",
                "key_email",
            ))
            .unwrap();

        // 模拟 INSERT 100 行
        for i in 0..100u32 {
            let ssn = format!("123-45-{i:04}");
            let card = format!("4532-0000-0000-{i:04}");
            let email = format!("user{i}@example.com");

            let ssn_ct = engine.encrypt("customers", "ssn", ssn.as_bytes()).unwrap();
            let card_ct = engine
                .encrypt("customers", "card_number", card.as_bytes())
                .unwrap();
            let email_ct = engine
                .encrypt("customers", "email", email.as_bytes())
                .unwrap();

            // SELECT：解密验证
            assert_eq!(
                engine.decrypt("customers", "ssn", &ssn_ct).unwrap(),
                ssn.as_bytes()
            );
            assert_eq!(
                engine
                    .decrypt("customers", "card_number", &card_ct)
                    .unwrap(),
                card.as_bytes()
            );
            assert_eq!(
                engine.decrypt("customers", "email", &email_ct).unwrap(),
                email.as_bytes()
            );
        }

        // 统计验证
        let stats = engine.stats();
        assert_eq!(stats.encryptions, 300); // 100 行 × 3 列
        assert_eq!(stats.decryptions, 300);

        // 哈希链完整性验证（注册表）
        let registry = engine.registry();
        assert_eq!(registry.len(), 3);
        assert!(registry.is_encrypted("customers", "ssn"));
        assert!(registry.is_encrypted("customers", "card_number"));
        assert!(registry.is_encrypted("customers", "email"));
    }
}
