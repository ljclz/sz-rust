# mbedTLS（PolarSSL）架构借鉴 —— szrsql 安全层重构参考

> 分析对象：mbedTLS v3.x（ARM 的 TLS/密码学库，PSA Cryptography API 参考实现）
> 目标项目：szrsql（Rust 数据库）
> 基于版本：2026-07 工作区代码状态

---

## 1. 背景

szrsql 现有的安全/加密功能已经涵盖：

| 模块 | 文件 | 加密操作 | 密钥管理 | 状态 |
|------|------|----------|----------|------|
| TDE 透明加密 | [tde.rs](../crates/szrsql-security/src/tde.rs) | AES-256-CTR（手动实现） | `MasterKey`（独立结构体） | 已实现 |
| 列级加密 | [column_enc.rs](../crates/szrsql-security/src/column_enc.rs) | AES-256-GCM（`aes-gcm` crate） | `ColumnKey`（独立结构体） | 已实现 |
| 审计哈希链 | [audit_hash.rs](../crates/szrsql-security/src/audit_hash.rs) | SHA-256（`sha2` crate） | 无 | 已实现 |
| TLS 传输加密 | [tls.rs](../crates/szrsql-protocol/src/pgwire/tls.rs) | 委托 rustls | 委托 rustls | 已实现 |
| 密码派生 | [column_enc.rs:132-148](../crates/szrsql-security/src/column_enc.rs#L132-L148) | 手写 SHA-256 迭代 | 无 | 已实现 |

### 1.1 当前架构的核心问题

逐个看实际代码可以发现三个具体问题：

**问题 A：密钥管理重复**

`MasterKey`（[tde.rs:102-166](file:///E:/vue/test/鲜视达/rust/szrsql/crates/szrsql-security/src/tde.rs#L102-L166)）和 `ColumnKey`（[column_enc.rs:96-178](file:///E:/vue/test/鲜视达/rust/szrsql/crates/szrsql-security/src/column_enc.rs#L96-L178)）是两份几乎相同的代码：

| 功能 | MasterKey | ColumnKey |
|------|-----------|-----------|
| `from_bytes(bytes)` | ✅ | ✅ |
| `generate()` | ✅ | ✅ |
| `from_passphrase()` | ✅ | ✅ |
| `fingerprint()` | ✅ | ✅ |
| `Debug` 不泄露密钥 | ✅ | ✅ |
| 密钥轮换 | ✅（rotete_key_lazy） | ❌ |

**问题 B：密码学操作分散在安全模块中**

TDE 手动实现 AES-256-CTR（[tde.rs:209-233](file:///E:/vue/test/鲜视达/rust/szrsql/crates/szrsql-security/src/tde.rs#L209-L233)），列加密直接调 `aes-gcm` crate 的 API（[column_enc.rs:436-461](file:///E:/vue/test/鲜视达/rust/szrsql/crates/szrsql-security/src/column_enc.rs#L436-L461)）。如果将来要：
- 把 TDE 从 CTR 模式换成 XTS 模式（磁盘加密标准）
- 把列加密从 AES-256-GCM 换成 ChaCha20-Poly1305
- 接入硬件加速（AES-NI、QAT）

都需要逐个模块修改。

**问题 C：没有统一的加密审计点**

目前无法回答"系统里哪些页面用了什么算法、什么密钥加密"——每个模块有自己的统计（`TdeStats`、`ColumnEncStats`），但没有全局视图。

---

## 2. PSA Cryptography API 三层架构

mbedTLS 的核心价值不在于实现密码学，而在于定义了 **PSA Cryptography API** —— 一套标准化的密码学接口规范：

```
┌───────────────────────────────────────┐
│  上层应用                             │
│  TDE、列加密、审计哈希、密钥轮换等        │
├───────────────────────────────────────┤
│  PSA Crypto API（统一抽象层）           │
│  psa_cipher_encrypt()                 │
│  psa_hash_compute()                   │
│  psa_key_management()                 │
│  psa_key_attributes()                 │
├───────────────────────────────────────┤
│  后端实现                              │
│  软件: mbedtls 原生实现                │
│  硬件: TEE / TPM / Secure Element     │
└───────────────────────────────────────┘
```

szrsql 是 Rust 数据库，可以借鉴同样的分层思路：在 `szrsql-security` crate 内部增加一个 `crypto` 模块，作为所有密码学操作的统一入口。

---

## 3. 具体改造方案

### 3.1 新增 `szrsql-security/src/crypto/` 模块

```
szrsql-security/src/crypto/
├── mod.rs              # 统一导出 + 运行时初始化
├── cipher.rs           # 加密/解密 trait + 算法枚举
├── hash.rs             # 哈希 trait
├── key.rs              # 统一密钥类型（替代 MasterKey / ColumnKey）
├── key_store.rs        # 统一密钥存储（替代多份 HashMap）
├── rng.rs              # 密码学安全随机数封装
├── secure_memory.rs    # 安全内存（零化、mlock 等）
└── providers/
    ├── mod.rs
    ├── software.rs     # 纯软件实现（基于 aes-gcm、sha2 等 crate）
    └── hsm.rs          # （预留）HSM/TPM 后端
```

### 3.2 cipher.rs —— 算法枚举 + 加密 trait

```rust
// szrsql-security/src/crypto/cipher.rs

/// 密码学算法标识（与具体实现解耦）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CipherAlgorithm {
    /// AES-256-GCM 认证加密
    Aes256Gcm,
    /// AES-256-CTR 流加密（当前 TDE 使用）
    Aes256Ctr,
    /// AES-256-XTS 磁盘加密（未来 TDE 升级选择）
    Aes256Xts,
    /// ChaCha20-Poly1305（未来可选）
    Chacha20Poly1305,
    /// 无加密（透传）
    None,
}

/// 加密结果（统一格式，各算法各自填充）
#[derive(Debug)]
pub struct EncryptOutput {
    /// 密文
    pub ciphertext: Vec<u8>,
    /// Nonce / IV
    pub nonce: Vec<u8>,
    /// 认证标签（仅 AEAD 模式）
    pub tag: Option<Vec<u8>>,
    /// 使用的密钥 ID
    pub key_id: String,
    /// 使用的算法
    pub algorithm: CipherAlgorithm,
}

/// 加密/解密 trait
#[async_trait]
pub trait Cipher: Send + Sync {
    /// 加密
    async fn encrypt(
        &self,
        key: &KeyMaterial,
        algorithm: CipherAlgorithm,
        plaintext: &[u8],
        aad: Option<&[u8]>,
    ) -> Result<EncryptOutput, CryptoError>;

    /// 解密
    async fn decrypt(
        &self,
        key: &KeyMaterial,
        algorithm: CipherAlgorithm,
        ciphertext: &[u8],
        nonce: &[u8],
        tag: Option<&[u8]>,
        aad: Option<&[u8]>,
    ) -> Result<Vec<u8>, CryptoError>;
}
```

**核心价值：** `EncryptOutput` 携带 `key_id` 和 `algorithm`，为审计提供结构化数据，不再需要各模块各自拼凑统计。

### 3.3 key.rs —— 统一密钥类型

合并 `MasterKey` 和 `ColumnKey` 为统一的 `KeyMaterial`：

```rust
// szrsql-security/src/crypto/key.rs

/// 密钥用途标签（限制密钥只能用于特定操作）
#[derive(Debug, Clone)]
pub enum KeyUsage {
    /// 可用于加密/解密
    EncryptDecrypt,
    /// 仅用于计算哈希（如审计链）
    HashOnly,
    /// 仅用于签名/验证
    SignVerify,
}

/// 统一密钥材料
pub struct KeyMaterial {
    /// 密钥 ID（全局唯一，用于审计追踪）
    key_id: String,
    /// 密钥字节（由 SecureMemory 管理）
    bytes: SecureMemory<32>,
    /// 允许的算法列表（None = 无限制）
    allowed_algorithms: Option<Vec<CipherAlgorithm>>,
    /// 密钥用途
    usage: KeyUsage,
    /// 创建时间
    created_at: std::time::Instant,
}

impl KeyMaterial {
    pub fn from_bytes(
        key_id: impl Into<String>,
        bytes: &[u8],
        usage: KeyUsage,
    ) -> Result<Self, CryptoError> {
        // 校验密钥长度 ...
        // 拷贝到 SecureMemory ...
    }

    pub fn generate(
        key_id: impl Into<String>,
        algorithm: CipherAlgorithm,
    ) -> Self {
        // 根据算法类型自动确定密钥长度并生成随机密钥
    }

    /// 指纹（SHA-256 前 8 字节，与现有 MasterKey/ColumnKey 一致）
    pub fn fingerprint(&self) -> String { /* ... */ }
}
```

**改造后，MasterKey 和 ColumnKey 统一替换为 KeyMaterial：**

- [tde.rs:102](file:///E:/vue/test/鲜视达/rust/szrsql/crates/szrsql-security/src/tde.rs#L102) `struct MasterKey` → `type MasterKey = KeyMaterial`（或直接删除）
- [column_enc.rs:96](file:///E:/vue/test/鲜视达/rust/szrsql/crates/szrsql-security/src/column_enc.rs#L96) `struct ColumnKey` → `type ColumnKey = KeyMaterial`（或直接删除）

### 3.4 key_store.rs —— 统一密钥存储

替代 TDE 和列加密各自持有的 `HashMap`：

```rust
// szrsql-security/src/crypto/key_store.rs

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 统一密钥存储
pub struct KeyStore {
    keys: RwLock<HashMap<String, KeyMaterial>>,
    // 轮换过渡期保留的旧密钥：(key_id, version) → KeyMaterial
    archived: RwLock<Vec<(String, KeyMaterial)>>,
}

impl KeyStore {
    pub fn new() -> Self { /* ... */ }

    /// 注册密钥
    pub async fn register(&self, key: KeyMaterial) { /* ... */ }

    /// 获取密钥
    pub async fn get(&self, key_id: &str) -> Option<KeyMaterial> { /* ... */ }

    /// 删除密钥（安全销毁密钥材料）
    pub async fn destroy(&self, key_id: &str) { /* ... */ }

    /// 密钥轮换：归档旧密钥 → 注册新密钥（参考 tde.rs:rotate_key_lazy）
    pub async fn rotate(&self, key_id: &str, new_key: KeyMaterial) { /* ... */ }

    /// 完成轮换：清除归档密钥
    pub fn finalize_rotation(&self) { /* ... */ }
}
```

**使用效果对比：**

| 当前（TDE） | 改造后 |
|-------------|--------|
| `TdeEngine` 持有 `master_key: Option<MasterKey>` + `old_keys: Vec<(u8, MasterKey)>` | `KeyStore` 统一管理，TdeEngine 不再持有密钥 |
| `ColumnEncryptionEngine` 持有 `keys: HashMap<String, ColumnKey>` | `KeyStore` 统一管理，ColumnEncryptionEngine 不再持有密钥 |
| 各模块自行实现密钥轮换逻辑 | `KeyStore::rotate()` 一次实现，全局可用 |

### 3.5 providers/software.rs —— 统一后端实现

```rust
// szrsql-security/src/crypto/providers/software.rs

use aes_gcm::{Aes256Gcm, Nonce};
use aes::cipher::{BlockEncrypt, KeyInit};
use sha2::{Digest, Sha256};

pub struct SoftwareCipher;

#[async_trait]
impl Cipher for SoftwareCipher {
    async fn encrypt(
        &self,
        key: &KeyMaterial,
        algorithm: CipherAlgorithm,
        plaintext: &[u8],
        aad: Option<&[u8]>,
    ) -> Result<EncryptOutput, CryptoError> {
        match algorithm {
            CipherAlgorithm::Aes256Gcm => {
                // 从 column_enc.rs:436 提取 + 包装
                let cipher = Aes256Gcm::new_from_slice(key.bytes())?;
                let nonce_bytes = rng::generate::<12>()?;
                let nonce = Nonce::from_slice(&nonce_bytes);
                let ct_with_tag = cipher.encrypt(nonce, plaintext)?;
                // 分离 ciphertext 和 tag（GCM 的 tag 在末尾 16 字节）
                let (ct, tag) = ct_with_tag.split_at(ct_with_tag.len() - 16);
                Ok(EncryptOutput {
                    ciphertext: ct.to_vec(),
                    nonce: nonce_bytes.to_vec(),
                    tag: Some(tag.to_vec()),
                    key_id: key.key_id().to_string(),
                    algorithm,
                })
            }
            CipherAlgorithm::Aes256Ctr => {
                // 从 tde.rs:209-233 提取 + 包装
                let iv = derive_iv(key.bytes(), 0);  // page_id 由上层传入
                let result = aes_256_ctr_process(key.bytes(), &iv, plaintext);
                Ok(EncryptOutput {
                    ciphertext: result,
                    nonce: iv.to_vec(),
                    tag: None,
                    key_id: key.key_id().to_string(),
                    algorithm,
                })
            }
            CipherAlgorithm::None => {
                Ok(EncryptOutput {
                    ciphertext: plaintext.to_vec(),
                    nonce: vec![],
                    tag: None,
                    key_id: key.key_id().to_string(),
                    algorithm,
                })
            }
        }
    }
    // ... decrypt 类似
}
```

**注意：** CTR 模式需要一个额外的 `page_id` 来派生 IV，这和 GCM 不同。解决方案：
- 在 `encrypt()` 参数中加一个 `context: &[u8]` 字段用于 IV 派生
- 或者 CTR 模式单独实现不经过统一接口（保留在 TDE 内部但统一密钥管理）

### 3.6 mod.rs —— 初始化 + 全局访问

```rust
// szrsql-security/src/crypto/mod.rs

use once_cell::sync::OnceCell;

static CIPHER: OnceCell<Box<dyn Cipher>> = OnceCell::new();

/// 初始化密码学层。szrsql 启动时调用一次。
pub fn init_crypto(backend: CryptoBackend) {
    let cipher: Box<dyn Cipher> = match backend {
        CryptoBackend::Software => Box::new(providers::software::SoftwareCipher),
        CryptoBackend::Hsm => todo!("Phase 7c.8 HSM 集成"),
    };
    CIPHER.set(cipher).ok();
}

/// 获取 Cipher 实例
pub fn cipher() -> &'static dyn Cipher {
    CIPHER.get().expect("crypto not initialized")
}
```

---

## 4. 各模块的迁移路径

### 4.1 列加密（column_enc.rs）—— 最简单的切入点

**改造前：** `ColumnEncryptionEngine` 直接调 `aes-gcm` crate，自己管理密钥。

```rust
// 当前 column_enc.rs:436
let cipher = Aes256Gcm::new_from_slice(key.as_bytes())?;
let ciphertext_with_tag = cipher.encrypt(nonce, plaintext)?;
```

**改造后：**

```rust
// 改造后
use crate::crypto::{cipher, key_store, CipherAlgorithm};

let key = key_store().get("key_ssn").await?;
let output = cipher().encrypt(&key, CipherAlgorithm::Aes256Gcm, plaintext, None).await?;
// output 自动包含 key_id 和 algorithm，审计信息完备
```

### 4.2 TDE（tde.rs）—— 保持内部实现，统一密钥管理

TDE 的 AES-256-CTR 实现（[tde.rs:209-233](file:///E:/vue/test/鲜视达/rust/szrsql/crates/szrsql-security/src/tde.rs#L209-L233)）绑定 page_id 做 IV 派生，不适合走统一 Cipher 接口。但**密钥管理**可以统一：

```rust
// 当前 tde.rs:261-271 — TdeEngine 自己持有密钥
pub struct TdeEngine {
    master_key: Option<MasterKey>,
    old_keys: Vec<(u8, MasterKey)>,
    key_version: u8,
    // ...
}

// 改造后 — TdeEngine 只持有 key_id，密钥从 KeyStore 获取
pub struct TdeEngine {
    key_id: Option<String>,
    // 密钥版本号仍由 TdeEngine 管理（CTR 密文头部需要版本号）
    key_version: u8,
    // 不再持有 MasterKey
}
```

### 4.3 哈希操作（audit_hash.rs）—— 统一 Hash trait

```rust
// src/crypto/hash.rs
pub trait Hash: Send + Sync {
    fn update(&mut self, data: &[u8]);
    fn finalize(self) -> Vec<u8>;
}

pub enum HashAlgorithm {
    Sha256,
    Sha512,
    Blake3,
}
```

---

## 5. SecureMemory —— 密钥内存安全

这是 mbedTLS/PSA 的一个重要设计点。Rust 的 `zeroize` crate 提供了类似功能，但 szrsql 目前没有系统性地使用。

```rust
// szrsql-security/src/crypto/secure_memory.rs

use zeroize::Zeroize;
use std::ops::Deref;

/// 安全内存：析构时自动清零
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct SecureMemory<const N: usize>([u8; N]);

impl<const N: usize> SecureMemory<N> {
    pub fn new(bytes: &[u8]) -> Self {
        let mut inner = [0u8; N];
        inner.copy_from_slice(bytes);
        Self(inner)
    }
}
```

建议在 `Cargo.toml` 中添加 `zeroize` 依赖，并将 `KeyMaterial.bytes` 的类型改为 `SecureMemory<32>`。

---

## 6. 审计点统一

当前各模块各自维护统计（[tde.rs:273-286](file:///E:/vue/test/鲜视达/rust/szrsql/crates/szrsql-security/src/tde.rs#L273-L286) `TdeStats`、[column_enc.rs:326-335](file:///E:/vue/test/鲜视达/rust/szrsql/crates/szrsql-security/src/column_enc.rs#L326-L335) `ColumnEncStats`），格式不同。

在 Cipher 基类加统一的审计钩子：

```rust
#[derive(Clone, Serialize)]
pub struct CryptoAuditEvent {
    pub timestamp: i64,
    pub operation: &'static str,      // "encrypt" | "decrypt" | "hash"
    pub algorithm: CipherAlgorithm,
    pub key_id: String,
    pub plaintext_len: usize,
    pub ciphertext_len: usize,
    pub success: bool,
}

pub trait Cipher: Send + Sync {
    fn on_event(&self, event: CryptoAuditEvent) {
        // 默认空实现 — 可被 AuditLog 覆盖
    }
    // ...
}
```

这样所有加密操作自动进入审计系统，无需各模块手动记录统计。

---

## 7. 迁移优先级

| 阶段 | 内容 | 涉及文件 | 收益 |
|------|------|----------|------|
| **P0** | 建 `crypto/` 目录骨架 + `KeyMaterial` 统一类型 | `crypto/{mod,key,key_store}.rs` | 消除 MasterKey/ColumnKey 重复 |
| **P1** | `KeyStore` 实现 + 列加密迁移 | `column_enc.rs`、`key_store.rs` | 列加密密钥管理集中化 |
| **P2** | TDE 迁移到 KeyStore | `tde.rs` | TDE 密钥管理集中化 |
| **P3** | `Cipher` trait + `SoftwareCipher`（提取现有代码） | `cipher.rs`、`providers/software.rs` | 加密操作统一接口 |
| **P4** | `SecureMemory` + `zeroize` 集成 | `secure_memory.rs`、`Cargo.toml` | 密钥内存安全 |
| **P5** | 统一审计钩子 | `cipher.rs`、`audit.rs` | 全局限密可观测性 |
| **P6** | （可选）HSM/TPM provider | `providers/hsm.rs` | 硬件加速 |

---

## 8. 和 szrsql 现有设计的兼容性

1. **最小侵入** — 第一阶段（P0-P1）只新增文件，不改现有模块的行为。现有模块逐步接入新接口，不影响测试。
2. **渐进替换** — 所有现有测试依然有效。统一接口的测试覆盖在 `crypto/` 内部，模块级测试逐步迁移。
3. **零新依赖（P0-P3）** — 只需要将已有的 `aes`、`aes-gcm`、`sha2` 等 crate 重新组织，P4 才新增 `zeroize`。
4. **保持 Rust 生态最佳实践** — PSA 的设计理念用 Rust trait 表达，比 C 语言的函数指针表更自然。
5. **与现有审计系统融合** — 审计钩子直接输出到已有的 `audit.rs`（`AuditLog`），不需要引入新的审计设施。

---

## 参考

- [PSA Certified Cryptography API 规范](https://arm-software.github.io/psa-api/crypto/)
- [mbedTLS PSA Crypto 实现（C 语言）](https://github.com/Mbed-TLS/mbedtls/tree/development/library)
- [zeroize crate](https://docs.rs/zeroize) — Rust 安全内存清零
- szrsql 现有模块：
  - [szrsql-security/src/tde.rs](../crates/szrsql-security/src/tde.rs)
  - [szrsql-security/src/column_enc.rs](../crates/szrsql-security/src/column_enc.rs)
  - [szrsql-security/src/audit_hash.rs](../crates/szrsql-security/src/audit_hash.rs)
  - [szrsql-protocol/src/pgwire/tls.rs](../crates/szrsql-protocol/src/pgwire/tls.rs)
