//! 审计日志哈希链 + RSA 签名 — Phase 7d.14
//!
//! 对应 `SzRSQL实施进度.md` Phase 7d.14。
//!
//! # 设计
//!
//! 在 Phase 7c.3 基础审计日志之上，本模块补充：
//!
//! 1. **SHA-256 哈希链** — 每条审计记录包含 `prev_hash`，形成链式结构
//! 2. **RSA 数字签名** — 对审计报告根哈希进行 RSA 签名，防篡改 + 防抵赖
//! 3. **完整性验证** — 从头重新计算哈希链，检测任意篡改
//! 4. **签名验证** — 使用公钥验证签名，确保审计报告真实可信

use sha2::{Digest, Sha256};

/// 哈希长度（SHA-256 = 32 字节）
pub const HASH_LEN: usize = 32;

/// 初始 prev_hash（全零）
pub const INITIAL_HASH: [u8; HASH_LEN] = [0u8; HASH_LEN];

/// RSA 默认公钥指数 e（65537）
pub const DEFAULT_PUBLIC_EXPONENT: u64 = 65537;

// =====================================================================
//  AuditEntry — 审计日志条目
// =====================================================================

/// 审计日志条目
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// 序列号（递增）
    pub seq: u64,
    /// 时间戳（Unix 秒）
    pub timestamp: u64,
    /// 操作用户
    pub user: String,
    /// 操作类型（INSERT/UPDATE/DELETE/DDL 等）
    pub action: String,
    /// 操作对象（表名/索引名等）
    pub object: String,
    /// 操作详情
    pub detail: String,
}

impl AuditEntry {
    /// 创建新的审计条目
    pub fn new(
        seq: u64,
        timestamp: u64,
        user: impl Into<String>,
        action: impl Into<String>,
        object: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            seq,
            timestamp,
            user: user.into(),
            action: action.into(),
            object: object.into(),
            detail: detail.into(),
        }
    }

    /// 序列化为字节流（用于哈希计算）
    ///
    /// 各字段以 `\x1f`（单元分隔符）分隔，确保无歧义。
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(128);
        bytes.extend_from_slice(&self.seq.to_be_bytes());
        bytes.extend_from_slice(&self.timestamp.to_be_bytes());
        bytes.extend_from_slice(self.user.as_bytes());
        bytes.push(0x1f);
        bytes.extend_from_slice(self.action.as_bytes());
        bytes.push(0x1f);
        bytes.extend_from_slice(self.object.as_bytes());
        bytes.push(0x1f);
        bytes.extend_from_slice(self.detail.as_bytes());
        bytes
    }
}

impl std::fmt::Display for AuditEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[#{} @{}] {} {} {} ({})",
            self.seq, self.timestamp, self.user, self.action, self.object, self.detail
        )
    }
}

// =====================================================================
//  ChainEntry — 哈希链条目
// =====================================================================

/// 哈希链条目（审计条目 + 该条目的哈希 + 前一条的哈希）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainEntry {
    /// 审计条目
    pub entry: AuditEntry,
    /// 本条目的哈希
    pub hash: [u8; HASH_LEN],
    /// 前一条目的哈希
    pub prev_hash: [u8; HASH_LEN],
}

impl ChainEntry {
    /// 返回本条目哈希的十六进制字符串
    pub fn hash_hex(&self) -> String {
        hex_encode(&self.hash)
    }

    /// 返回前一条目哈希的十六进制字符串
    pub fn prev_hash_hex(&self) -> String {
        hex_encode(&self.prev_hash)
    }
}

// =====================================================================
//  AuditHashChain — 哈希链
// =====================================================================

/// SHA-256 哈希链
///
/// 每条审计记录的哈希包含前一条的哈希，形成链式结构。
/// 篡改任意记录都会导致后续所有哈希不匹配。
#[derive(Debug, Clone, Default)]
pub struct AuditHashChain {
    /// 链中所有条目
    entries: Vec<ChainEntry>,
}

impl AuditHashChain {
    /// 创建空的哈希链
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 返回上一条记录的哈希（若链为空返回 INITIAL_HASH）
    fn last_hash(&self) -> [u8; HASH_LEN] {
        self.entries.last().map(|e| e.hash).unwrap_or(INITIAL_HASH)
    }

    /// 追加一条审计记录到链尾
    pub fn append(&mut self, entry: AuditEntry) -> [u8; HASH_LEN] {
        let prev_hash = self.last_hash();
        let hash = compute_entry_hash(&prev_hash, &entry);
        self.entries.push(ChainEntry {
            entry,
            hash,
            prev_hash,
        });
        hash
    }

