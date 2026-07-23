//! SZ-300 API 性能基准测试
//!
//! 运行方式:
//!   cargo bench --package sz-rust-sz300
//!
//! 前置条件:
//!   需要在 Cargo.toml 的 [dev-dependencies] 中添加:
//!   criterion = { version = "3.1", features = ["async_futures"] }
//!   tokio = { workspace = true, features = ["rt", "macros"] }

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::{json, Value};
use sz_orm_auth::{Credentials, JwtAuthenticator};

// ─── 辅助函数 ───────────────────────────────────────────────────────────────

/// 构建基准测试用的 JWT Authenticator 实例（每次调用新建，避免 OnceLock 干扰）
fn build_auth() -> JwtAuthenticator {
    JwtAuthenticator::new("bench-test-secret-key-00000000000000000000", "sz300", 86400)
}

/// 生成一个有效的 JWT token 供 verify 基准测试使用
fn create_test_token(auth: &JwtAuthenticator) -> String {
    let creds = Credentials::new("bench-user", "bench-pass");
    //  authenticate 在无 PasswordVerifier 时会返回 AuthError，
    //  所以直接调用底层 encoder 生成 token
    let claims = sz_orm_auth::auth::Claims::new("bench-user")
        .with_roles(vec!["admin".into()])
        .with_permissions(vec!["*".into()]);
    // 直接从 encoder 编码
    let encoder = sz_orm_auth::jwt::JwtEncoder::new("bench-test-secret-key-00000000000000000000");
    encoder
        .encode(&sz_orm_auth::jwt::JwtClaims::new("bench-user", 9999999999))
        .expect("token 生成失败")
}

// ─── 基准测试: 健康检查响应反序列化 ──────────────────────────────────────

fn bench_health_deserialize(c: &mut Criterion) {
    let response_json = json!({
        "code": 1,
        "msg": "success",
        "data": {
            "status": "ok",
            "version": "0.1.0",
            "service": "sz300-server",
            "timestamp": 1771257600
        }
    });
    let json_str = serde_json::to_string(&response_json).expect("JSON 序列化失败");

    c.bench_function("health/deserialize_response", |b| {
        b.iter(|| {
            let _: Value = serde_json::from_str(black_box(&json_str)).expect("反序列化失败");
        })
    });
}

// ─── 基准测试: JWT Token 验证 ─────────────────────────────────────────────

fn bench_jwt_verify(c: &mut Criterion) {
    let auth = build_auth();
    let token = create_test_token(&auth);

    c.bench_function("jwt/verify_token", |b| {
        b.iter(|| {
            let result = auth.verify_token(black_box(&token));
            assert!(result.is_ok(), "token 验证失败: {:?}", result.err());
        })
    });
}

fn bench_jwt_authenticate(c: &mut Criterion) {
    let auth = build_auth();
    let creds = Credentials::new("bench-user", "bench-pass");

    c.bench_function("jwt/authenticate_credentials", |b| {
        b.iter(|| {
            // authenticate 内部走 PasswordVerifier，无 verifier 时会返回 error，
            // 这里主要 benchmark 调用路径本身的 overhead
            let _ = auth.authenticate(black_box(&creds));
        })
    });
}

// ─── 基准测试: 文件服务 URL 生成 ──────────────────────────────────────────

fn bench_file_url_generation(c: &mut Criterion) {
    let filename = "product_photo.jpg";
    let ext = "jpg";

    c.bench_function("file/generate_url", |b| {
        b.iter(|| {
            let date_str = chrono::Local::now().format("%Y/%m/%d").to_string();
            let new_name = format!(
                "{}_{}.{}",
                chrono::Local::now().format("%Y%m%d_%H%M%S"),
                "a1b2c3d4",
                ext
            );
            let url = format!("/uploads/{}/{}", date_str, new_name);
            black_box(url);
        })
    });
}

fn bench_file_extension_check(c: &mut Criterion) {
    let allowed_extensions: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp"];
    let filenames = vec![
        "photo.jpg",
        "image.jpeg",
        "screenshot.png",
        "animation.gif",
        "bitmap.bmp",
        "document.pdf", // 不合法，测试快速失败路径
    ];

    c.bench_function("file/extension_check", |b| {
        b.iter(|| {
            for name in &filenames {
                let ext = name
                    .rfind('.')
                    .and_then(|pos| name.get(pos + 1..))
                    .unwrap_or("");
                let valid = allowed_extensions.contains(&ext);
                black_box(valid);
            }
        })
    });
}

// ─── 基准测试: JSON 序列化 ────────────────────────────────────────────────

fn bench_json_serialize(c: &mut Criterion) {
    let response = json!({
        "code": 1,
        "msg": "success",
        "data": {
            "status": "ok",
            "version": "0.1.0",
            "service": "sz300-server",
            "timestamp": 1771257600
        }
    });

    c.bench_function("json/serialize_health_response", |b| {
        b.iter(|| {
            let _ = serde_json::to_string(black_box(&response)).expect("序列化失败");
        })
    });
}

// ─── 基准测试分组 ─────────────────────────────────────────────────────────

criterion_group!(
    name = health;
    config = Criterion::default().sample_size(100).warm_up_time(std::time::Duration::from_secs(2));
    targets = bench_health_deserialize, bench_json_serialize
);

criterion_group!(
    name = jwt;
    config = Criterion::default().sample_size(100).warm_up_time(std::time::Duration::from_secs(2));
    targets = bench_jwt_verify, bench_jwt_authenticate
);

criterion_group!(
    name = file;
    config = Criterion::default().sample_size(100).warm_up_time(std::time::Duration::from_secs(2));
    targets = bench_file_url_generation, bench_file_extension_check
);

criterion_main!(health, jwt, file);
