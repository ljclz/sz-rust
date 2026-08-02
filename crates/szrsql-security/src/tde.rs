//! TDE 透明数据加密（Transparent Data Encryption）— Phase 7c.1
//!
//! 对应 `SzRSQL技术实现方案.md` 9.25 节。
//!
//! # 设计
//!
//! TDE 在存储层透明加密数据页，对上层 SQL 引擎完全透明：
//!
//! 1. **启用 TDE** — `TdeEngine::enable(key)` 设置 32 字节主密钥
//! 2. **写入加密** — 存储层写页前调用 `encrypt_page(page_id, plaintext)` → 密文落盘
//! 3. **读取解密** — 存储层读页后调用 `decrypt_page(page_id, ciphertext)` → 明文返回上层
//! 4. **禁用 TDE** — `TdeEngine::disable()` 清除主密钥，之后读写不加密
//!
//! ## 加密算法
//!
//! - **AES-256-CTR** — 流式加密，无需填充，密文长度 == 明文长度
//! - **IV 派生** — `SHA-256(master_key || page_id_le_bytes)` 取前 16 字节，每页 IV 唯一
//! - **CTR 计数器** — IV 作为初始计数器，每 16 字节块后大端递增
//!
//! ## 密文格式
//!
//! 每个加密页的密文格式：
//! ```text
//! +-----------+-------------------+
//! | magic (4) | encrypted payload |
//! +-----------+-------------------+
//! | "TDE1"    | AES-256-CTR(data) |
//! +-----------+-------------------+
//! ```
//!
//! # 验证标准
//!
//! - 启用 TDE → 写入 1000000 行 → 直接 hexdump 数据文件 → 数据加密不可读
//! - SzRSQL 正常读取 → 数据正确
//!
//! 对应 `SzRSQL实施进度.md` Phase 7c.1。

use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes256;
use cipher::generic_array::GenericArray;
use sha2::{Digest, Sha256};

// =====================================================================
//  常量
// =====================================================================

/// TDE 密文魔术字（标识这是一个 TDE 加密的页）
const TDE_MAGIC: &[u8; 4] = b"TDE1";

/// AES-256 密钥长度（字节）
pub const AES_256_KEY_LEN: usize = 32;

/// AES 块大小（字节）
const AES_BLOCK_SIZE: usize = 16;

/// IV 长度（字节）
const IV_LEN: usize = 16;

/// 密钥版本字节长度（1 字节，支持 0-255 次轮换）
const KEY_VERSION_LEN: usize = 1;

/// 密文头部长度：magic(4) + key_version(1) = 5 字节
const HEADER_LEN: usize = TDE_MAGIC.len() + KEY_VERSION_LEN;

// =====================================================================
//  错误类型
// =====================================================================

/// TDE 错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TdeError {
    /// TDE 未启用
    #[error("TDE is not enabled")]
    NotEnabled,
    /// TDE 已启用
    #[error("TDE is already enabled")]
    AlreadyEnabled,
    /// 密钥长度无效
    #[error("invalid key length: expected {expected}, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
    /// 密文太短（不含头部）
    #[error("ciphertext too short: got {got} bytes, minimum {min}")]
    CiphertextTooShort { got: usize, min: usize },
    /// 密文 magic 不匹配
    #[error("invalid ciphertext magic: expected {expected:?}, got {got:?}")]
    InvalidMagic { expected: [u8; 4], got: [u8; 4] },
    /// 密钥版本未知（轮换已结束，旧版本密钥已清除）
    #[error("unknown key version: {0}, rotation already finalized")]
    UnknownKeyVersion(u8),
    /// 密钥版本溢出（超过 255 次轮换）
    #[error("key version overflow: too many rotations")]
    KeyVersionOverflow,
    /// 轮换未激活
    #[error("no active key rotation")]
    NoActiveRotation,
}

// =====================================================================
//  MasterKey — 主密钥
// =====================================================================

/// TDE 主密钥（32 字节 AES-256）
#[derive(Clone)]
pub struct MasterKey {
    /// 密钥字节（32 字节）
    bytes: [u8; AES_256_KEY_LEN],
}

impl MasterKey {
    /// 从字节切片创建主密钥（必须 32 字节）
    pub fn from_bytes(key: &[u8]) -> Result<Self, TdeError> {
        if key.len() != AES_256_KEY_LEN {
            return Err(TdeError::InvalidKeyLength {
                expected: AES_256_KEY_LEN,
                actual: key.len(),
            });
        }
        let mut bytes = [0u8; AES_256_KEY_LEN];
        bytes.copy_from_slice(key);
        Ok(Self { bytes })
    }

    /// 从密码短语派生主密钥（PBKDF2 风格：SHA-256 多轮迭代）
    ///
    /// 注意：这是简化版 KDF，生产环境应使用 PBKDF2/Argon2。
    pub fn from_passphrase(passphrase: &str, salt: &[u8], iterations: u32) -> Self {
        let mut hash = [0u8; AES_256_KEY_LEN];

        // 初始哈希：passphrase || salt
        let mut hasher = Sha256::new();
        hasher.update(passphrase.as_bytes());
        hasher.update(salt);
        let mut current = hasher.finalize();

        // 多轮迭代
        for _ in 0..iterations {
            let mut h = Sha256::new();
            h.update(current);
            current = h.finalize();
        }

        hash.copy_from_slice(&current);
        Self { bytes: hash }
    }

    /// 生成随机主密钥
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; AES_256_KEY_LEN];
        rand::rng().fill_bytes(&mut bytes);
        Self { bytes }
    }

    /// 获取密钥字节引用
    pub fn as_bytes(&self) -> &[u8; AES_256_KEY_LEN] {
        &self.bytes
    }

    /// 密钥指纹（SHA-256 前 8 字节的十六进制表示，用于标识密钥）
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.bytes);
        let digest = hasher.finalize();
        digest[..8].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKey")
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

// =====================================================================
//  AES-256-CTR 加密原语
// =====================================================================

/// 大端递增 16 字节计数器
fn increment_counter(counter: &mut [u8; AES_BLOCK_SIZE]) {
    // 大端递增：从最后一个字节开始进位
    for i in (0..AES_BLOCK_SIZE).rev() {
        counter[i] = counter[i].wrapping_add(1);
        if counter[i] != 0 {
            break;
        }
    }
}

/// 从主密钥 + page_id 派生 IV
///
/// IV = SHA-256(master_key || page_id_le_bytes)[..16]
fn derive_iv(master_key: &[u8; AES_256_KEY_LEN], page_id: u64) -> [u8; IV_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(master_key);
    hasher.update(page_id.to_le_bytes());
    let digest = hasher.finalize();
    let mut iv = [0u8; IV_LEN];
    iv.copy_from_slice(&digest[..IV_LEN]);
    iv
}

/// AES-256-CTR 加密/解密（对称操作，XOR 流密码）
///
/// CTR 模式加密和解密是同一个操作：
/// - 加密：plaintext XOR keystream = ciphertext
/// - 解密：ciphertext XOR keystream = plaintext
fn aes_256_ctr_process(key: &[u8; AES_256_KEY_LEN], iv: &[u8; IV_LEN], data: &[u8]) -> Vec<u8> {
    let cipher = Aes256::new(GenericArray::from_slice(key));
    let mut counter = *iv;
    let mut output = Vec::with_capacity(data.len());

    let mut keystream_block = [0u8; AES_BLOCK_SIZE];

    for chunk in data.chunks(AES_BLOCK_SIZE) {
        // 加密计数器块生成密钥流
        keystream_block.copy_from_slice(&counter);
        let mut block = GenericArray::clone_from_slice(&keystream_block);
        cipher.encrypt_block(&mut block);
        keystream_block.copy_from_slice(&block);

        // XOR 明文/密文与密钥流
        for (d, k) in chunk.iter().zip(keystream_block.iter()) {
            output.push(d ^ k);
        }

        // 递增计数器
        increment_counter(&mut counter);
    }

    output
}

// =====================================================================
//  TdeEngine — TDE 引擎
// =====================================================================