    /// 返回链长度
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 链是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 返回所有条目的引用
    pub fn entries(&self) -> &[ChainEntry] {
        &self.entries
    }

    /// 返回根哈希（最后一条记录的哈希，链为空时返回 INITIAL_HASH）
    pub fn root_hash(&self) -> [u8; HASH_LEN] {
        self.last_hash()
    }

    /// 验证哈希链完整性
    ///
    /// 从头重新计算每条记录的哈希，检查是否与存储的哈希匹配。
    /// 返回 `Ok(())` 表示链完整，返回 `Err(index)` 表示第 `index` 条记录被篡改。
    pub fn verify(&self) -> Result<(), usize> {
        let mut prev_hash = INITIAL_HASH;
        for (idx, chain_entry) in self.entries.iter().enumerate() {
            if chain_entry.prev_hash != prev_hash {
                return Err(idx);
            }
            let expected = compute_entry_hash(&prev_hash, &chain_entry.entry);
            if chain_entry.hash != expected {
                return Err(idx);
            }
            prev_hash = chain_entry.hash;
        }
        Ok(())
    }

    /// 篡改第 `index` 条记录的 detail 字段（仅用于测试）
    pub fn tamper_detail(&mut self, index: usize, new_detail: impl Into<String>) -> String {
        let original = self.entries[index].entry.detail.clone();
        self.entries[index].entry.detail = new_detail.into();
        original
    }

    /// 篡改第 `index` 条记录的哈希字段（仅用于测试）
    pub fn tamper_hash(&mut self, index: usize) {
        let fake_hash = [0xABu8; HASH_LEN];
        self.entries[index].hash = fake_hash;
    }

    /// 清空链
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

// =====================================================================
//  RSA 密钥对
// =====================================================================

/// RSA 公钥（n, e）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsaPublicKey {
    /// 模数 n = p * q
    pub n: u64,
    /// 公钥指数 e
    pub e: u64,
}

/// RSA 私钥（n, d）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsaPrivateKey {
    /// 模数 n = p * q
    pub n: u64,
    /// 私钥指数 d = e^(-1) mod phi
    pub d: u64,
}

/// RSA 密钥对
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsaKeyPair {
    /// 公钥
    pub public_key: RsaPublicKey,
    /// 私钥
    pub private_key: RsaPrivateKey,
}

impl RsaKeyPair {
    /// 从两个素数 p, q 和公钥指数 e 构造密钥对
    ///
    /// # Panics
    ///
    /// 如果 p 或 q 不是素数，或 e 与 phi 不互素，将 panic。
    pub fn from_primes(p: u64, q: u64, e: u64) -> Self {
        assert!(is_prime(p), "p must be prime, got {}", p);
        assert!(is_prime(q), "q must be prime, got {}", q);
        assert!(p != q, "p and q must be distinct");
        assert!(e > 1, "e must be > 1");

        let n = p.checked_mul(q).expect("n = p * q overflow");
        let phi = (p - 1).checked_mul(q - 1).expect("phi overflow");
        let d = mod_inverse(e, phi).expect("e and phi must be coprime");

        Self {
            public_key: RsaPublicKey { n, e },
            private_key: RsaPrivateKey { n, d },
        }
    }

    /// 生成演示用密钥对（p=10007, q=10009, e=65537）
    pub fn generate_demo() -> Self {
        Self::from_primes(10007, 10009, DEFAULT_PUBLIC_EXPONENT)
    }

    /// 返回公钥
    pub fn public_key(&self) -> &RsaPublicKey {
        &self.public_key
    }

    /// 返回私钥
    pub fn private_key(&self) -> &RsaPrivateKey {
        &self.private_key
    }
}

// =====================================================================
//  RsaSignature / RsaSigner — 签名与验证
// =====================================================================

/// RSA 签名
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsaSignature {
    /// 签名值（signature = hash^d mod n）
    pub signature: u64,
    /// 被签名的哈希（u64 形式，由 32 字节哈希截断）
    pub signed_hash: u64,
}

impl RsaSignature {
    /// 签名是否为空（未签名）
    pub fn is_empty(&self) -> bool {
        self.signature == 0 && self.signed_hash == 0
    }
}

impl std::fmt::Display for RsaSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RSA(sig={}, hash={})", self.signature, self.signed_hash)
    }
}

/// RSA 签名器
#[derive(Debug, Clone)]
pub struct RsaSigner {
    /// 密钥对
    key_pair: RsaKeyPair,
}

impl RsaSigner {
    /// 从密钥对创建签名器
    pub fn new(key_pair: RsaKeyPair) -> Self {
        Self { key_pair }
    }

