// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! gRPC 支持（tonic + prost）
//!
//! 提供 gRPC 服务构建和鉴权拦截器。
//!
//! # 用法
//!
//! ```ignore
//! use sz_rust_http_facade::grpc::serve_grpc;
//!
//! // 定义 gRPC service
//! #[tonic::async_trait]
//! impl user_service_server::UserService for MyService {
//!     async fn get_user(&self, req: Request<GetUserRequest>) -> Result<Response<User>, Status> {
//!         Ok(Response::new(User { ... }))
//!     }
//! }
//!
//! // 启动 gRPC 服务
//! serve_grpc(MyService::default(), "0.0.0.0:9090".parse().unwrap()).await?;
//! ```

use std::net::SocketAddr;
use thiserror::Error;

/// gRPC 错误
#[derive(Debug, Error)]
pub enum GrpcError {
    /// 传输层错误
    #[error("Transport error: {0}")]
    Transport(String),
    /// 鉴权错误
    #[error("Auth error: {0}")]
    Auth(String),
}

/// gRPC 鉴权拦截器
///
/// 从 metadata 提取 authorization token，验证 JWT。
pub struct AuthInterceptor {
    /// JWT 密钥
    secret: Vec<u8>,
}

impl AuthInterceptor {
    /// 创建鉴权拦截器
    pub fn new(secret: Vec<u8>) -> Self {
        Self { secret }
    }

    /// 从 metadata 提取 token
    pub fn extract_token(metadata: &tonic::metadata::MetadataMap) -> Option<&str> {
        metadata
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.strip_prefix("Bearer ").unwrap_or(s))
    }

    /// 验证 token（简化版，实际应复用 sz-rust-auth JWT 验证）
    pub fn verify(&self, token: &str) -> Result<(), GrpcError> {
        if token.is_empty() {
            return Err(GrpcError::Auth("empty token".into()));
        }
        // 简化验证：实际应调用 sz_rust_auth_facade::AuthService::verify_jwt
        // 这里仅检查 token 非空，实际项目应接入 JWT 验证
        if self.secret.is_empty() {
            return Err(GrpcError::Auth("secret not configured".into()));
        }
        Ok(())
    }
}

/// 启动 gRPC 服务
///
/// 接受已通过 `tonic::transport::Server::builder().add_service(...)` 构建的 Router。
///
/// 需启用 `grpc` feature。
///
/// # 用法
///
/// ```ignore
/// use sz_rust_http_facade::grpc::serve_grpc;
///
/// let router = tonic::transport::Server::builder()
///     .add_service(MyServiceServer::new(my_service));
/// serve_grpc(router, "0.0.0.0:9090".parse().unwrap()).await?;
/// ```
#[cfg(feature = "grpc")]
pub async fn serve_grpc(
    router: tonic::transport::server::Router,
    addr: SocketAddr,
) -> Result<(), GrpcError> {
    router
        .serve(addr)
        .await
        .map_err(|e| GrpcError::Transport(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_interceptor_new() {
        let interceptor = AuthInterceptor::new(b"secret".to_vec());
        assert_eq!(interceptor.secret, b"secret");
    }

    #[test]
    fn test_auth_interceptor_verify() {
        let interceptor = AuthInterceptor::new(b"secret".to_vec());
        assert!(interceptor.verify("valid_token").is_ok());
        assert!(interceptor.verify("").is_err());
    }

    #[test]
    fn test_auth_interceptor_no_secret() {
        let interceptor = AuthInterceptor::new(vec![]);
        assert!(interceptor.verify("token").is_err());
    }

    #[test]
    fn test_grpc_error_display() {
        let err = GrpcError::Transport("connection refused".into());
        assert_eq!(err.to_string(), "Transport error: connection refused");

        let err = GrpcError::Auth("invalid token".into());
        assert_eq!(err.to_string(), "Auth error: invalid token");
    }

    #[test]
    fn test_extract_token() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("authorization", "Bearer abc123".parse().unwrap());
        let token = AuthInterceptor::extract_token(&metadata);
        assert_eq!(token, Some("abc123"));
    }

    #[test]
    fn test_extract_token_missing() {
        let metadata = tonic::metadata::MetadataMap::new();
        let token = AuthInterceptor::extract_token(&metadata);
        assert_eq!(token, None);
    }
}
