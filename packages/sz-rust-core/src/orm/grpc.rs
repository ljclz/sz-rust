//! gRPC Facade — sz-orm-grpc 的统一访问入口
//!
//! ## 设计目标
//!
//! 业务包通过 `sz_rust_core::orm::grpc::*` 访问 gRPC 功能，
//! 而非直接依赖 `sz-orm-grpc`，保持 facade 收口。
//!
//! ## 启用方式
//!
//! 在 `sz-rust-core` 的 Cargo.toml 中启用 `grpc` feature：
//! ```toml
//! sz-rust-core = { version = "0.3", features = ["grpc"] }
//! ```
//!
//! ## 核心类型
//!
//! | 类型 | 说明 |
//! |------|------|
//! | [`GrpcServer`] / [`GrpcServiceDef`] / [`GrpcMethod`] | 服务定义与注册 |
//! | [`UserGrpcService`] | 用户服务 trait |
//! | [`GrpcChannel`] / [`UserGrpcClient`] | 客户端通道与封装 |
//! | [`GrpcStream`] | 同步迭代器风格流式响应 |
//! | [`Interceptor`] / [`LoggingInterceptor`] / [`AuthInterceptor`] | 请求拦截器 |
//! | [`RetryPolicy`] / [`TimeoutPolicy`] | 超时与重试策略 |
//! | [`GrpcError`] | gRPC 错误类型 |
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use sz_rust_core::orm::grpc::{GrpcServer, GrpcServiceDef, GrpcMethod, InMemoryUserService};
//!
//! let service = GrpcServiceDef {
//!     name: "UserService".to_string(),
//!     methods: vec![],
//! };
//! let server = GrpcServer::new("127.0.0.1", 50051)
//!     .register_service(service)
//!     .register_user_service(Arc::new(InMemoryUserService::new()));
//! let handle = server.start().expect("start");
//! ```

#[cfg(feature = "grpc")]
pub use sz_orm_grpc::{
    AuthInterceptor, GrpcChannel, GrpcError, GrpcMethod, GrpcServer, GrpcServerHandle,
    GrpcServiceDef, GrpcStream, InMemoryUserService, Interceptor, InterceptorRequest,
    LoggingInterceptor, RetryPolicy, RetryableErrorKind, TimeoutPolicy, UserGrpcClient,
    UserGrpcService, UserRequest, UserResponse,
};

#[cfg(not(feature = "grpc"))]
compile_error!(
    "gRPC facade requires the `grpc` feature. \
     Enable it in sz-rust-core: sz-rust-core = { features = [\"grpc\"] }"
);