    /// 创建演示用签名器
    pub fn demo() -> Self {
        Self::new(RsaKeyPair::generate_demo())
    }

    /// 返回公钥
    pub fn public_key(&self) -> &RsaPublicKey {
        &self.key_pair.public_key
    }

    /// 返回私钥
    pub fn private_key(&self) -> &RsaPrivateKey {
        &self.key_pair.private_key
    }

    /// 对 32 字节哈希进行签名
    ///
    /// 由于使用 u64 作为 RSA 模数上限，哈希值需先取模 n 再签名。
    /// `signed_hash` 字段存储的是 `hash_u64 % n`，而非原始哈希。
    pub fn sign_hash(&self, hash: &[u8; HASH_LEN]) -> RsaSignature {
        let hash_u64 = hash_to_u64(hash);
        let n = self.key_pair.private_key.n;
        let hash_mod = hash_u64 % n;
        let signature = mod_pow(hash_mod, self.key_pair.private_key.d, n);
        RsaSignature {
            signature,
            signed_hash: hash_mod,
        }
    }

    /// 验证签名
    pub fn verify_hash(&self, sig: &RsaSignature) -> bool {
        let recovered = mod_pow(
            sig.signature,
            self.key_pair.public_key.e,
            self.key_pair.public_key.n,
        );
        recovered == sig.signed_hash
    }

    /// 使用指定公钥验证签名
    pub fn verify_with_public_key(sig: &RsaSignature, public_key: &RsaPublicKey) -> bool {
        let recovered = mod_pow(sig.signature, public_key.e, public_key.n);
        recovered == sig.signed_hash
    }
}

// =====================================================================
//  AuditReport — 审计报告
// =====================================================================

/// 审计报告（含哈希链根哈希 + 可选 RSA 签名）
#[derive(Debug, Clone)]
pub struct AuditReport {
    /// 报告生成时链长度
    pub entry_count: usize,
    /// 哈希链根哈希
    pub root_hash: [u8; HASH_LEN],
    /// 报告生成时间戳
    pub generated_at: u64,
    /// RSA 签名（None 表示未签名）
    pub signature: Option<RsaSignature>,
}

impl AuditReport {
    /// 从哈希链生成未签名报告
    pub fn from_chain(chain: &AuditHashChain) -> Self {
        Self {
            entry_count: chain.len(),
            root_hash: chain.root_hash(),
            generated_at: current_timestamp(),
            signature: None,
        }
    }

    /// 对报告进行 RSA 签名
    pub fn sign(&mut self, signer: &RsaSigner) {
        let sig = signer.sign_hash(&self.root_hash);
        self.signature = Some(sig);
    }

    /// 验证报告签名
    pub fn verify_signature(&self, public_key: &RsaPublicKey) -> bool {
        match &self.signature {
            Some(sig) => RsaSigner::verify_with_public_key(sig, public_key),
            None => false,
        }
    }

    /// 报告是否已签名
    pub fn is_signed(&self) -> bool {
        self.signature.is_some()
    }

    /// 序列化为字节流
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(64);
        bytes.extend_from_slice(&(self.entry_count as u64).to_be_bytes());
        bytes.extend_from_slice(&self.root_hash);
        bytes.extend_from_slice(&self.generated_at.to_be_bytes());
        if let Some(sig) = &self.signature {
            bytes.push(1);
            bytes.extend_from_slice(&sig.signature.to_be_bytes());
            bytes.extend_from_slice(&sig.signed_hash.to_be_bytes());
        } else {
            bytes.push(0);
        }
        bytes
    }
}

// =====================================================================
//  AuditSystem — 完整审计系统
// =====================================================================

/// 完整审计系统（哈希链 + RSA 签名）
#[derive(Debug, Clone)]
pub struct AuditSystem {
    /// 哈希链
    chain: AuditHashChain,
    /// RSA 签名器
    signer: RsaSigner,
    /// 序列号计数器
    seq_counter: u64,
}

impl AuditSystem {
    /// 创建新的审计系统（使用指定密钥对）
    pub fn with_key_pair(key_pair: RsaKeyPair) -> Self {
        Self {
            chain: AuditHashChain::new(),
            signer: RsaSigner::new(key_pair),
            seq_counter: 0,
        }
    }

    /// 创建演示用审计系统
    pub fn new() -> Self {
        Self::with_key_pair(RsaKeyPair::generate_demo())
    }

    /// 记录一条审计日志
    pub fn record(
        &mut self,
        user: impl Into<String>,
        action: impl Into<String>,
        object: impl Into<String>,
        detail: impl Into<String>,
    ) -> u64 {
        let seq = self.seq_counter;
        self.seq_counter += 1;
        let entry = AuditEntry::new(seq, current_timestamp(), user, action, object, detail);
        self.chain.append(entry);
        seq
    }