/// TDE 引擎 — 透明数据加密
///
/// # 工作流程
///
/// 1. `enable(key)` — 启用 TDE
/// 2. `encrypt_page(page_id, plaintext)` — 加密数据页（写盘前调用）
/// 3. `decrypt_page(page_id, ciphertext)` — 解密数据页（读盘后调用）
/// 4. `disable()` — 禁用 TDE
/// 5. `rotate_key_lazy(new_key)` — 密钥轮换（Phase 7c.2，保留旧密钥供过渡期解密）
/// 6. `reencrypt_page(page_id, ciphertext)` — 重加密旧页（读取时逐步重加密）
/// 7. `finalize_rotation()` — 完成轮换（清除旧密钥）
///
/// # 密文格式
///
/// ```text
/// +-----------+--------------+-------------------+
/// | magic (4) | key_ver (1)  | encrypted payload |
/// +-----------+--------------+-------------------+
/// | "TDE1"    | version byte | AES-256-CTR(data) |
/// +-----------+--------------+-------------------+
/// ```
#[derive(Debug, Default)]
pub struct TdeEngine {
    /// 当前主密钥（None = TDE 未启用）
    master_key: Option<MasterKey>,
    /// 当前密钥版本号（0 = 初始密钥，每次轮换 +1）
    key_version: u8,
    /// 旧密钥列表（轮换过渡期保留，供解密旧密文用）
    /// 元素：(版本号, 密钥)
    old_keys: Vec<(u8, MasterKey)>,
    /// 统计信息
    stats: TdeStats,
}

/// TDE 统计信息
#[derive(Debug, Clone, Default)]
pub struct TdeStats {
    /// 加密页次数
    pub pages_encrypted: u64,
    /// 解密页次数
    pub pages_decrypted: u64,
    /// 加密字节数
    pub bytes_encrypted: u64,
    /// 解密字节数
    pub bytes_decrypted: u64,
    /// 密钥轮换次数
    pub key_rotations: u64,
}

impl TdeEngine {
    /// 创建 TDE 引擎（默认未启用）
    pub fn new() -> Self {
        Self::default()
    }

    /// 启用 TDE
    ///
    /// - `key` — 32 字节主密钥
    ///
    /// # 错误
    ///
    /// - `AlreadyEnabled` — TDE 已启用
    /// - `InvalidKeyLength` — 密钥长度不是 32 字节
    pub fn enable(&mut self, key: &[u8]) -> Result<(), TdeError> {
        if self.master_key.is_some() {
            return Err(TdeError::AlreadyEnabled);
        }
        let master_key = MasterKey::from_bytes(key)?;
        self.master_key = Some(master_key);
        Ok(())
    }

    /// 启用 TDE（使用 MasterKey）
    pub fn enable_with_key(&mut self, key: MasterKey) -> Result<(), TdeError> {
        if self.master_key.is_some() {
            return Err(TdeError::AlreadyEnabled);
        }
        self.master_key = Some(key);
        Ok(())
    }

    /// 禁用 TDE（清除主密钥 + 旧密钥 + 重置版本号）
    ///
    /// # 错误
    ///
    /// - `NotEnabled` — TDE 未启用
    pub fn disable(&mut self) -> Result<(), TdeError> {
        if self.master_key.is_none() {
            return Err(TdeError::NotEnabled);
        }
        self.master_key = None;
        self.old_keys.clear();
        self.key_version = 0;
        Ok(())
    }

    /// TDE 是否启用
    pub fn is_enabled(&self) -> bool {
        self.master_key.is_some()
    }

    /// 获取主密钥指纹（用于标识当前密钥）
    pub fn key_fingerprint(&self) -> Option<String> {
        self.master_key.as_ref().map(|k| k.fingerprint())
    }

    /// 获取当前密钥版本号
    pub fn key_version(&self) -> u8 {
        self.key_version
    }

    /// 是否正在轮换过渡期（旧密钥仍保留）
    pub fn is_rotating(&self) -> bool {
        !self.old_keys.is_empty()
    }

    /// 获取统计信息
    pub fn stats(&self) -> &TdeStats {
        &self.stats
    }

    /// 重置统计信息
    pub fn reset_stats(&mut self) {
        self.stats = TdeStats::default();
    }

    /// 加密数据页
    ///
    /// - `page_id` — 页 ID（用于派生 IV）
    /// - `plaintext` — 明文页数据
    ///
    /// 返回密文（magic + key_version + 加密数据）
    ///
    /// # 错误
    ///
    /// - `NotEnabled` — TDE 未启用
    pub fn encrypt_page(&mut self, page_id: u64, plaintext: &[u8]) -> Result<Vec<u8>, TdeError> {
        let master_key = self.master_key.as_ref().ok_or(TdeError::NotEnabled)?;
        let iv = derive_iv(master_key.as_bytes(), page_id);
        let encrypted = aes_256_ctr_process(master_key.as_bytes(), &iv, plaintext);

        // 密文格式：magic + key_version + encrypted payload
        let mut ciphertext = Vec::with_capacity(HEADER_LEN + encrypted.len());
        ciphertext.extend_from_slice(TDE_MAGIC);
        ciphertext.push(self.key_version);
        ciphertext.extend_from_slice(&encrypted);

        self.stats.pages_encrypted += 1;
        self.stats.bytes_encrypted += plaintext.len() as u64;

        Ok(ciphertext)
    }

    /// 解密数据页
    ///
    /// - `page_id` — 页 ID（用于派生 IV）
    /// - `ciphertext` — 密文页数据（含 magic + key_version 头）
    ///
    /// 返回明文。轮换过渡期，旧版本密文自动用旧密钥解密。
    ///
    /// # 错误
    ///
    /// - `NotEnabled` — TDE 未启用
    /// - `CiphertextTooShort` — 密文太短
    /// - `InvalidMagic` — 密文 magic 不匹配
    /// - `UnknownKeyVersion` — 密钥版本未知（轮换已结束，旧密钥已清除）
    pub fn decrypt_page(&mut self, page_id: u64, ciphertext: &[u8]) -> Result<Vec<u8>, TdeError> {
        let _ = self.master_key.as_ref().ok_or(TdeError::NotEnabled)?;

        // 检查密文长度（至少包含 magic + key_version）
        if ciphertext.len() < HEADER_LEN {
            return Err(TdeError::CiphertextTooShort {
                got: ciphertext.len(),
                min: HEADER_LEN,
            });
        }

        // 检查 magic
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&ciphertext[..4]);
        if &magic != TDE_MAGIC {
            return Err(TdeError::InvalidMagic {
                expected: *TDE_MAGIC,
                got: magic,
            });
        }

        // 读取密钥版本
        let ct_key_version = ciphertext[4];
        let encrypted = &ciphertext[5..];

        // 选择解密密钥
        let decrypt_key = if ct_key_version == self.key_version {
            // 当前版本 → 用当前密钥
            self.master_key.as_ref().unwrap()
        } else {
            // 旧版本 → 在 old_keys 中查找
            self.old_keys
                .iter()
                .find(|(ver, _)| *ver == ct_key_version)
                .map(|(_, k)| k)
                .ok_or(TdeError::UnknownKeyVersion(ct_key_version))?
        };

        let iv = derive_iv(decrypt_key.as_bytes(), page_id);
        let plaintext = aes_256_ctr_process(decrypt_key.as_bytes(), &iv, encrypted);

        self.stats.pages_decrypted += 1;
        self.stats.bytes_decrypted += encrypted.len() as u64;

