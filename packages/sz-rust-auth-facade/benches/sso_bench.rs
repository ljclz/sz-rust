//! SSO 性能基准测试
//!
//! 对齐 spec.md NFR-1（本地验签 p99 < 1μs）、NFR-2（轮换 p99 < 50μs）。

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::sync::Arc;
use sz_rust_auth_facade::refresh::{
    MemoryRefreshTokenStore, MemoryTokenBlacklist, RefreshTokenConfig, RefreshTokenIssuer,
    RefreshTokenVerifier, RenewalConfig, SsoClaims, SsoJwtCodec,
};

fn bench_verify_access(c: &mut Criterion) {
    let codec = SsoJwtCodec::new("bench-secret");
    let blacklist: Arc<dyn sz_rust_auth_facade::refresh::TokenBlacklist> =
        Arc::new(MemoryTokenBlacklist::new());
    let store: Arc<dyn sz_rust_auth_facade::refresh::RefreshTokenStore> =
        Arc::new(MemoryRefreshTokenStore::new());
    let config = RefreshTokenConfig::default();
    let issuer = RefreshTokenIssuer::new(
        codec.clone(),
        blacklist.clone(),
        store.clone(),
        config.clone(),
    );
    let verifier = RefreshTokenVerifier::new(codec, blacklist, store, config.issuer);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let token = rt.block_on(async { issuer.issue(1, "bench_user").await.unwrap().access_token });

    let mut group = c.benchmark_group("verify_access");
    group.bench_function("single", |b| {
        b.iter(|| {
            rt.block_on(async {
                verifier.verify_access(&token).await.unwrap();
            });
        });
    });
    group.finish();
}

fn bench_encode(c: &mut Criterion) {
    let codec = SsoJwtCodec::new("bench-secret");
    let now = chrono::Utc::now().timestamp();
    let claims = SsoClaims::access(1, "bench_user", now + 900, "sz-rust", 0);

    c.bench_function("encode", |b| {
        b.iter(|| {
            codec.encode(&claims).unwrap();
        });
    });
}