    /// 验证哈希链完整性
    pub fn verify_chain(&self) -> Result<(), usize> {
        self.chain.verify()
    }

    /// 生成已签名的审计报告
    pub fn generate_report(&mut self) -> AuditReport {
        let mut report = AuditReport::from_chain(&self.chain);
        report.sign(&self.signer);
        report
    }

    /// 验证审计报告签名
    pub fn verify_report(&self, report: &AuditReport) -> bool {
        report.verify_signature(self.signer.public_key())
    }

    /// 返回公钥
    pub fn public_key(&self) -> &RsaPublicKey {
        self.signer.public_key()
    }

    /// 返回审计记录数量
    pub fn entry_count(&self) -> usize {
        self.chain.len()
    }

    /// 清空审计链
    pub fn clear(&mut self) {
        self.chain.clear();
        self.seq_counter = 0;
    }
}

impl Default for AuditSystem {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 计算 `SHA256(prev_hash || entry.to_bytes())`
pub fn compute_entry_hash(prev_hash: &[u8; HASH_LEN], entry: &AuditEntry) -> [u8; HASH_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash);
    hasher.update(entry.to_bytes());
    let result = hasher.finalize();
    let mut hash = [0u8; HASH_LEN];
    hash.copy_from_slice(&result);
    hash
}

/// 计算 SHA-256 哈希
pub fn sha256(data: &[u8]) -> [u8; HASH_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hash = [0u8; HASH_LEN];
    hash.copy_from_slice(&result);
    hash
}

/// 将 32 字节哈希截断为 u64（取前 8 字节大端序）
pub fn hash_to_u64(hash: &[u8; HASH_LEN]) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash[..8]);
    u64::from_be_bytes(bytes)
}

/// 快速幂取模：计算 `(base^exp) mod modulus`
pub fn mod_pow(base: u64, exp: u64, modulus: u64) -> u64 {
    if modulus == 1 {
        return 0;
    }
    let mut result: u128 = 1;
    let mut base_u128 = (base as u128) % (modulus as u128);
    let mut exp = exp;

    while exp > 0 {
        if exp & 1 == 1 {
            result = (result * base_u128) % (modulus as u128);
        }
        exp >>= 1;
        base_u128 = (base_u128 * base_u128) % (modulus as u128);
    }
    result as u64
}

/// 扩展欧几里得算法求模逆元 `d = e^(-1) mod phi`
pub fn mod_inverse(e: u64, phi: u64) -> Option<u64> {
    let mut t: i128 = 0;
    let mut new_t: i128 = 1;
    let mut r: i128 = phi as i128;
    let mut new_r: i128 = e as i128;

    while new_r != 0 {
        let quotient = r / new_r;
        let temp_t = t - quotient * new_t;
        t = new_t;
        new_t = temp_t;
        let temp_r = r - quotient * new_r;
        r = new_r;
        new_r = temp_r;
    }

    if r > 1 {
        return None;
    }
    if t < 0 {
        t += phi as i128;
    }
    Some(t as u64)
}

/// 素性检验（试除法）
pub fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    if n < 4 {
        return true;
    }
    if n.is_multiple_of(2) {
        return false;
    }
    let mut i = 3u64;
    while i.saturating_mul(i) <= n {
        if n.is_multiple_of(i) {
            return false;
        }
        i += 2;
    }
    true
}