        Ok(plaintext)
    }

    /// 重加密旧版本密文为当前版本（读取时逐步重加密）
    ///
    /// - `page_id` — 页 ID
    /// - `ciphertext` — 旧版本密文
    ///
    /// 返回用当前密钥重新加密的密文。如果密文已是当前版本，原样返回。
    ///
    /// # 错误
    ///
    /// - `NotEnabled` — TDE 未启用
    /// - `UnknownKeyVersion` — 密钥版本未知
    pub fn reencrypt_page(&mut self, page_id: u64, ciphertext: &[u8]) -> Result<Vec<u8>, TdeError> {
        if ciphertext.len() < HEADER_LEN {
            return Err(TdeError::CiphertextTooShort {
                got: ciphertext.len(),
                min: HEADER_LEN,
            });
        }

        let ct_key_version = ciphertext[4];
        if ct_key_version == self.key_version {
            // 已是当前版本，无需重加密
            return Ok(ciphertext.to_vec());
        }

        // 解密旧密文
        let plaintext = self.decrypt_page(page_id, ciphertext)?;
        // 用当前密钥重新加密
        self.encrypt_page(page_id, &plaintext)
    }

    /// 加密任意字节（不附加 magic 头，用于非页数据）
    ///
    /// - `plaintext` — 明文
    ///
    /// 返回密文（长度 == 明文长度）
    pub fn encrypt_bytes(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, TdeError> {
        let master_key = self.master_key.as_ref().ok_or(TdeError::NotEnabled)?;
        // 使用 IV = 0（encrypt_bytes 用于通用加密，不绑定 page_id）
        let iv = [0u8; IV_LEN];
        let result = aes_256_ctr_process(master_key.as_bytes(), &iv, plaintext);
        self.stats.bytes_encrypted += plaintext.len() as u64;
        Ok(result)
    }

    /// 解密任意字节（对应 encrypt_bytes）
    pub fn decrypt_bytes(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, TdeError> {
        let master_key = self.master_key.as_ref().ok_or(TdeError::NotEnabled)?;
        let iv = [0u8; IV_LEN];
        let result = aes_256_ctr_process(master_key.as_bytes(), &iv, ciphertext);
        self.stats.bytes_decrypted += ciphertext.len() as u64;
        Ok(result)
    }

    // -----------------------------------------------------------------
    //  Phase 7c.2：密钥轮换
    // -----------------------------------------------------------------

    /// 密钥轮换（立即）— 用新密钥替换旧密钥，不保留旧密钥
    ///
    /// 注意：立即轮换后，旧版本密文将无法解密（因为旧密钥已清除）。
    /// 如需保留旧密钥供过渡期解密，请使用 `rotate_key_lazy`。
    ///
    /// 返回旧密钥指纹（用于审计）
    ///
    /// # 错误
    ///
    /// - `NotEnabled` — TDE 未启用
    /// - `KeyVersionOverflow` — 密钥版本溢出（超过 255 次轮换）
    pub fn rotate_key(&mut self, new_key: MasterKey) -> Result<String, TdeError> {
        let old = self.master_key.as_ref().ok_or(TdeError::NotEnabled)?;
        let old_fingerprint = old.fingerprint();

        self.key_version = self
            .key_version
            .checked_add(1)
            .ok_or(TdeError::KeyVersionOverflow)?;
        self.master_key = Some(new_key);
        self.old_keys.clear(); // 立即轮换不保留旧密钥
        self.stats.key_rotations += 1;
        Ok(old_fingerprint)
    }

    /// 密钥轮换（立即）— 用新密钥字节替换旧密钥
    pub fn rotate_key_bytes(&mut self, new_key: &[u8]) -> Result<String, TdeError> {
        let key = MasterKey::from_bytes(new_key)?;
        self.rotate_key(key)
    }

    /// 密钥轮换（延迟）— 保留旧密钥供过渡期解密旧密文
    ///
    /// 轮换后：
    /// - 新写入的页使用新密钥（key_version + 1）
    /// - 旧密文仍可用旧密钥解密
    /// - 读取旧密文时可调用 `reencrypt_page` 逐步重加密为新版本
    /// - 调用 `finalize_rotation` 清除旧密钥，完成轮换
    ///
    /// 返回旧密钥指纹（用于审计）
    ///
    /// # 错误
    ///
    /// - `NotEnabled` — TDE 未启用
    /// - `KeyVersionOverflow` — 密钥版本溢出
    pub fn rotate_key_lazy(&mut self, new_key: MasterKey) -> Result<String, TdeError> {
        let old = self.master_key.as_ref().ok_or(TdeError::NotEnabled)?;
        let old_fingerprint = old.fingerprint();

        // 先检查溢出，避免在失败时已 push 旧密钥（事务性）
        let new_version = self
            .key_version
            .checked_add(1)
            .ok_or(TdeError::KeyVersionOverflow)?;

        let old_version = self.key_version;
        // 保留旧密钥（克隆）供过渡期解密
        self.old_keys.push((old_version, old.clone()));

        self.key_version = new_version;
        self.master_key = Some(new_key);
        self.stats.key_rotations += 1;
        Ok(old_fingerprint)
    }

    /// 密钥轮换（延迟）— 用新密钥字节
    pub fn rotate_key_lazy_bytes(&mut self, new_key: &[u8]) -> Result<String, TdeError> {
        let key = MasterKey::from_bytes(new_key)?;
        self.rotate_key_lazy(key)
    }

    /// 完成轮换 — 清除所有旧密钥
    ///
    /// 调用后，旧版本密文将无法解密（返回 `UnknownKeyVersion` 错误）。
    /// 应在所有旧密文重加密为新版本后调用。
    ///
    /// # 错误
    ///
    /// - `NoActiveRotation` — 没有正在进行的轮换
    pub fn finalize_rotation(&mut self) -> Result<(), TdeError> {
        if self.old_keys.is_empty() {
            return Err(TdeError::NoActiveRotation);
        }
        self.old_keys.clear();
        Ok(())
    }

    /// 获取旧密钥版本列表（用于追踪哪些版本仍在过渡期）
    pub fn old_key_versions(&self) -> Vec<u8> {
        self.old_keys.iter().map(|(v, _)| *v).collect()
    }
}

// =====================================================================
//  TdePageEncryptor — WalWriter FPI 加密路径集成（P2-17）
// =====================================================================

/// TDE 页加密器：将 `TdeEngine` 适配为 `szrsql_tx::wal::PageEncryptor` trait。
///
/// 用于在 `WalWriter::append` 路径中对 Full Page Image（FPI）记录的页数据
/// 进行透明加密，实现 TDE 写入路径的完整性。
///
/// # 线程安全
///
/// `TdeEngine::encrypt_page` 需要 `&mut self`，本类型通过内部 `Mutex`
/// 实现 `Sync`，可安全地在多线程 WAL 写入场景下共享（`Arc<TdePageEncryptor>`）。
///
/// # 用法
///
/// ```ignore
/// use szrsql_security::tde::{TdeEngine, TdePageEncryptor};
/// let mut tde = TdeEngine::new();
/// tde.enable(&master_key).unwrap();
/// let encryptor = Arc::new(TdePageEncryptor::new(tde));
/// let writer = WalWriter::open(path)?
///     .with_encryptor(encryptor);
/// ```
#[derive(Debug)]
pub struct TdePageEncryptor {
    inner: std::sync::Mutex<TdeEngine>,
}

impl TdePageEncryptor {
    /// 从已启用 TDE 的 `TdeEngine` 创建加密器。
    pub fn new(engine: TdeEngine) -> Self {
        Self {
            inner: std::sync::Mutex::new(engine),
        }
    }

    /// 获取底层 `TdeEngine` 的不可变引用（用于查询状态/统计）。
    pub fn engine(
        &self,
    ) -> Result<
        std::sync::MutexGuard<'_, TdeEngine>,
        std::sync::PoisonError<std::sync::MutexGuard<'_, TdeEngine>>,
    > {
        self.inner.lock()
    }
}

impl szrsql_tx::wal::PageEncryptor for TdePageEncryptor {
    fn encrypt(&self, page_id: u32, plaintext: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut engine = self
            .inner
            .lock()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        engine
            .encrypt_page(page_id as u64, plaintext)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 生成随机 32 字节主密钥
pub fn generate_master_key() -> MasterKey {
    MasterKey::generate()
}

/// 从密码短语派生主密钥
pub fn derive_master_key_from_passphrase(passphrase: &str) -> MasterKey {
    MasterKey::from_passphrase(passphrase, b"szrsql_tde_salt", 10_000)
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;

    // -----------------------------------------------------------------
    //  MasterKey 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_master_key_from_bytes() {
        let key_bytes = [42u8; AES_256_KEY_LEN];
        let key = MasterKey::from_bytes(&key_bytes).unwrap();
        assert_eq!(key.as_bytes(), &key_bytes);
    }

    #[test]
    fn test_7c1_master_key_invalid_length() {
        let short_key = [0u8; 16];
        let result = MasterKey::from_bytes(&short_key);
        assert!(result.is_err());
        match result.unwrap_err() {
            TdeError::InvalidKeyLength { expected, actual } => {
                assert_eq!(expected, AES_256_KEY_LEN);
                assert_eq!(actual, 16);
            }
            _ => panic!("expected InvalidKeyLength"),
        }
    }

    #[test]
    fn test_7c1_master_key_from_passphrase() {
        let key1 = MasterKey::from_passphrase("my_password", b"salt", 1000);
        let key2 = MasterKey::from_passphrase("my_password", b"salt", 1000);
        let key3 = MasterKey::from_passphrase("wrong_password", b"salt", 1000);

        // 相同密码 + 相同盐 → 相同密钥
        assert_eq!(key1.as_bytes(), key2.as_bytes());
        // 不同密码 → 不同密钥
        assert_ne!(key1.as_bytes(), key3.as_bytes());
    }

    #[test]
    fn test_7c1_master_key_generate() {
        let key1 = MasterKey::generate();
        let key2 = MasterKey::generate();
        // 随机生成的密钥应不同（概率上）
        assert_ne!(key1.as_bytes(), key2.as_bytes());
    }

    #[test]
    fn test_7c1_master_key_fingerprint() {
        let key = MasterKey::from_bytes(&[0u8; AES_256_KEY_LEN]).unwrap();
        let fp = key.fingerprint();
        // 指纹是 16 个十六进制字符（8 字节）
        assert_eq!(fp.len(), 16);
        // 相同密钥 → 相同指纹
        let key2 = MasterKey::from_bytes(&[0u8; AES_256_KEY_LEN]).unwrap();
        assert_eq!(fp, key2.fingerprint());
    }

    #[test]
    fn test_7c1_master_key_debug_no_leak() {
        let key = MasterKey::from_bytes(&[0xABu8; AES_256_KEY_LEN]).unwrap();
        let debug_str = format!("{key:?}");
        // Debug 输出不应泄露密钥内容
        assert!(!debug_str.contains("AB"));
        assert!(debug_str.contains("fingerprint"));
    }

    // -----------------------------------------------------------------
    //  TdeEngine 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_engine_creation() {
        let engine = TdeEngine::new();
        assert!(!engine.is_enabled());
        assert!(engine.key_fingerprint().is_none());
    }

    #[test]
    fn test_7c1_enable_disable() {
        let mut engine = TdeEngine::new();
        let key = [1u8; AES_256_KEY_LEN];

        engine.enable(&key).unwrap();
        assert!(engine.is_enabled());
        assert!(engine.key_fingerprint().is_some());

        engine.disable().unwrap();
        assert!(!engine.is_enabled());
    }

    #[test]
    fn test_7c1_enable_already_enabled() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();
        let result = engine.enable(&[2u8; AES_256_KEY_LEN]);
        assert_eq!(result.unwrap_err(), TdeError::AlreadyEnabled);
    }

    #[test]
    fn test_7c1_disable_not_enabled() {
        let mut engine = TdeEngine::new();
        let result = engine.disable();
        assert_eq!(result.unwrap_err(), TdeError::NotEnabled);
    }

    #[test]
    fn test_7c1_enable_invalid_key_length() {
        let mut engine = TdeEngine::new();
        let result = engine.enable(&[0u8; 16]);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    //  加密/解密测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_encrypt_decrypt_page_roundtrip() {
        let mut engine = TdeEngine::new();
        engine.enable(&[42u8; AES_256_KEY_LEN]).unwrap();

        let page_id: u64 = 100;
        let plaintext = b"Hello, TDE World! This is a test page.".to_vec();

        let ciphertext = engine.encrypt_page(page_id, &plaintext).unwrap();
        assert_ne!(&ciphertext[4..], &plaintext[..]); // 密文 != 明文

        let decrypted = engine.decrypt_page(page_id, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_7c1_encrypt_page_not_enabled() {
        let mut engine = TdeEngine::new();
        let result = engine.encrypt_page(0, b"data");
        assert_eq!(result.unwrap_err(), TdeError::NotEnabled);
    }

    #[test]
    fn test_7c1_decrypt_page_not_enabled() {
        let mut engine = TdeEngine::new();
        let result = engine.decrypt_page(0, b"TDE1data");
        assert_eq!(result.unwrap_err(), TdeError::NotEnabled);
    }

    #[test]
    fn test_7c1_decrypt_ciphertext_too_short() {
        let mut engine = TdeEngine::new();
        engine.enable(&[42u8; AES_256_KEY_LEN]).unwrap();

        let result = engine.decrypt_page(0, b"abc");
        assert!(matches!(
            result.unwrap_err(),
            TdeError::CiphertextTooShort { .. }
        ));
    }

    #[test]
    fn test_7c1_decrypt_invalid_magic() {
        let mut engine = TdeEngine::new();
        engine.enable(&[42u8; AES_256_KEY_LEN]).unwrap();

        // 5 字节，magic 不匹配
        let bad_data = b"XXXXx";
        let result = engine.decrypt_page(0, bad_data);
        assert!(matches!(result.unwrap_err(), TdeError::InvalidMagic { .. }));
    }

    #[test]
    fn test_7c1_encrypt_decrypt_bytes_roundtrip() {
        let mut engine = TdeEngine::new();
        engine.enable(&[7u8; AES_256_KEY_LEN]).unwrap();

        let data = b"Sensitive data that needs encryption".to_vec();
        let encrypted = engine.encrypt_bytes(&data).unwrap();
        let decrypted = engine.decrypt_bytes(&encrypted).unwrap();

        assert_eq!(decrypted, data);
        assert_eq!(encrypted.len(), data.len()); // CTR 模式密文长度 == 明文长度
    }

    // -----------------------------------------------------------------
    //  加密特性测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_ciphertext_differs_from_plaintext() {
        let mut engine = TdeEngine::new();
        engine.enable(&[99u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = vec![0xAAu8; 256];
        let ciphertext = engine.encrypt_page(1, &plaintext).unwrap();

        // 密文内容（去掉 magic 头后）应与明文不同
        let ct_payload = &ciphertext[4..];
        let diff_count = ct_payload
            .iter()
            .zip(plaintext.iter())
            .filter(|(c, p)| c != p)
            .count();
        // 绝大多数字节应不同（CTR 模式下几乎全部翻转）
        assert!(
            diff_count > 250,
            "expected >250 differing bytes, got {diff_count}"
        );
    }

    #[test]
    fn test_7c1_different_pages_different_ciphertext() {
        let mut engine = TdeEngine::new();
        engine.enable(&[55u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = b"same plaintext for different pages".to_vec();
        let ct1 = engine.encrypt_page(1, &plaintext).unwrap();
        let ct2 = engine.encrypt_page(2, &plaintext).unwrap();

        // 相同明文 + 不同 page_id → 不同密文（因为 IV 不同）
        assert_ne!(ct1, ct2);

        // 但都能正确解密
        assert_eq!(engine.decrypt_page(1, &ct1).unwrap(), plaintext);
        assert_eq!(engine.decrypt_page(2, &ct2).unwrap(), plaintext);
    }

    #[test]
    fn test_7c1_same_page_same_ciphertext() {
        let mut engine = TdeEngine::new();
        engine.enable(&[55u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = b"deterministic encryption".to_vec();
        let ct1 = engine.encrypt_page(5, &plaintext).unwrap();
        let ct2 = engine.encrypt_page(5, &plaintext).unwrap();

        // 相同明文 + 相同 page_id → 相同密文（IV 确定性派生）
        assert_eq!(ct1, ct2);
    }

    #[test]
    fn test_7c1_different_keys_different_ciphertext() {
        let mut engine1 = TdeEngine::new();
        engine1.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let mut engine2 = TdeEngine::new();
        engine2.enable(&[2u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = b"same data, different keys".to_vec();
        let ct1 = engine1.encrypt_page(0, &plaintext).unwrap();
        let ct2 = engine2.encrypt_page(0, &plaintext).unwrap();

        // 不同密钥 → 不同密文
        assert_ne!(ct1, ct2);
    }

    #[test]
    fn test_7c1_decrypt_with_wrong_key_fails() {
        let mut engine1 = TdeEngine::new();
        engine1.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let mut engine2 = TdeEngine::new();
        engine2.enable(&[2u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = b"secret data".to_vec();
        let ciphertext = engine1.encrypt_page(0, &plaintext).unwrap();

        // 用错误密钥解密 → magic 通过但数据错误
        let decrypted = engine2.decrypt_page(0, &ciphertext).unwrap();
        assert_ne!(decrypted, plaintext); // 解密结果不等于原始明文
    }

    // -----------------------------------------------------------------
    //  统计测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_stats_tracking() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let data = vec![0u8; 100];

        engine.encrypt_page(0, &data).unwrap();
        engine.encrypt_page(1, &data).unwrap();

        let ciphertext = engine.encrypt_page(2, &data).unwrap();
        engine.decrypt_page(2, &ciphertext).unwrap();

        let stats = engine.stats();
        assert_eq!(stats.pages_encrypted, 3);
        assert_eq!(stats.pages_decrypted, 1);
        assert_eq!(stats.bytes_encrypted, 300);
        assert_eq!(stats.bytes_decrypted, 100);
    }

    #[test]
    fn test_7c1_reset_stats() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        engine.encrypt_page(0, b"data").unwrap();
        assert!(engine.stats().pages_encrypted > 0);

        engine.reset_stats();
        assert_eq!(engine.stats().pages_encrypted, 0);
    }

    // -----------------------------------------------------------------
    //  密钥轮换测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_rotate_key() {
        let mut engine = TdeEngine::new();
        let key1 = MasterKey::from_bytes(&[1u8; AES_256_KEY_LEN]).unwrap();
        let key2 = MasterKey::from_bytes(&[2u8; AES_256_KEY_LEN]).unwrap();

        engine.enable_with_key(key1).unwrap();
        let fp1 = engine.key_fingerprint().unwrap();

        // 用 key1 加密（key_version = 0）
        let plaintext = b"data encrypted with key1".to_vec();
        let ciphertext = engine.encrypt_page(0, &plaintext).unwrap();
        assert_eq!(ciphertext[4], 0); // key_version = 0

        // 立即轮换到 key2（key_version = 1，旧密钥已清除）
        let old_fp = engine.rotate_key(key2).unwrap();
        assert_eq!(old_fp, fp1);
        assert_eq!(engine.stats().key_rotations, 1);
        assert_eq!(engine.key_version(), 1);

        // 立即轮换后旧密文无法解密（旧密钥已清除，key_version 不匹配）
        let result = engine.decrypt_page(0, &ciphertext);
        assert_eq!(result.unwrap_err(), TdeError::UnknownKeyVersion(0));

        // 用 key2 加密新数据（key_version = 1）→ key2 可正确解密
        let plaintext2 = b"data encrypted with key2".to_vec();
        let ciphertext2 = engine.encrypt_page(0, &plaintext2).unwrap();
        assert_eq!(ciphertext2[4], 1); // key_version = 1
        let decrypted2 = engine.decrypt_page(0, &ciphertext2).unwrap();
        assert_eq!(decrypted2, plaintext2);
    }

    #[test]
    fn test_7c1_rotate_key_not_enabled() {
        let mut engine = TdeEngine::new();
        let key = MasterKey::generate();
        let result = engine.rotate_key(key);
        assert_eq!(result.unwrap_err(), TdeError::NotEnabled);
    }

    #[test]
    fn test_7c1_rotate_key_bytes() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let old_fp = engine.rotate_key_bytes(&[2u8; AES_256_KEY_LEN]).unwrap();
        assert!(!old_fp.is_empty());
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_generate_master_key() {
        let key = generate_master_key();
        assert_eq!(key.as_bytes().len(), AES_256_KEY_LEN);
    }

    #[test]
    fn test_7c1_derive_master_key_from_passphrase() {
        let key = derive_master_key_from_passphrase("my_secret");
        assert_eq!(key.as_bytes().len(), AES_256_KEY_LEN);

        // 相同密码 → 相同密钥
        let key2 = derive_master_key_from_passphrase("my_secret");
        assert_eq!(key.as_bytes(), key2.as_bytes());
    }

    // -----------------------------------------------------------------
    //  CTR 计数器测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_increment_counter() {
        let mut counter = [0u8; AES_BLOCK_SIZE];
        increment_counter(&mut counter);
        assert_eq!(counter[15], 1);

        // 进位测试
        counter = [
            0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0u8, 0xFF,
        ];
        increment_counter(&mut counter);
        assert_eq!(counter[15], 0);
        assert_eq!(counter[14], 1);
    }

    #[test]
    fn test_7c1_ctr_large_data() {
        let mut engine = TdeEngine::new();
        engine.enable(&[42u8; AES_256_KEY_LEN]).unwrap();

        // 1MB 数据（超过单个 AES 块的 65536 倍）
        let plaintext: Vec<u8> = (0..1_048_576).map(|i| (i % 256) as u8).collect();
        let ciphertext = engine.encrypt_page(0, &plaintext).unwrap();
        let decrypted = engine.decrypt_page(0, &ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    // -----------------------------------------------------------------
    //  完整工作流测试（模拟 1000000 行场景）
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_full_workflow_large_scale() {
        let mut engine = TdeEngine::new();
        let key = generate_master_key();
        engine.enable_with_key(key).unwrap();

        // 模拟 1000 页 × 1000 行/页 = 1,000,000 行
        // 每行 100 字节，每页 100,000 字节
        let page_count = 1000;
        let rows_per_page = 1000;
        let row_size = 100;

        let mut original_pages: Vec<Vec<u8>> = Vec::with_capacity(page_count);
        let mut encrypted_pages: Vec<Vec<u8>> = Vec::with_capacity(page_count);

        for page_id in 0..page_count as u64 {
            // 构造页数据：rows_per_page × row_size 字节
            let plaintext: Vec<u8> = (0..rows_per_page)
                .flat_map(|row| {
                    let offset = row * row_size;
                    (0..row_size).map(move |j| ((offset + j) % 256) as u8)
                })
                .collect();

            let ciphertext = engine.encrypt_page(page_id, &plaintext).unwrap();
            original_pages.push(plaintext);
            encrypted_pages.push(ciphertext);
        }

        // 验证：密文 != 明文（加密不可读）
        for (page_id, (orig, enc)) in original_pages
            .iter()
            .zip(encrypted_pages.iter())
            .enumerate()
        {
            assert_ne!(
                &enc[4..],
                &orig[..],
                "page {page_id} ciphertext should differ from plaintext"
            );
        }

        // 验证：解密后数据正确（透明读取）
        for (page_id, (orig, enc)) in original_pages
            .iter()
            .zip(encrypted_pages.iter())
            .enumerate()
        {
            let decrypted = engine.decrypt_page(page_id as u64, enc).unwrap();
            assert_eq!(&decrypted, orig, "page {page_id} decryption mismatch");
        }

        // 验证统计
        let stats = engine.stats();
        assert_eq!(stats.pages_encrypted, page_count as u64);
        assert_eq!(stats.pages_decrypted, page_count as u64);
    }

    #[test]
    fn test_7c1_encrypted_data_unreadable_hexdump() {
        // 验证：hexdump 加密数据 → 不可读
        let mut engine = TdeEngine::new();
        engine.enable(&[77u8; AES_256_KEY_LEN]).unwrap();

        let plaintext =
            b"INSERT INTO users VALUES ('admin', 'password123', 'secret@email.com')".to_vec();
        let ciphertext = engine.encrypt_page(0, &plaintext).unwrap();

        // 密文中不应包含明文的 ASCII 子串
        let ct_str = String::from_utf8_lossy(&ciphertext[4..]);
        assert!(!ct_str.contains("admin"));
        assert!(!ct_str.contains("password"));
        assert!(!ct_str.contains("email"));

        // 但解密后应包含
        let decrypted = engine.decrypt_page(0, &ciphertext).unwrap();
        let dec_str = String::from_utf8_lossy(&decrypted);
        assert!(dec_str.contains("admin"));
        assert!(dec_str.contains("password123"));
    }

    // -----------------------------------------------------------------
    //  边界测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_empty_page() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let plaintext: Vec<u8> = vec![];
        let ciphertext = engine.encrypt_page(0, &plaintext).unwrap();

        // 空明文 → 密文只有头部（magic + key_version = 5 字节）
        assert_eq!(ciphertext.len(), HEADER_LEN);
        assert_eq!(&ciphertext[..4], TDE_MAGIC);
        assert_eq!(ciphertext[4], 0); // key_version = 0

        let decrypted = engine.decrypt_page(0, &ciphertext).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_7c1_single_byte_page() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = vec![42u8];
        let ciphertext = engine.encrypt_page(0, &plaintext).unwrap();
        let decrypted = engine.decrypt_page(0, &ciphertext).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_7c1_block_aligned_page() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        // 恰好 16 字节（1 个 AES 块）
        let plaintext = vec![0xABu8; AES_BLOCK_SIZE];
        let ciphertext = engine.encrypt_page(0, &plaintext).unwrap();
        let decrypted = engine.decrypt_page(0, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_7c1_non_block_aligned_page() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        // 23 字节（1 个完整块 + 7 字节部分块）
        let plaintext: Vec<u8> = (0..23u8).collect();
        let ciphertext = engine.encrypt_page(0, &plaintext).unwrap();
        let decrypted = engine.decrypt_page(0, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_7c1_max_page_id() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = b"max page id test".to_vec();
        let ciphertext = engine.encrypt_page(u64::MAX, &plaintext).unwrap();
        let decrypted = engine.decrypt_page(u64::MAX, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    // -----------------------------------------------------------------
    //  enable_with_key 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7c1_enable_with_master_key() {
        let mut engine = TdeEngine::new();
        let key = MasterKey::generate();
        let fp = key.fingerprint();

        engine.enable_with_key(key).unwrap();
        assert!(engine.is_enabled());
        assert_eq!(engine.key_fingerprint(), Some(fp));
    }

    #[test]
    fn test_7c1_enable_with_key_already_enabled() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let key = MasterKey::generate();
        let result = engine.enable_with_key(key);
        assert_eq!(result.unwrap_err(), TdeError::AlreadyEnabled);
    }

    // =================================================================
    //  Phase 7c.2：密钥轮换 + 逐步重加密测试
    // =================================================================

    // -----------------------------------------------------------------
    //  rotate_key_lazy 延迟轮换基础流程
    // -----------------------------------------------------------------

    #[test]
    fn test_7c2_rotate_key_lazy_basic() {
        // 验证标准：ALTER SYSTEM ROTATE ENCRYPTION KEY →
        //          新数据用新密钥 → 旧数据在读取时逐步重加密 →
        //          所有数据最终用新密钥
        let mut engine = TdeEngine::new();
        let key1 = MasterKey::from_bytes(&[1u8; AES_256_KEY_LEN]).unwrap();
        let key2 = MasterKey::from_bytes(&[2u8; AES_256_KEY_LEN]).unwrap();

        engine.enable_with_key(key1).unwrap();
        assert_eq!(engine.key_version(), 0);
        assert!(!engine.is_rotating());

        // 用 key1 加密页 0（key_version = 0）
        let plaintext1 = b"old data encrypted with key1".to_vec();
        let ciphertext1 = engine.encrypt_page(0, &plaintext1).unwrap();
        assert_eq!(ciphertext1[4], 0); // key_version = 0

        // 延迟轮换到 key2
        let old_fp = engine.rotate_key_lazy(key2).unwrap();
        assert_eq!(engine.key_version(), 1);
        assert!(engine.is_rotating()); // 正在轮换（旧密钥保留）
        assert!(!old_fp.is_empty());
        assert_eq!(engine.stats().key_rotations, 1);

        // 旧密文仍可解密（用旧密钥 key1）
        let decrypted_old = engine.decrypt_page(0, &ciphertext1).unwrap();
        assert_eq!(decrypted_old, plaintext1);

        // 新数据用 key2 加密（key_version = 1）
        let plaintext2 = b"new data encrypted with key2".to_vec();
        let ciphertext2 = engine.encrypt_page(1, &plaintext2).unwrap();
        assert_eq!(ciphertext2[4], 1); // key_version = 1
        let decrypted2 = engine.decrypt_page(1, &ciphertext2).unwrap();
        assert_eq!(decrypted2, plaintext2);

        // 逐步重加密：旧密文 → 新密文
        let reencrypted = engine.reencrypt_page(0, &ciphertext1).unwrap();
        assert_eq!(reencrypted[4], 1); // 重加密后 key_version = 1
        assert_ne!(reencrypted, ciphertext1); // 密文已变化

        // 重加密后的密文可用当前密钥（key2）解密
        let decrypted_re = engine.decrypt_page(0, &reencrypted).unwrap();
        assert_eq!(decrypted_re, plaintext1);

        // 完成轮换 — 清除旧密钥
        engine.finalize_rotation().unwrap();
        assert!(!engine.is_rotating());

        // 完成轮换后，旧密文（key_version=0）无法解密
        let result = engine.decrypt_page(0, &ciphertext1);
        assert_eq!(result.unwrap_err(), TdeError::UnknownKeyVersion(0));

        // 但重加密后的新密文（key_version=1）仍可解密
        let decrypted_final = engine.decrypt_page(0, &reencrypted).unwrap();
        assert_eq!(decrypted_final, plaintext1);
    }

    #[test]
    fn test_7c2_rotate_key_lazy_old_ciphertext_still_decryptable() {
        // 验证：延迟轮换后，旧密文仍可正确解密（过渡期）
        let mut engine = TdeEngine::new();
        engine.enable(&[10u8; AES_256_KEY_LEN]).unwrap();

        // 加密多个页
        let pages: Vec<(u64, Vec<u8>)> = (0..5u64)
            .map(|id| (id, format!("page {id} data").into_bytes()))
            .collect();

        let ciphertexts: Vec<(u64, Vec<u8>)> = pages
            .iter()
            .map(|(id, plaintext)| (*id, engine.encrypt_page(*id, plaintext).unwrap()))
            .collect();

        // 延迟轮换
        engine
            .rotate_key_lazy_bytes(&[20u8; AES_256_KEY_LEN])
            .unwrap();

        // 所有旧密文仍可正确解密
        for ((id, plaintext), (_, ciphertext)) in pages.iter().zip(ciphertexts.iter()) {
            let decrypted = engine.decrypt_page(*id, ciphertext).unwrap();
            assert_eq!(&decrypted, plaintext);
        }
    }

    // -----------------------------------------------------------------
    //  reencrypt_page 逐步重加密
    // -----------------------------------------------------------------

    #[test]
    fn test_7c2_reencrypt_page() {
        // 验证：读取旧密文时用当前密钥重新加密
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = b"sensitive data to reencrypt".to_vec();
        let old_ciphertext = engine.encrypt_page(42, &plaintext).unwrap();
        assert_eq!(old_ciphertext[4], 0);

        // 延迟轮换
        engine
            .rotate_key_lazy_bytes(&[2u8; AES_256_KEY_LEN])
            .unwrap();

        // 重加密旧密文
        let new_ciphertext = engine.reencrypt_page(42, &old_ciphertext).unwrap();
        assert_eq!(new_ciphertext[4], 1); // 新 key_version

        // 新密文与旧密文不同
        assert_ne!(new_ciphertext, old_ciphertext);

        // 新密文可正确解密
        let decrypted = engine.decrypt_page(42, &new_ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);

        // 完成轮换后，旧密文不可解密，但新密文仍可解密
        engine.finalize_rotation().unwrap();
        assert_eq!(
            engine.decrypt_page(42, &old_ciphertext).unwrap_err(),
            TdeError::UnknownKeyVersion(0)
        );
        let decrypted2 = engine.decrypt_page(42, &new_ciphertext).unwrap();
        assert_eq!(decrypted2, plaintext);
    }

    #[test]
    fn test_7c2_reencrypt_already_current_version() {
        // 验证：reencrypt_page 对当前版本密文原样返回
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = b"current version data".to_vec();
        let ciphertext = engine.encrypt_page(0, &plaintext).unwrap();

        // 未轮换，密文已是当前版本 → 原样返回
        let result = engine.reencrypt_page(0, &ciphertext).unwrap();
        assert_eq!(result, ciphertext);
    }

    #[test]
    fn test_7c2_reencrypt_all_pages_workflow() {
        // 验证：批量重加密所有页 → 最终所有数据用新密钥
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        // 加密 100 页
        let mut pages: Vec<(u64, Vec<u8>, Vec<u8>)> = Vec::new();
        for page_id in 0..100u64 {
            let plaintext = format!("page_{page_id}_content").into_bytes();
            let ciphertext = engine.encrypt_page(page_id, &plaintext).unwrap();
            pages.push((page_id, plaintext, ciphertext));
        }

        // 延迟轮换
        engine
            .rotate_key_lazy_bytes(&[2u8; AES_256_KEY_LEN])
            .unwrap();
        assert!(engine.is_rotating());

        // 逐页重加密
        let mut reencrypted_pages: Vec<(u64, Vec<u8>)> = Vec::new();
        for (page_id, _, ciphertext) in &pages {
            let new_ct = engine.reencrypt_page(*page_id, ciphertext).unwrap();
            assert_eq!(new_ct[4], 1); // 全部升级到 key_version = 1
            reencrypted_pages.push((*page_id, new_ct));
        }

        // 完成轮换
        engine.finalize_rotation().unwrap();
        assert!(!engine.is_rotating());

        // 所有重加密后的页可用新密钥解密，且数据正确
        for ((page_id, plaintext, _), (_, new_ct)) in pages.iter().zip(reencrypted_pages.iter()) {
            let decrypted = engine.decrypt_page(*page_id, new_ct).unwrap();
            assert_eq!(&decrypted, plaintext);
        }
    }

    // -----------------------------------------------------------------
    //  finalize_rotation 完成轮换
    // -----------------------------------------------------------------

    #[test]
    fn test_7c2_finalize_rotation() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        engine
            .rotate_key_lazy_bytes(&[2u8; AES_256_KEY_LEN])
            .unwrap();
        assert!(engine.is_rotating());

        engine.finalize_rotation().unwrap();
        assert!(!engine.is_rotating());
        assert!(engine.old_key_versions().is_empty());
    }

    #[test]
    fn test_7c2_finalize_no_active_rotation() {
        // 验证：无轮换时调用 finalize_rotation → NoActiveRotation
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let result = engine.finalize_rotation();
        assert_eq!(result.unwrap_err(), TdeError::NoActiveRotation);

        // 立即轮换后也无 active rotation
        engine.rotate_key_bytes(&[2u8; AES_256_KEY_LEN]).unwrap();
        let result = engine.finalize_rotation();
        assert_eq!(result.unwrap_err(), TdeError::NoActiveRotation);
    }

    // -----------------------------------------------------------------
    //  is_rotating + key_version 状态
    // -----------------------------------------------------------------

    #[test]
    fn test_7c2_is_rotating_status() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        // 初始状态：未轮换
        assert!(!engine.is_rotating());

        // 延迟轮换 → 正在轮换
        engine
            .rotate_key_lazy_bytes(&[2u8; AES_256_KEY_LEN])
            .unwrap();
        assert!(engine.is_rotating());

        // 再次延迟轮换 → 仍在轮换
        engine
            .rotate_key_lazy_bytes(&[3u8; AES_256_KEY_LEN])
            .unwrap();
        assert!(engine.is_rotating());

        // 完成 → 不再轮换
        engine.finalize_rotation().unwrap();
        assert!(!engine.is_rotating());
    }

    #[test]
    fn test_7c2_key_version_increment() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        assert_eq!(engine.key_version(), 0);

        engine.rotate_key_bytes(&[2u8; AES_256_KEY_LEN]).unwrap();
        assert_eq!(engine.key_version(), 1);

        engine.rotate_key_bytes(&[3u8; AES_256_KEY_LEN]).unwrap();
        assert_eq!(engine.key_version(), 2);

        engine
            .rotate_key_lazy_bytes(&[4u8; AES_256_KEY_LEN])
            .unwrap();
        assert_eq!(engine.key_version(), 3);
    }

    // -----------------------------------------------------------------
    //  多次轮换 + 多版本旧密钥共存
    // -----------------------------------------------------------------

    #[test]
    fn test_7c2_multiple_rotations_coexistence() {
        // 验证：多次延迟轮换后，多个旧版本密钥共存，各版本密文均可解密
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        // key_version=0 加密页 0
        let pt0 = b"version 0 data".to_vec();
        let ct0 = engine.encrypt_page(0, &pt0).unwrap();

        engine
            .rotate_key_lazy_bytes(&[2u8; AES_256_KEY_LEN])
            .unwrap();
        // key_version=1 加密页 1
        let pt1 = b"version 1 data".to_vec();
        let ct1 = engine.encrypt_page(1, &pt1).unwrap();

        engine
            .rotate_key_lazy_bytes(&[3u8; AES_256_KEY_LEN])
            .unwrap();
        // key_version=2 加密页 2
        let pt2 = b"version 2 data".to_vec();
        let ct2 = engine.encrypt_page(2, &pt2).unwrap();

        // 三个版本的密钥共存
        assert_eq!(engine.key_version(), 2);
        assert!(engine.is_rotating());
        let old_versions = engine.old_key_versions();
        assert_eq!(old_versions.len(), 2);
        assert!(old_versions.contains(&0));
        assert!(old_versions.contains(&1));

        // 各版本密文均可正确解密
        assert_eq!(engine.decrypt_page(0, &ct0).unwrap(), pt0);
        assert_eq!(engine.decrypt_page(1, &ct1).unwrap(), pt1);
        assert_eq!(engine.decrypt_page(2, &ct2).unwrap(), pt2);

        // 完成轮换 → 旧版本密文不可解密
        engine.finalize_rotation().unwrap();
        assert_eq!(
            engine.decrypt_page(0, &ct0).unwrap_err(),
            TdeError::UnknownKeyVersion(0)
        );
        assert_eq!(
            engine.decrypt_page(1, &ct1).unwrap_err(),
            TdeError::UnknownKeyVersion(1)
        );
        // 当前版本仍可解密
        assert_eq!(engine.decrypt_page(2, &ct2).unwrap(), pt2);
    }

    #[test]
    fn test_7c2_old_key_versions() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        // 无轮换时 → 空列表
        assert!(engine.old_key_versions().is_empty());

        engine
            .rotate_key_lazy_bytes(&[2u8; AES_256_KEY_LEN])
            .unwrap();
        assert_eq!(engine.old_key_versions(), vec![0]);

        engine
            .rotate_key_lazy_bytes(&[3u8; AES_256_KEY_LEN])
            .unwrap();
        assert_eq!(engine.old_key_versions(), vec![0, 1]);

        engine.finalize_rotation().unwrap();
        assert!(engine.old_key_versions().is_empty());
    }

    // -----------------------------------------------------------------
    //  错误处理
    // -----------------------------------------------------------------

    #[test]
    fn test_7c2_rotate_key_lazy_not_enabled() {
        let mut engine = TdeEngine::new();
        let key = MasterKey::generate();
        let result = engine.rotate_key_lazy(key);
        assert_eq!(result.unwrap_err(), TdeError::NotEnabled);
    }

    #[test]
    fn test_7c2_rotate_key_lazy_bytes_invalid_length() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let result = engine.rotate_key_lazy_bytes(&[0u8; 16]);
        assert!(matches!(
            result.unwrap_err(),
            TdeError::InvalidKeyLength { .. }
        ));
    }

    #[test]
    fn test_7c2_reencrypt_too_short() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let result = engine.reencrypt_page(0, b"abc");
        assert!(matches!(
            result.unwrap_err(),
            TdeError::CiphertextTooShort { .. }
        ));
    }

    #[test]
    fn test_7c2_unknown_key_version_after_immediate_rotate() {
        // 验证：立即轮换后旧密文返回 UnknownKeyVersion
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        let plaintext = b"old data".to_vec();
        let ciphertext = engine.encrypt_page(0, &plaintext).unwrap();

        engine.rotate_key_bytes(&[2u8; AES_256_KEY_LEN]).unwrap();

        let result = engine.decrypt_page(0, &ciphertext);
        assert_eq!(result.unwrap_err(), TdeError::UnknownKeyVersion(0));
    }

    // -----------------------------------------------------------------
    //  KeyVersionOverflow 溢出保护
    // -----------------------------------------------------------------

    #[test]
    fn test_7c2_key_version_overflow() {
        // 验证：key_version 达到 255 后再轮换 → KeyVersionOverflow
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        // 手动将 key_version 设为 255（模拟 255 次轮换后的状态）
        engine.key_version = 255;

        // 延迟轮换 → 应返回 KeyVersionOverflow
        let result = engine.rotate_key_lazy_bytes(&[2u8; AES_256_KEY_LEN]);
        assert_eq!(result.unwrap_err(), TdeError::KeyVersionOverflow);

        // 验证状态未被破坏（事务性）：key_version 不变，old_keys 无残留
        assert_eq!(engine.key_version(), 255);
        assert!(engine.old_key_versions().is_empty());
        assert!(!engine.is_rotating());

        // 立即轮换 → 也应返回 KeyVersionOverflow
        let result = engine.rotate_key_bytes(&[3u8; AES_256_KEY_LEN]);
        assert_eq!(result.unwrap_err(), TdeError::KeyVersionOverflow);
        assert_eq!(engine.key_version(), 255);
    }

    // -----------------------------------------------------------------
    //  disable 清除轮换状态
    // -----------------------------------------------------------------

    #[test]
    fn test_7c2_disable_clears_rotation_state() {
        let mut engine = TdeEngine::new();
        engine.enable(&[1u8; AES_256_KEY_LEN]).unwrap();

        engine
            .rotate_key_lazy_bytes(&[2u8; AES_256_KEY_LEN])
            .unwrap();
        assert!(engine.is_rotating());
        assert_eq!(engine.key_version(), 1);

        // disable 清除所有状态
        engine.disable().unwrap();
        assert!(!engine.is_enabled());
        assert!(!engine.is_rotating());
        assert_eq!(engine.key_version(), 0);
        assert!(engine.old_key_versions().is_empty());

        // 重新启用后 key_version 从 0 开始
        engine.enable(&[3u8; AES_256_KEY_LEN]).unwrap();
        assert_eq!(engine.key_version(), 0);
    }

    // -----------------------------------------------------------------
    //  完整工作流（模拟 ALTER SYSTEM ROTATE ENCRYPTION KEY）
    // -----------------------------------------------------------------

    #[test]
    fn test_7c2_full_rotation_workflow() {
        // 验证标准：ALTER SYSTEM ROTATE ENCRYPTION KEY →
        //          新数据用新密钥 → 旧数据在读取时逐步重加密 →
        //          所有数据最终用新密钥
        let mut engine = TdeEngine::new();
        let initial_key = MasterKey::from_passphrase("initial_password", b"salt", 1000);
        engine.enable_with_key(initial_key).unwrap();

        // 写入 50 页数据
        let mut pages: Vec<(u64, Vec<u8>, Vec<u8>)> = Vec::new(); // (id, plaintext, ciphertext)
        for page_id in 0..50u64 {
            let plaintext =
                format!("INSERT INTO users VALUES ({page_id}, 'user{page_id}')").into_bytes();
            let ciphertext = engine.encrypt_page(page_id, &plaintext).unwrap();
            assert_eq!(ciphertext[4], 0); // key_version = 0
            pages.push((page_id, plaintext, ciphertext));
        }

        // ALTER SYSTEM ROTATE ENCRYPTION KEY（延迟轮换）
        let new_key = MasterKey::from_passphrase("new_password", b"salt", 1000);
        let old_fp = engine.rotate_key_lazy(new_key).unwrap();
        assert!(!old_fp.is_empty());
        assert_eq!(engine.key_version(), 1);
        assert!(engine.is_rotating());

        // 新数据用新密钥（key_version = 1）
        for page_id in 50..100u64 {
            let plaintext =
                format!("INSERT INTO users VALUES ({page_id}, 'user{page_id}')").into_bytes();
            let ciphertext = engine.encrypt_page(page_id, &plaintext).unwrap();
            assert_eq!(ciphertext[4], 1); // key_version = 1
            pages.push((page_id, plaintext, ciphertext));
        }

        // 所有数据可读（旧数据用旧密钥，新数据用新密钥）
        for (page_id, plaintext, ciphertext) in &pages {
            let decrypted = engine.decrypt_page(*page_id, ciphertext).unwrap();
            assert_eq!(&decrypted, plaintext);
        }

        // 逐步重加密旧数据（模拟读取时重加密）
        for (page_id, _, ciphertext) in pages.iter_mut() {
            if ciphertext[4] == 0 {
                let new_ct = engine.reencrypt_page(*page_id, ciphertext).unwrap();
                assert_eq!(new_ct[4], 1); // 升级到 key_version = 1
                *ciphertext = new_ct;
            }
        }

        // 完成轮换
        engine.finalize_rotation().unwrap();
        assert!(!engine.is_rotating());

        // 所有数据最终用新密钥（key_version = 1）
        for (page_id, plaintext, ciphertext) in &pages {
            assert_eq!(ciphertext[4], 1); // 全部是新版本
            let decrypted = engine.decrypt_page(*page_id, ciphertext).unwrap();
            assert_eq!(&decrypted, plaintext);
        }
    }

    // =================================================================
    //  P2-17：TdePageEncryptor — WalWriter FPI 加密路径集成
    // =================================================================

    #[test]
    fn test_p2_17_page_encryptor_encrypts_fpi_data() {
        // 验证标准：TdePageEncryptor 作为 PageEncryptor trait 实现，
        // 对 FPI 页数据加密后，密文可通过 TdeEngine 正确解密。
        let mut tde = TdeEngine::new();
        tde.enable(&[42u8; AES_256_KEY_LEN]).unwrap();
        let encryptor = TdePageEncryptor::new(tde);

        let page_id: u32 = 100;
        let plaintext = vec![0xABu8; 8192]; // 模拟 8KB 数据页

        // 通过 trait 接口加密
        let ciphertext = <TdePageEncryptor as szrsql_tx::wal::PageEncryptor>::encrypt(
            &encryptor, page_id, &plaintext,
        )
        .expect("encrypt should succeed");

        // 密文长度 > 明文（含 magic + key_version 头）
        assert!(ciphertext.len() > plaintext.len());

        // 用底层引擎解密验证正确性
        let mut engine = encryptor.engine().unwrap();
        let decrypted = engine.decrypt_page(page_id as u64, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_p2_17_page_encryptor_different_pages_different_ciphertext() {
        // 验证：相同明文 + 不同 page_id → 不同密文（IV 绑定 page_id）
        let mut tde = TdeEngine::new();
        tde.enable(&[7u8; AES_256_KEY_LEN]).unwrap();
        let encryptor = TdePageEncryptor::new(tde);

        let plaintext = b"same page content".to_vec();
        let ct1 =
            <TdePageEncryptor as szrsql_tx::wal::PageEncryptor>::encrypt(&encryptor, 1, &plaintext)
                .unwrap();
        let ct2 =
            <TdePageEncryptor as szrsql_tx::wal::PageEncryptor>::encrypt(&encryptor, 2, &plaintext)
                .unwrap();

        assert_ne!(ct1, ct2);
    }

    #[test]
    fn test_p2_17_page_encryptor_empty_page() {
        // 边界：空页数据加密/解密
        let mut tde = TdeEngine::new();
        tde.enable(&[1u8; AES_256_KEY_LEN]).unwrap();
        let encryptor = TdePageEncryptor::new(tde);

        let ciphertext =
            <TdePageEncryptor as szrsql_tx::wal::PageEncryptor>::encrypt(&encryptor, 0, &[])
                .unwrap();
        assert_eq!(ciphertext.len(), 5); // magic(4) + key_version(1)

        let mut engine = encryptor.engine().unwrap();
        let decrypted = engine.decrypt_page(0, &ciphertext).unwrap();
        assert!(decrypted.is_empty());
    }
}
