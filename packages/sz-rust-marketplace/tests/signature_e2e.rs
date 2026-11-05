// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 插件市场全链路验签集成测试
//!
//! 闭合架构师复评报告（docs/audit/2026-11-04-系统架构师复评报告.md）第 56 行扣分项
//! 与 Phase 2 #9 验收标准（第 143 行）：「插件签名密钥管理：PLUGIN_PUBLIC_KEY 正式注入
//! + 插件包哈希 + 签名 CI → 插件市场全链路验签测试通过」。
//!
//! 覆盖全链路：发布者签名 → SHA256 哈希完整性 → 客户端验签 → fail-closed 负例。
//! 纯内存测试，不依赖数据库，可由 marketplace-ci.yml 的 signature-verify job 独立运行
//! `cargo test -p sz-rust-marketplace --test signature_e2e`。

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{SigningKey, VerifyingKey};

use sz_rust_marketplace::signature::SignatureService;

/// 由种子确定性生成 Ed25519 签名密钥（测试用，避免随机性导致用例不稳定）
fn keypair_from_seed(seed: [u8; 32]) -> SigningKey {
    SigningKey::from_bytes(&seed)
}

#[test]
fn e2e_sign_checksum_verify_roundtrip() {
    // 全链路正例：sign → sha256 checksum → verify
    let signing_key = keypair_from_seed([7u8; 32]);
    let public_key = signing_key.verifying_key();

    let package = b"plugin-wasm-bytes-v1.0.0";
    let signature = SignatureService::sign(package, &signing_key);
    let checksum = SignatureService::sha256_checksum(package);

    // 哈希完整性：64 位十六进制 + 相同包哈希稳定
    assert_eq!(checksum.len(), 64, "SHA256 哈希应为 64 位十六进制");
    assert_eq!(
        checksum,
        SignatureService::sha256_checksum(package),
        "相同包哈希应稳定"
    );

    // 验签通过
    let result = SignatureService::verify(package, &signature, &public_key);
    assert!(result.is_ok(), "全链路验签应通过, 实际: {:?}", result);
}

#[test]
fn e2e_tampered_package_rejected() {
    // 负例：篡改插件包内容 → 验签失败
    let signing_key = keypair_from_seed([11u8; 32]);
    let public_key = signing_key.verifying_key();

    let original = b"original package content";
    let signature = SignatureService::sign(original, &signing_key);

    let tampered = b"tampered package content";
    let result = SignatureService::verify(tampered, &signature, &public_key);
    assert!(result.is_err(), "篡改包应验签失败（fail-closed）");
}

#[test]
fn e2e_tampered_signature_rejected() {
    // 负例：篡改签名最后一个字节 → 验签失败
    let signing_key = keypair_from_seed([13u8; 32]);
    let public_key = signing_key.verifying_key();
    let package = b"package bytes";

    let mut sig_bytes = B64
        .decode(SignatureService::sign(package, &signing_key))
        .expect("合法签名应可 Base64 解码");
    // 翻转最后一字节，破坏签名
    sig_bytes[63] ^= 0xff;
    let tampered_signature = B64.encode(sig_bytes);

    let result = SignatureService::verify(package, &tampered_signature, &public_key);
    assert!(result.is_err(), "篡改签名应验签失败（fail-closed）");
}

#[test]
fn e2e_wrong_public_key_rejected() {
    // 负例：用错误公钥验签 → 失败（防止公钥替换攻击）
    let signing_key_a = keypair_from_seed([17u8; 32]);
    let signing_key_b = keypair_from_seed([19u8; 32]);
    let public_key_b = signing_key_b.verifying_key();

    let package = b"signed by key a";
    let signature = SignatureService::sign(package, &signing_key_a);

    let result = SignatureService::verify(package, &signature, &public_key_b);
    assert!(result.is_err(), "用错误公钥验签应失败（fail-closed）");
}

