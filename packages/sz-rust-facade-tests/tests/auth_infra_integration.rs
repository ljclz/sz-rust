//! P9-FACADE-03：auth + infra + orm(jwt) 集成测试
//!
//! 验证 `sz-rust-auth-facade`（微信签名验证 / 网关）、
//! `sz-rust-infra-facade`（路径安全 / MIME / 上传命名）、
//! `sz-rust-orm-facade::jwt`（JWT 签发与验证）的协作。

use std::sync::Arc;

use sha1::Digest;

use sz_rust_auth_facade::wechat::{
    MemoryWechatHttpTransport, WechatAppType, WechatConfig, WechatSdk,
};
use sz_rust_infra_facade::static_files::{is_path_safe, mime_type_for_extension};
use sz_rust_orm_facade::jwt::{JwtClaims, JwtEncoder};

/// 微信签名验证 + JWT 签发/验证（两种认证机制并行协作）
#[test]
fn wechat_signature_and_jwt_auth_chain() {
    // 1. auth-facade：微信服务器回调签名验证
    let config = WechatConfig::new(WechatAppType::OfficialAccount, "wx_app_001", "secret_001")
        .with_token("sz_test_token_2026");
    let sdk = WechatSdk::new(config, Arc::new(MemoryWechatHttpTransport::new()));

    let ts = "1754150400";
    let nonce = "88481803";
    let token = "sz_test_token_2026";
    // 对齐实现：SHA1(token + timestamp + nonce)
    let mut hasher = sha1::Sha1::new();
    hasher.update(token.as_bytes());
    hasher.update(ts.as_bytes());
    hasher.update(nonce.as_bytes());
    let correct_sig = hex::encode(hasher.finalize());

    assert!(
        sdk.verify_signature(&correct_sig, ts, nonce, token),
        "P9-FACADE-03: 正确签名应验证通过"
    );
    assert!(
        !sdk.verify_signature("deadbeefdeadbeef", ts, nonce, token),
        "P9-FACADE-03: 篡改签名应验证失败"
    );

    // 2. orm-facade::jwt：同一业务流签发 JWT 并验证
    let encoder = JwtEncoder::new("sz-jwt-secret");
    let jwt = encoder
        .encode(&JwtClaims::new("u_1001", 4_000_000_000_i64).with_user_id(1001))
        .unwrap();
    let claims = encoder.decode(&jwt).unwrap();
    assert_eq!(claims.sub, "u_1001", "P9-FACADE-03: JWT 主体应与签发一致");
    assert!(
        !claims.is_expired(),
        "P9-FACADE-03: 未过期 token 不应判定过期"
    );
}

/// infra 路径安全 + MIME 识别（静态资源防护与类型识别）
#[test]
fn path_safety_and_mime_detection() {
    // 1. 路径穿越防护（规则 8：路径归一化）
    let root = tempfile::tempdir().unwrap();
    let public = root.path().join("public");
    std::fs::create_dir(&public).unwrap();
    let safe_file = public.join("avatar.png");
    std::fs::write(&safe_file, b"fake-png-bytes").unwrap();

    assert!(
        is_path_safe(&safe_file, &public),
        "P9-FACADE-03: 目录内合法路径应通过"
    );
    assert!(
        !is_path_safe(
            &public.join("..").join("..").join("etc").join("passwd"),
            &public
        ),
        "P9-FACADE-03: 越界路径应被拒绝"
    );

    // 2. MIME 类型识别
    assert_eq!(
        mime_type_for_extension("png"),
        Some("image/png"),
        "P9-FACADE-03: png 扩展名应识别为 image/png"
    );
    assert_eq!(
        mime_type_for_extension("html"),
        Some("text/html"),
        "P9-FACADE-03: html 扩展名应识别为 text/html"
    );
}

/// upload 保存文件名生成（infra-facade）—— 供 auth 头像等上传业务复用
#[test]
fn upload_save_name_generation() {
    use std::path::Path;
    use sz_rust_infra_facade::upload::storage::build_save_name;

    let name = build_save_name(Path::new("/tmp/upload_20260803.bin"), "png");
    // 对齐 PHP：storage/{Ymd}/{YmdHis}{md5[0..5]}{rand}.{ext}
    assert!(
        name.starts_with("storage/"),
        "P9-FACADE-03: 保存名应以 storage/ 开头"
    );
    assert!(name.ends_with(".png"), "P9-FACADE-03: 保存名应以 .png 结尾");
    assert!(
        name.len() > "storage/20260803/".len() + 6,
        "P9-FACADE-03: 保存名应包含时间戳 + 随机后缀"
    );
}