#[cfg(all(feature = "grpc", test))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn unique_port() -> u16 {
        // Use a port derived from test identity to avoid collisions.
        51000 + (std::process::id() % 500) as u16
    }

    #[test]
    fn test_grpc_facade_exports_server_and_service_def() {
        let service = GrpcServiceDef {
            name: "UserService".to_string(),
            methods: vec![GrpcMethod {
                name: "GetUser".to_string(),
                input_type: "UserRequest".to_string(),
                output_type: "UserResponse".to_string(),
                client_streaming: false,
                server_streaming: false,
            }],
        };
        assert_eq!(service.name, "UserService");
        assert_eq!(service.methods.len(), 1);
        assert_eq!(service.methods[0].name, "GetUser");
    }

    #[test]
    fn test_grpc_server_register_and_start() {
        let port = unique_port();
        let service = GrpcServiceDef {
            name: "UserService".to_string(),
            methods: vec![],
        };
        let server = GrpcServer::new("localhost", port).register_service(service);
        let handle = server.start().expect("server should start");
        assert!(handle.address().contains(&port.to_string()));
    }

    #[test]
    fn test_grpc_server_start_no_services_fails() {
        let server = GrpcServer::new("localhost", unique_port());
        assert!(server.start().is_err());
    }

    #[test]
    fn test_grpc_channel_new() {
        let channel = GrpcChannel::new("localhost:50051");
        assert_eq!(channel.address(), "localhost:50051");
    }

    #[test]
    fn test_grpc_channel_with_metadata() {
        let channel =
            GrpcChannel::new("localhost:50051").with_metadata("authorization", "Bearer token123");
        assert_eq!(
            channel.metadata().get("authorization"),
            Some(&"Bearer token123".to_string())
        );
    }

    #[test]
    fn test_user_grpc_client_connect_validates_empty_address() {
        let result = UserGrpcClient::connect("");
        assert!(result.is_err());
    }

    #[test]
    fn test_logging_interceptor_always_ok() {
        let interceptor = LoggingInterceptor;
        let req = InterceptorRequest {
            method: "GetUser".to_string(),
            service_name: "UserService".to_string(),
            metadata: std::collections::HashMap::new(),
        };
        assert!(interceptor.call(&req).is_ok());
    }

    #[test]
    fn test_auth_interceptor_rejects_and_accepts() {
        let interceptor = AuthInterceptor::new("Bearer secret");

        // Missing token
        let req = InterceptorRequest {
            method: "GetUser".to_string(),
            service_name: "UserService".to_string(),
            metadata: std::collections::HashMap::new(),
        };
        assert!(matches!(
            interceptor.call(&req),
            Err(GrpcError::Unauthorized(_))
        ));

        // Valid token
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("authorization".to_string(), "Bearer secret".to_string());
        let req = InterceptorRequest {
            method: "GetUser".to_string(),
            service_name: "UserService".to_string(),
            metadata,
        };
        assert!(interceptor.call(&req).is_ok());
    }

    #[test]
    fn test_retry_policy_default_values() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.initial_delay_ms, 50);
        assert_eq!(policy.max_delay_ms, 1000);
        assert_eq!(policy.multiplier, 2.0);
    }

    #[test]
    fn test_timeout_policy_default() {
        let policy = TimeoutPolicy::default();
        assert_eq!(policy.deadline, std::time::Duration::from_secs(30));
    }

    #[test]
    fn test_grpc_stream_basic() {
        let stream = GrpcStream::new();
        stream.push(1);
        stream.push(2);
        assert_eq!(stream.next(), Some(1));
        assert_eq!(stream.next(), Some(2));
        assert_eq!(stream.next(), None);
        stream.close();
        assert!(stream.is_closed());
    }

    #[test]
    fn test_grpc_error_display() {
        let err = GrpcError::ConnectionFailed("server down".to_string());
        assert!(err.to_string().contains("Connection failed"));
    }

    #[test]
    fn test_in_memory_user_service_crud() {
        let svc = InMemoryUserService::new();
        svc.add_user(UserResponse {
            id: 1,
            username: "alice".to_string(),
            email: "alice@example.com".to_string(),
        });
        assert_eq!(svc.list_users().unwrap().len(), 1);
        let user = svc
            .get_user(UserRequest {
                id: 1,
                username: String::new(),
            })
            .unwrap();
        assert_eq!(user.username, "alice");
        assert!(svc.remove_user(1).is_some());
        assert_eq!(svc.list_users().unwrap().len(), 0);
    }

    #[test]
    fn test_grpc_end_to_end_client_server() {
        // 使用随机端口避免并行测试冲突
        let port = 52000u16 + (std::process::id() % 1000) as u16;
        let addr = format!("localhost:{}", port);

        let svc = Arc::new(InMemoryUserService::new().with_user(UserResponse {
            id: 42,
            username: "bob".to_string(),
            email: "bob@example.com".to_string(),
        }));

        let server = GrpcServer::new("localhost", port)
            .register_service(GrpcServiceDef {
                name: "UserService".to_string(),
                methods: vec![],
            })
            .register_user_service(svc);
        let _handle = server.start().expect("server should start");

        let client = UserGrpcClient::connect(&addr).expect("connect should succeed");
        let user = client.get_user(42).expect("get_user should succeed");
        assert_eq!(user.id, 42);
        assert_eq!(user.username, "bob");

        let missing = client.get_user(999);
        assert!(missing.is_err());
    }
}