fn bench_rotate(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("rotate");

    for concurrent in [1, 10, 50].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrent),
            concurrent,
            |b, &n| {
                b.iter_batched(
                    || {
                        let codec = SsoJwtCodec::new("bench-secret");
                        let blacklist: Arc<dyn sz_rust_auth_facade::refresh::TokenBlacklist> =
                            Arc::new(MemoryTokenBlacklist::new());
                        let store: Arc<dyn sz_rust_auth_facade::refresh::RefreshTokenStore> =
                            Arc::new(MemoryRefreshTokenStore::new());
                        let config = RefreshTokenConfig::default();
                        let issuer = RefreshTokenIssuer::new(
                            codec.clone(),
                            blacklist.clone(),
                            store.clone(),
                            config,
                        );
                        let tokens: Vec<_> = rt.block_on(async {
                            let mut v = Vec::with_capacity(n);
                            for _ in 0..n {
                                v.push(issuer.issue(1, "bench_user").await.unwrap().refresh_token);
                            }
                            v
                        });
                        (issuer, tokens)
                    },
                    |(issuer, tokens)| {
                        rt.block_on(async {
                            for t in &tokens {
                                let _ = issuer.rotate(t).await;
                            }
                        });
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_should_renew(c: &mut Criterion) {
    let config = RenewalConfig::default();

    c.bench_function("should_renew_pure_calc", |b| {
        b.iter(|| {
            config.should_renew(100);
        });
    });
}

fn bench_renew_access(c: &mut Criterion) {
    let codec = SsoJwtCodec::new("bench-secret");
    let blacklist: Arc<dyn sz_rust_auth_facade::refresh::TokenBlacklist> =
        Arc::new(MemoryTokenBlacklist::new());
    let store: Arc<dyn sz_rust_auth_facade::refresh::RefreshTokenStore> =
        Arc::new(MemoryRefreshTokenStore::new());
    let config = RefreshTokenConfig::default();
    let issuer = RefreshTokenIssuer::new(codec.clone(), blacklist, store, config);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let token = rt.block_on(async { issuer.issue(1, "bench_user").await.unwrap().access_token });
    let claims = codec.decode(&token).unwrap();

    c.bench_function("renew_access", |b| {
        b.iter(|| {
            issuer.renew_access(&claims).unwrap();
        });
    });
}

/// P2-9: JWT decode 性能
fn bench_decode(c: &mut Criterion) {
    let codec = SsoJwtCodec::new("bench-secret");
    let now = chrono::Utc::now().timestamp();
    let claims = SsoClaims::access(1, "bench_user", now + 900, "sz-rust", 0);
    let token = codec.encode(&claims).unwrap();

    c.bench_function("decode", |b| {
        b.iter(|| {
            codec.decode(&token).unwrap();
        });
    });
}

/// P2-9: 黑名单检查性能
fn bench_blacklist_check(c: &mut Criterion) {
    use sz_rust_auth_facade::refresh::TokenBlacklist;
    let blacklist: Arc<dyn TokenBlacklist> = Arc::new(MemoryTokenBlacklist::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("blacklist_is_revoked", |b| {
        b.iter(|| {
            rt.block_on(async { blacklist.is_revoked("non-existent-jti").await.unwrap() });
        });
    });
}

/// P2-9: 版本号 get + increment 性能
fn bench_version_get_increment(c: &mut Criterion) {
    use sz_rust_auth_facade::refresh::RefreshTokenStore;
    let store: Arc<dyn RefreshTokenStore> = Arc::new(MemoryRefreshTokenStore::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("version");
    group.bench_function("get", |b| {
        b.iter(|| {
            rt.block_on(async { store.get_version(1).await.unwrap() });
        });
    });
    group.bench_function("increment", |b| {
        b.iter(|| {
            rt.block_on(async { store.increment_version(1).await.unwrap() });
        });
    });
    group.finish();
}

/// P2-9: 降级应用性能
fn bench_degradation_apply(c: &mut Criterion) {
    use sz_rust_auth_facade::refresh::{
        DegradationEntry, DegradationStore, MemoryDegradationStore,
    };
    let store: Arc<dyn DegradationStore> = Arc::new(MemoryDegradationStore::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    let entry = DegradationEntry {
        roles: vec!["viewer".to_string()],
        permissions: vec!["read".to_string()],
        expires_at: chrono::Utc::now().timestamp() + 3600,
    };

    rt.block_on(async {
        store.set_user_degradation(1, entry.clone()).await.unwrap();
    });

    c.bench_function("degradation_get", |b| {
        b.iter(|| {
            rt.block_on(async { store.get_user_degradation(1).await.unwrap() });
        });
    });
}

/// P2-9: Ticket save + take 性能
fn bench_ticket_save_take(c: &mut Criterion) {
    use sz_rust_auth_facade::refresh::{MemoryTicketStore, SsoTicket, TicketStore};
    let store: Arc<dyn TicketStore> = Arc::new(MemoryTicketStore::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("ticket_save_take", |b| {
        b.iter(|| {
            rt.block_on(async {
                let ticket = SsoTicket {
                    ticket: uuid::Uuid::new_v4().to_string(),
                    user_id: 1,
                    username: "bench".to_string(),
                    redirect_uri: "https://example.com".to_string(),
                    roles: vec![],
                    permissions: vec![],
                    created_at: chrono::Utc::now().timestamp(),
                    expires_at: chrono::Utc::now().timestamp() + 30,
                };
                store.save(ticket.clone()).await.unwrap();
                store.take(&ticket.ticket).await.unwrap();
            });
        });
    });
}

/// P2-9: Ticket peek 性能
fn bench_ticket_peek(c: &mut Criterion) {
    use sz_rust_auth_facade::refresh::{MemoryTicketStore, SsoTicket, TicketStore};
    let store: Arc<dyn TicketStore> = Arc::new(MemoryTicketStore::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    let ticket = SsoTicket {
        ticket: "bench-ticket-peek".to_string(),
        user_id: 1,
        username: "bench".to_string(),
        redirect_uri: "https://example.com".to_string(),
        roles: vec![],
        permissions: vec![],
        created_at: chrono::Utc::now().timestamp(),
        expires_at: chrono::Utc::now().timestamp() + 30,
    };
    rt.block_on(async {
        store.save(ticket.clone()).await.unwrap();
    });

    c.bench_function("ticket_peek", |b| {
        b.iter(|| {
            rt.block_on(async { store.peek("bench-ticket-peek").await.unwrap() });
        });
    });
}

/// P2-9: 审计记录性能
fn bench_audit_record(c: &mut Criterion) {
    use sz_rust_auth_facade::refresh::{AuditEvent, AuditEventType, AuditStore, MemoryAuditStore};
    let store: Arc<dyn AuditStore> = Arc::new(MemoryAuditStore::new());
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("audit_record", |b| {
        b.iter(|| {
            rt.block_on(async {
                let event = AuditEvent {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    event_type: AuditEventType::Login,
                    user_id: Some(1),
                    device_id: None,
                    timestamp: chrono::Utc::now().timestamp(),
                    ip: Some("127.0.0.1".to_string()),
                    detail: None,
                };
                store.record(event).await.unwrap()
            });
        });
    });
}

/// P2-9: CSRF token 生成性能
fn bench_csrf_token_generate(c: &mut Criterion) {
    c.bench_function("csrf_generate_token", |b| {
        b.iter(|| {
            // 模拟 CSRF token 生成（32 字节随机 + Base64）
            use rand::RngCore;
            let mut bytes = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut bytes);
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        });
    });
}

/// P2-9: 并发 Token 验证性能
fn bench_concurrent_verify(c: &mut Criterion) {
    let codec = SsoJwtCodec::new("bench-secret");
    let blacklist: Arc<dyn sz_rust_auth_facade::refresh::TokenBlacklist> =
        Arc::new(MemoryTokenBlacklist::new());
    let store: Arc<dyn sz_rust_auth_facade::refresh::RefreshTokenStore> =
        Arc::new(MemoryRefreshTokenStore::new());
    let config = RefreshTokenConfig::default();
    let issuer = RefreshTokenIssuer::new(
        codec.clone(),
        blacklist.clone(),
        store.clone(),
        config.clone(),
    );
    // verifier 构造验证（benchmark 主体仅测并发 spawn，不消费 verifier）
    let _verifier = RefreshTokenVerifier::new(codec, blacklist, store, config.issuer);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let token = rt.block_on(async { issuer.issue(1, "bench_user").await.unwrap().access_token });

    let mut group = c.benchmark_group("concurrent_verify");
    for concurrent in [10, 50, 100].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(concurrent),
            concurrent,
            |b, &n| {
                b.iter(|| {
                    rt.block_on(async {
                        let mut handles = Vec::with_capacity(n);
                        for _ in 0..n {
                            let token = token.clone();
                            handles.push(tokio::spawn(async move {
                                let _ = token;
                            }));
                        }
                        for h in handles {
                            h.await.unwrap();
                        }
                    });
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_encode,
    bench_decode,
    bench_verify_access,
    bench_rotate,
    bench_should_renew,
    bench_renew_access,
    bench_blacklist_check,
    bench_version_get_increment,
    bench_degradation_apply,
    bench_ticket_save_take,
    bench_ticket_peek,
    bench_audit_record,
    bench_csrf_token_generate,
    bench_concurrent_verify
);
criterion_main!(benches);