/// 将字节流编码为十六进制字符串
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 返回当前 Unix 时间戳（秒）
pub fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 生成测试用审计条目序列
pub fn generate_test_entries(count: usize) -> Vec<AuditEntry> {
    let actions = ["INSERT", "UPDATE", "DELETE", "SELECT", "CREATE"];
    let objects = ["users", "orders", "products", "logs", "config"];
    let users = ["alice", "bob", "charlie", "dave", "eve"];

    (0..count)
        .map(|i| {
            let idx = i % 5;
            AuditEntry::new(
                i as u64,
                1700000000 + i as u64,
                users[idx],
                actions[idx],
                objects[idx],
                format!("operation #{}", i),
            )
        })
        .collect()
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- AuditEntry ---

    #[test]
    fn test_audit_entry_new() {
        let entry = AuditEntry::new(1, 1700000000, "alice", "INSERT", "users", "add user");
        assert_eq!(entry.seq, 1);
        assert_eq!(entry.timestamp, 1700000000);
        assert_eq!(entry.user, "alice");
        assert_eq!(entry.action, "INSERT");
        assert_eq!(entry.object, "users");
        assert_eq!(entry.detail, "add user");
    }

    #[test]
    fn test_audit_entry_to_bytes_stable() {
        let e1 = AuditEntry::new(1, 100, "u", "a", "o", "d");
        let e2 = AuditEntry::new(1, 100, "u", "a", "o", "d");
        assert_eq!(e1.to_bytes(), e2.to_bytes());
    }

    #[test]
    fn test_audit_entry_to_bytes_distinguishes_different() {
        let e1 = AuditEntry::new(1, 100, "u", "a", "o", "d1");
        let e2 = AuditEntry::new(1, 100, "u", "a", "o", "d2");
        assert_ne!(e1.to_bytes(), e2.to_bytes());
    }

    #[test]
    fn test_audit_entry_display() {
        let entry = AuditEntry::new(1, 1700000000, "alice", "INSERT", "users", "add");
        let s = format!("{}", entry);
        assert!(s.contains("alice"));
        assert!(s.contains("INSERT"));
        assert!(s.contains("users"));
    }

    // --- AuditHashChain ---

    #[test]
    fn test_chain_new_empty() {
        let chain = AuditHashChain::new();
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
        assert_eq!(chain.root_hash(), INITIAL_HASH);
    }

    #[test]
    fn test_chain_append_increments_length() {
        let mut chain = AuditHashChain::new();
        chain.append(AuditEntry::new(0, 100, "u", "a", "o", "d"));
        assert_eq!(chain.len(), 1);
        assert!(!chain.is_empty());
    }

    #[test]
    fn test_chain_append_multiple() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(10) {
            chain.append(entry);
        }
        assert_eq!(chain.len(), 10);
    }

    #[test]
    fn test_chain_verify_empty() {
        let chain = AuditHashChain::new();
        assert!(chain.verify().is_ok());
    }

    #[test]
    fn test_chain_verify_single_entry() {
        let mut chain = AuditHashChain::new();
        chain.append(AuditEntry::new(0, 100, "u", "a", "o", "d"));
        assert!(chain.verify().is_ok());
    }

    #[test]
    fn test_chain_verify_multiple_entries() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(20) {
            chain.append(entry);
        }
        assert!(chain.verify().is_ok());
    }

    #[test]
    fn test_chain_verify_detects_tampered_detail() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(10) {
            chain.append(entry);
        }
        assert!(chain.verify().is_ok());
        chain.tamper_detail(5, "tampered");
        let result = chain.verify();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), 5);
    }

    #[test]
    fn test_chain_verify_detects_tampered_hash() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(5) {
            chain.append(entry);
        }
        assert!(chain.verify().is_ok());
        chain.tamper_hash(2);
        let result = chain.verify();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), 2);
    }

    #[test]
    fn test_chain_root_hash_changes_on_append() {
        let mut chain = AuditHashChain::new();
        let root0 = chain.root_hash();
        chain.append(AuditEntry::new(0, 100, "u", "a", "o", "d1"));
        let root1 = chain.root_hash();
        assert_ne!(root0, root1);
        chain.append(AuditEntry::new(1, 101, "u", "a", "o", "d2"));
        let root2 = chain.root_hash();
        assert_ne!(root1, root2);
    }

    #[test]
    fn test_chain_entries_returns_all() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(5) {
            chain.append(entry);
        }
        let entries = chain.entries();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].entry.seq, 0);
        assert_eq!(entries[4].entry.seq, 4);
    }

    #[test]
    fn test_chain_clear() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(5) {
            chain.append(entry);
        }
        assert_eq!(chain.len(), 5);
        chain.clear();
        assert_eq!(chain.len(), 0);
        assert!(chain.is_empty());
    }

    #[test]
    fn test_chain_entry_hash_hex() {
        let mut chain = AuditHashChain::new();
        chain.append(AuditEntry::new(0, 100, "u", "a", "o", "d"));
        let entry = &chain.entries()[0];
        let hex = entry.hash_hex();
        assert_eq!(hex.len(), HASH_LEN * 2);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_chain_entry_prev_hash_hex_first_is_zero() {
        let mut chain = AuditHashChain::new();
        chain.append(AuditEntry::new(0, 100, "u", "a", "o", "d"));
        let entry = &chain.entries()[0];
        assert_eq!(entry.prev_hash, INITIAL_HASH);
        assert_eq!(entry.prev_hash_hex(), "0".repeat(HASH_LEN * 2));
    }

    // --- RsaKeyPair ---

    #[test]
    fn test_rsa_key_pair_from_primes() {
        let kp = RsaKeyPair::from_primes(10007, 10009, 65537);
        assert_eq!(kp.public_key.n, 10007 * 10009);
        assert_eq!(kp.public_key.e, 65537);
        let phi = (10007 - 1) * (10009 - 1);
        assert_eq!((kp.private_key.d * 65537) % phi, 1);
    }

    #[test]
    fn test_rsa_key_pair_generate_demo() {
        let kp = RsaKeyPair::generate_demo();
        assert_eq!(kp.public_key.n, 10007 * 10009);
        assert_eq!(kp.public_key.e, 65537);
    }

    #[test]
    #[should_panic(expected = "p must be prime")]
    fn test_rsa_key_pair_non_prime_p_panics() {
        let _ = RsaKeyPair::from_primes(4, 10009, 65537);
    }

    #[test]
    #[should_panic(expected = "q must be prime")]
    fn test_rsa_key_pair_non_prime_q_panics() {
        let _ = RsaKeyPair::from_primes(10007, 4, 65537);
    }

    #[test]
    #[should_panic(expected = "p and q must be distinct")]
    fn test_rsa_key_pair_same_primes_panics() {
        let _ = RsaKeyPair::from_primes(10007, 10007, 65537);
    }

    // --- RsaSigner ---

    #[test]
    fn test_rsa_signer_sign_and_verify() {
        let signer = RsaSigner::demo();
        let hash = sha256(b"test audit report");
        let sig = signer.sign_hash(&hash);
        assert!(!sig.is_empty());
        assert!(signer.verify_hash(&sig));
    }

    #[test]
    fn test_rsa_signer_verify_with_public_key() {
        let signer = RsaSigner::demo();
        let hash = sha256(b"test report");
        let sig = signer.sign_hash(&hash);
        let public_key = signer.public_key().clone();
        assert!(RsaSigner::verify_with_public_key(&sig, &public_key));
    }

    #[test]
    fn test_rsa_signer_verify_tampered_signature() {
        let signer = RsaSigner::demo();
        let hash = sha256(b"test report");
        let mut sig = signer.sign_hash(&hash);
        sig.signature = (sig.signature + 1) % signer.public_key().n;
        assert!(!signer.verify_hash(&sig));
    }

    #[test]
    fn test_rsa_signer_verify_tampered_hash() {
        let signer = RsaSigner::demo();
        let hash = sha256(b"test report");
        let mut sig = signer.sign_hash(&hash);
        sig.signed_hash = sig.signed_hash.wrapping_add(1);
        assert!(!signer.verify_hash(&sig));
    }

    #[test]
    fn test_rsa_signature_is_empty() {
        let empty = RsaSignature {
            signature: 0,
            signed_hash: 0,
        };
        assert!(empty.is_empty());

        let non_empty = RsaSignature {
            signature: 1,
            signed_hash: 0,
        };
        assert!(!non_empty.is_empty());
    }

    // --- AuditReport ---

    #[test]
    fn test_report_from_chain_empty() {
        let chain = AuditHashChain::new();
        let report = AuditReport::from_chain(&chain);
        assert_eq!(report.entry_count, 0);
        assert_eq!(report.root_hash, INITIAL_HASH);
        assert!(!report.is_signed());
    }

    #[test]
    fn test_report_from_chain_with_entries() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(10) {
            chain.append(entry);
        }
        let report = AuditReport::from_chain(&chain);
        assert_eq!(report.entry_count, 10);
        assert_ne!(report.root_hash, INITIAL_HASH);
        assert!(!report.is_signed());
    }

    #[test]
    fn test_report_sign() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(5) {
            chain.append(entry);
        }
        let signer = RsaSigner::demo();
        let mut report = AuditReport::from_chain(&chain);
        assert!(!report.is_signed());
        report.sign(&signer);
        assert!(report.is_signed());
    }

    #[test]
    fn test_report_verify_signature_signed() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(5) {
            chain.append(entry);
        }
        let signer = RsaSigner::demo();
        let mut report = AuditReport::from_chain(&chain);
        report.sign(&signer);
        assert!(report.verify_signature(signer.public_key()));
    }

    #[test]
    fn test_report_verify_signature_unsigned_returns_false() {
        let chain = AuditHashChain::new();
        let report = AuditReport::from_chain(&chain);
        let public_key = RsaKeyPair::generate_demo().public_key;
        assert!(!report.verify_signature(&public_key));
    }

    #[test]
    fn test_report_verify_signature_wrong_key_returns_false() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(5) {
            chain.append(entry);
        }
        let signer = RsaSigner::demo();
        let mut report = AuditReport::from_chain(&chain);
        report.sign(&signer);
        let wrong_kp = RsaKeyPair::from_primes(10037, 10039, 65537);
        assert!(!report.verify_signature(&wrong_kp.public_key));
    }

    #[test]
    fn test_report_to_bytes_signed() {
        let mut chain = AuditHashChain::new();
        chain.append(AuditEntry::new(0, 100, "u", "a", "o", "d"));
        let signer = RsaSigner::demo();
        let mut report = AuditReport::from_chain(&chain);
        report.sign(&signer);
        let bytes = report.to_bytes();
        assert_eq!(bytes.len(), 8 + 32 + 8 + 1 + 8 + 8);
        assert_eq!(bytes[48], 1);
    }

    #[test]
    fn test_report_to_bytes_unsigned() {
        let chain = AuditHashChain::new();
        let report = AuditReport::from_chain(&chain);
        let bytes = report.to_bytes();
        assert_eq!(bytes.len(), 8 + 32 + 8 + 1);
        assert_eq!(bytes[48], 0);
    }

    // --- AuditSystem ---

    #[test]
    fn test_system_new() {
        let sys = AuditSystem::new();
        assert_eq!(sys.entry_count(), 0);
        assert!(sys.verify_chain().is_ok());
    }

    #[test]
    fn test_system_record() {
        let mut sys = AuditSystem::new();
        let seq0 = sys.record("alice", "INSERT", "users", "add user 1");
        let seq1 = sys.record("bob", "UPDATE", "orders", "update order 2");
        assert_eq!(seq0, 0);
        assert_eq!(seq1, 1);
        assert_eq!(sys.entry_count(), 2);
    }

    #[test]
    fn test_system_verify_chain_after_records() {
        let mut sys = AuditSystem::new();
        for i in 0..20 {
            sys.record("alice", "INSERT", "users", format!("add user {}", i));
        }
        assert!(sys.verify_chain().is_ok());
    }

    #[test]
    fn test_system_generate_report_signed() {
        let mut sys = AuditSystem::new();
        for i in 0..10 {
            sys.record("alice", "INSERT", "users", format!("op {}", i));
        }
        let report = sys.generate_report();
        assert_eq!(report.entry_count, 10);
        assert!(report.is_signed());
        assert!(sys.verify_report(&report));
    }

    #[test]
    fn test_system_clear() {
        let mut sys = AuditSystem::new();
        for i in 0..5 {
            sys.record("u", "a", "o", format!("d{}", i));
        }
        assert_eq!(sys.entry_count(), 5);
        sys.clear();
        assert_eq!(sys.entry_count(), 0);
    }

    #[test]
    fn test_system_with_custom_key_pair() {
        let kp = RsaKeyPair::from_primes(10037, 10039, 65537);
        let mut sys = AuditSystem::with_key_pair(kp);
        sys.record("alice", "INSERT", "users", "test");
        let report = sys.generate_report();
        assert!(sys.verify_report(&report));
    }

    // --- 完整流程集成测试 ---

    #[test]
    fn test_integration_full_workflow() {
        let mut sys = AuditSystem::new();
        for i in 0..50 {
            sys.record("alice", "INSERT", "users", format!("operation #{}", i));
        }
        assert!(sys.verify_chain().is_ok());
        let report = sys.generate_report();
        assert_eq!(report.entry_count, 50);
        assert!(report.is_signed());
        assert!(sys.verify_report(&report));
        let public_key = sys.public_key().clone();
        assert!(report.verify_signature(&public_key));
    }

    // --- 辅助函数 ---

    #[test]
    fn test_sha256_known_value() {
        let hash = sha256(b"abc");
        let expected_hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(hex_encode(&hash), expected_hex);
    }

    #[test]
    fn test_hash_to_u64_takes_first_8_bytes() {
        let mut hash = [0u8; 32];
        hash[..8].copy_from_slice(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
        let result = hash_to_u64(&hash);
        assert_eq!(result, 0x0102030405060708);
    }

    #[test]
    fn test_mod_pow_simple() {
        assert_eq!(mod_pow(2, 10, 1000), 24);
        assert_eq!(mod_pow(3, 5, 7), 5);
    }

    #[test]
    fn test_mod_pow_identity() {
        assert_eq!(mod_pow(12345, 1, 1000), 345);
    }

    #[test]
    fn test_mod_pow_zero_exponent() {
        assert_eq!(mod_pow(12345, 0, 1000), 1);
    }

    #[test]
    fn test_mod_pow_modulus_one() {
        assert_eq!(mod_pow(12345, 100, 1), 0);
    }

    #[test]
    fn test_mod_inverse_coprime() {
        assert_eq!(mod_inverse(3, 11), Some(4));
    }

    #[test]
    fn test_mod_inverse_not_coprime() {
        assert_eq!(mod_inverse(2, 4), None);
    }

    #[test]
    fn test_mod_inverse_rsa_e_phi() {
        let p = 10007u64;
        let q = 10009u64;
        let e = 65537u64;
        let phi = (p - 1) * (q - 1);
        let d = mod_inverse(e, phi).unwrap();
        assert_eq!((d * e) % phi, 1);
    }

    #[test]
    fn test_is_prime_small_primes() {
        assert!(is_prime(2));
        assert!(is_prime(3));
        assert!(is_prime(5));
        assert!(is_prime(7));
        assert!(is_prime(11));
        assert!(is_prime(13));
        assert!(is_prime(10007));
        assert!(is_prime(10009));
    }

    #[test]
    fn test_is_prime_composites() {
        assert!(!is_prime(0));
        assert!(!is_prime(1));
        assert!(!is_prime(4));
        assert!(!is_prime(6));
        assert!(!is_prime(9));
        assert!(!is_prime(15));
        assert!(!is_prime(100));
    }

    #[test]
    fn test_hex_encode_empty() {
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn test_hex_encode_known() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0xab]), "00ffab");
        assert_eq!(hex_encode(&[0x01, 0x02, 0x03]), "010203");
    }

    #[test]
    fn test_current_timestamp_nonzero() {
        let ts = current_timestamp();
        assert!(ts > 1700000000);
    }

    #[test]
    fn test_generate_test_entries_count() {
        let entries = generate_test_entries(10);
        assert_eq!(entries.len(), 10);
        assert_eq!(entries[0].seq, 0);
        assert_eq!(entries[9].seq, 9);
    }

    // --- 边界测试 ---

    #[test]
    fn test_empty_chain_root_hash_is_initial() {
        let chain = AuditHashChain::new();
        assert_eq!(chain.root_hash(), INITIAL_HASH);
    }

    #[test]
    fn test_single_entry_chain_verify() {
        let mut chain = AuditHashChain::new();
        chain.append(AuditEntry::new(0, 0, "", "", "", ""));
        assert!(chain.verify().is_ok());
    }

    #[test]
    fn test_tamper_first_entry_detected() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(5) {
            chain.append(entry);
        }
        chain.tamper_detail(0, "tampered");
        assert!(chain.verify().is_err());
    }

    #[test]
    fn test_tamper_last_entry_detected() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(5) {
            chain.append(entry);
        }
        chain.tamper_detail(4, "tampered");
        assert!(chain.verify().is_err());
    }

    #[test]
    fn test_chain_with_empty_strings() {
        let mut chain = AuditHashChain::new();
        chain.append(AuditEntry::new(0, 0, "", "", "", ""));
        chain.append(AuditEntry::new(1, 0, "", "", "", ""));
        chain.append(AuditEntry::new(2, 0, "", "", "", ""));
        assert!(chain.verify().is_ok());
    }

    #[test]
    fn test_rsa_sign_different_hashes_different_signatures() {
        let signer = RsaSigner::demo();
        let hash1 = sha256(b"report 1");
        let hash2 = sha256(b"report 2");
        let sig1 = signer.sign_hash(&hash1);
        let sig2 = signer.sign_hash(&hash2);
        assert_ne!(sig1.signature, sig2.signature);
        assert_ne!(sig1.signed_hash, sig2.signed_hash);
    }

    #[test]
    fn test_rsa_sign_same_hash_same_signature() {
        let signer = RsaSigner::demo();
        let hash = sha256(b"same report");
        let sig1 = signer.sign_hash(&hash);
        let sig2 = signer.sign_hash(&hash);
        assert_eq!(sig1.signature, sig2.signature);
    }

    #[test]
    fn test_system_record_increments_seq() {
        let mut sys = AuditSystem::new();
        assert_eq!(sys.record("u", "a", "o", "d1"), 0);
        assert_eq!(sys.record("u", "a", "o", "d2"), 1);
        assert_eq!(sys.record("u", "a", "o", "d3"), 2);
        assert_eq!(sys.entry_count(), 3);
    }

    #[test]
    fn test_system_clear_resets_seq() {
        let mut sys = AuditSystem::new();
        for i in 0..5 {
            sys.record("u", "a", "o", format!("d{}", i));
        }
        sys.clear();
        assert_eq!(sys.record("u", "a", "o", "new"), 0);
    }

    #[test]
    fn test_large_chain_verify() {
        let mut chain = AuditHashChain::new();
        for entry in generate_test_entries(1000) {
            chain.append(entry);
        }
        assert_eq!(chain.len(), 1000);
        assert!(chain.verify().is_ok());
    }
}