#[test]
fn e2e_empty_signature_fail_closed() {
    // 负例：空签名 → fail-closed 拒绝（不接受空签名绕过）
    let signing_key = keypair_from_seed([23u8; 32]);
    let public_key = signing_key.verifying_key();
    let package = b"some package";

    let result = SignatureService::verify(package, "", &public_key);
    assert!(result.is_err(), "空签名应 fail-closed 拒绝");
}

#[test]
fn e2e_signature_length_not_64_rejected() {
    // 负例：签名长度非 64 字节 → 拒绝（长度校验是 fail-closed 第一道防线）
    let signing_key = keypair_from_seed([29u8; 32]);
    let public_key = signing_key.verifying_key();
    let package = b"package";

    // 63 字节签名（长度不足）
    let short_sig = B64.encode(vec![0u8; 63]);
    let result = SignatureService::verify(package, &short_sig, &public_key);
    assert!(result.is_err(), "非 64 字节签名应拒绝（fail-closed）");
}

#[test]
fn e2e_invalid_base64_signature_rejected() {
    // 负例：非法 Base64 → 拒绝
    let signing_key = keypair_from_seed([31u8; 32]);
    let public_key = signing_key.verifying_key();

    let result = SignatureService::verify(b"data", "!!!not base64!!!", &public_key);
    assert!(result.is_err(), "非法 Base64 签名应拒绝（fail-closed）");
}

#[test]
fn e2e_checksum_distinguishes_packages() {
    // 哈希区分性：不同包 → 不同哈希
    let pkg_a = b"package A";
    let pkg_b = b"package B";
    let checksum_a = SignatureService::sha256_checksum(pkg_a);
    let checksum_b = SignatureService::sha256_checksum(pkg_b);
    assert_ne!(checksum_a, checksum_b, "不同包哈希应不同");
    assert_eq!(checksum_a.len(), 64);
    assert_eq!(checksum_b.len(), 64);
}

#[test]
fn e2e_pubkey_fingerprint_stable_and_distinct() {
    // 公钥指纹：稳定 + 不同密钥不同指纹（用于可信来源比对）
    let key_a = keypair_from_seed([37u8; 32]);
    let key_b = keypair_from_seed([41u8; 32]);
    let fp_a = SignatureService::pubkey_fingerprint(&key_a.verifying_key());
    let fp_b = SignatureService::pubkey_fingerprint(&key_b.verifying_key());

    assert_eq!(fp_a.len(), 64, "指纹应为 64 位十六进制");
    assert_eq!(
        fp_a,
        SignatureService::pubkey_fingerprint(&key_a.verifying_key()),
        "指纹应稳定"
    );
    assert_ne!(fp_a, fp_b, "不同密钥指纹应不同");
}

#[test]
fn e2e_full_pipeline_simulating_install_verify() {
    // 模拟插件安装全链路：发布者签名 → 哈希校验 → 客户端验签 → 公钥指纹比对
    let publisher_key = keypair_from_seed([43u8; 32]);
    let publisher_pubkey: VerifyingKey = publisher_key.verifying_key();

    // 1. 发布者打包并签名
    let plugin_wasm = b"\x00asm\x01\x00\x00\x00plugin body";
    let signature = SignatureService::sign(plugin_wasm, &publisher_key);
    let published_checksum = SignatureService::sha256_checksum(plugin_wasm);

    // 2. 客户端下载后重新计算哈希，校验完整性
    let downloaded_checksum = SignatureService::sha256_checksum(plugin_wasm);
    assert_eq!(
        published_checksum, downloaded_checksum,
        "下载包哈希应与发布哈希一致（完整性校验）"
    );

    // 3. 客户端用发布者公钥验签
    let verify_result = SignatureService::verify(plugin_wasm, &signature, &publisher_pubkey);
    assert!(
        verify_result.is_ok(),
        "客户端验签应通过, 实际: {:?}",
        verify_result
    );

    // 4. 公钥指纹用于可信来源比对
    let fingerprint = SignatureService::pubkey_fingerprint(&publisher_pubkey);
    assert_eq!(fingerprint.len(), 64, "指纹应为 64 位十六进制");
}