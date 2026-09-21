// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户解析策略 — 从请求按 JWT → Header → Path 优先级解析 tenant_id
//!
//! orm-facade 不依赖 axum，通过 `TenantRequest` trait 抽象请求类型，
//! 实际 axum 请求解析在 middleware-facade 中实现。

use async_trait::async_trait;

use super::context::TenantResolveSource;
use super::error::TenantError;

/// 请求抽象 trait — 供解析策略提取 tenant_id
///
/// middleware-facade 为 `axum::extract::Request` 实现此 trait。
pub trait TenantRequest: Send + Sync {
    /// 获取 HTTP Header 值
    fn get_header(&self, name: &str) -> Option<String>;
    /// 获取路径参数值
    fn get_path_param(&self, name: &str) -> Option<String>;
    /// 获取 JWT 中已解码的 tenant_id（由认证中间件注入）
    fn get_jwt_tenant_id(&self) -> Option<i64>;
}

/// 租户解析策略 trait
#[async_trait]
pub trait TenantResolveStrategy: Send + Sync {
    async fn resolve(
        &self,
        req: &dyn TenantRequest,
    ) -> Result<(i64, TenantResolveSource), TenantError>;
}

/// Header 解析策略 — 从 `X-Tenant-Id` Header 提取
pub struct HeaderResolveStrategy;

impl HeaderResolveStrategy {
    pub const HEADER_NAME: &'static str = "X-Tenant-Id";
}

#[async_trait]
impl TenantResolveStrategy for HeaderResolveStrategy {
    async fn resolve(
        &self,
        req: &dyn TenantRequest,
    ) -> Result<(i64, TenantResolveSource), TenantError> {
        let raw = req
            .get_header(Self::HEADER_NAME)
            .ok_or(TenantError::TenantIdRequired)?;
        let tenant_id = raw
            .parse::<i64>()
            .map_err(|_| TenantError::InvalidTenantId(0))?;
        if tenant_id <= 0 {
            return Err(TenantError::InvalidTenantId(tenant_id));
        }
        Ok((tenant_id, TenantResolveSource::Header))
    }
}

/// JWT 解析策略 — 从 JWT Claim `tenant_id` 提取
pub struct JwtResolveStrategy;

#[async_trait]
impl TenantResolveStrategy for JwtResolveStrategy {
    async fn resolve(
        &self,
        req: &dyn TenantRequest,
    ) -> Result<(i64, TenantResolveSource), TenantError> {
        let tenant_id = req
            .get_jwt_tenant_id()
            .ok_or(TenantError::TenantIdRequired)?;
        if tenant_id <= 0 {
            return Err(TenantError::InvalidTenantId(tenant_id));
        }
        Ok((tenant_id, TenantResolveSource::Jwt))
    }
}

/// Path 解析策略 — 从路径参数提取
pub struct PathResolveStrategy {
    pub param_name: String,
}

impl PathResolveStrategy {
    pub fn new(param_name: impl Into<String>) -> Self {
        Self {
            param_name: param_name.into(),
        }
    }
}

impl Default for PathResolveStrategy {
    fn default() -> Self {
        Self::new("tenant")
    }
}

#[async_trait]
impl TenantResolveStrategy for PathResolveStrategy {
    async fn resolve(
        &self,
        req: &dyn TenantRequest,
    ) -> Result<(i64, TenantResolveSource), TenantError> {
        let raw = req
            .get_path_param(&self.param_name)
            .ok_or(TenantError::TenantIdRequired)?;
        let tenant_id = raw
            .parse::<i64>()
            .map_err(|_| TenantError::InvalidTenantId(0))?;
        if tenant_id <= 0 {
            return Err(TenantError::InvalidTenantId(tenant_id));
        }
        Ok((tenant_id, TenantResolveSource::Path))
    }
}

/// 租户解析器 — 按 JWT → Header → Path 优先级依次尝试
pub struct TenantResolver {
    jwt_strategy: JwtResolveStrategy,
    header_strategy: HeaderResolveStrategy,
    path_strategy: Option<PathResolveStrategy>,
}

impl TenantResolver {
    pub fn new(path_param_name: Option<String>) -> Self {
        Self {
            jwt_strategy: JwtResolveStrategy,
            header_strategy: HeaderResolveStrategy,
            path_strategy: path_param_name.map(PathResolveStrategy::new),
        }
    }

    /// 按 JWT → Header → Path 优先级解析，首个命中者生效
    pub async fn resolve(
        &self,
        req: &dyn TenantRequest,
    ) -> Result<(i64, TenantResolveSource), TenantError> {
        if let Ok(result) = self.jwt_strategy.resolve(req).await {
            return Ok(result);
        }
        if let Ok(result) = self.header_strategy.resolve(req).await {
            return Ok(result);
        }
        if let Some(path_strategy) = &self.path_strategy {
            if let Ok(result) = path_strategy.resolve(req).await {
                return Ok(result);
            }
        }
        Err(TenantError::TenantIdRequired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockRequest {
        headers: std::collections::HashMap<String, String>,
        path_params: std::collections::HashMap<String, String>,
        jwt_tenant_id: Option<i64>,
    }

    impl MockRequest {
        fn new() -> Self {
            Self {
                headers: std::collections::HashMap::new(),
                path_params: std::collections::HashMap::new(),
                jwt_tenant_id: None,
            }
        }

        fn with_header(mut self, key: &str, value: &str) -> Self {
            self.headers.insert(key.to_string(), value.to_string());
            self
        }

        fn with_path_param(mut self, key: &str, value: &str) -> Self {
            self.path_params.insert(key.to_string(), value.to_string());
            self
        }

        fn with_jwt_tenant(mut self, tenant_id: i64) -> Self {
            self.jwt_tenant_id = Some(tenant_id);
            self
        }
    }

    impl TenantRequest for MockRequest {
        fn get_header(&self, name: &str) -> Option<String> {
            self.headers.get(name).cloned()
        }

        fn get_path_param(&self, name: &str) -> Option<String> {
            self.path_params.get(name).cloned()
        }

        fn get_jwt_tenant_id(&self) -> Option<i64> {
            self.jwt_tenant_id
        }
    }

    #[tokio::test]
    async fn test_header_resolve_success() {
        let req = MockRequest::new().with_header("X-Tenant-Id", "42");
        let strategy = HeaderResolveStrategy;
        let (id, source) = strategy.resolve(&req).await.unwrap();
        assert_eq!(id, 42);
        assert_eq!(source, TenantResolveSource::Header);
    }

    #[tokio::test]
    async fn test_header_resolve_invalid() {
        let req = MockRequest::new().with_header("X-Tenant-Id", "abc");
        let strategy = HeaderResolveStrategy;
        let err = strategy.resolve(&req).await.unwrap_err();
        assert_eq!(err.error_code(), "INVALID_TENANT_ID");
    }

    #[tokio::test]
    async fn test_header_resolve_zero_id() {
        let req = MockRequest::new().with_header("X-Tenant-Id", "0");
        let strategy = HeaderResolveStrategy;
        let err = strategy.resolve(&req).await.unwrap_err();
        assert_eq!(err.error_code(), "INVALID_TENANT_ID");
    }

    #[tokio::test]
    async fn test_jwt_resolve_success() {
        let req = MockRequest::new().with_jwt_tenant(99);
        let strategy = JwtResolveStrategy;
        let (id, source) = strategy.resolve(&req).await.unwrap();
        assert_eq!(id, 99);
        assert_eq!(source, TenantResolveSource::Jwt);
    }

    #[tokio::test]
    async fn test_path_resolve_success() {
        let req = MockRequest::new().with_path_param("tenant", "7");
        let strategy = PathResolveStrategy::default();
        let (id, source) = strategy.resolve(&req).await.unwrap();
        assert_eq!(id, 7);
        assert_eq!(source, TenantResolveSource::Path);
    }

    #[tokio::test]
    async fn test_missing_returns_required() {
        let req = MockRequest::new();
        let strategy = HeaderResolveStrategy;
        let err = strategy.resolve(&req).await.unwrap_err();
        assert_eq!(err.error_code(), "TENANT_ID_REQUIRED");
    }

    #[tokio::test]
    async fn test_jwt_overrides_header() {
        let req = MockRequest::new()
            .with_jwt_tenant(2)
            .with_header("X-Tenant-Id", "1");
        let resolver = TenantResolver::new(None);
        let (id, source) = resolver.resolve(&req).await.unwrap();
        assert_eq!(id, 2);
        assert_eq!(source, TenantResolveSource::Jwt);
    }

    #[tokio::test]
    async fn test_header_when_no_jwt() {
        let req = MockRequest::new().with_header("X-Tenant-Id", "5");
        let resolver = TenantResolver::new(None);
        let (id, source) = resolver.resolve(&req).await.unwrap();
        assert_eq!(id, 5);
        assert_eq!(source, TenantResolveSource::Header);
    }

    #[tokio::test]
    async fn test_path_when_no_jwt_no_header() {
        let req = MockRequest::new().with_path_param("tenant", "3");
        let resolver = TenantResolver::new(Some("tenant".to_string()));
        let (id, source) = resolver.resolve(&req).await.unwrap();
        assert_eq!(id, 3);
        assert_eq!(source, TenantResolveSource::Path);
    }

    #[tokio::test]
    async fn test_all_missing_returns_required() {
        let req = MockRequest::new();
        let resolver = TenantResolver::new(None);
        let err = resolver.resolve(&req).await.unwrap_err();
        assert_eq!(err.error_code(), "TENANT_ID_REQUIRED");
    }

    #[tokio::test]
    async fn test_custom_path_param_name() {
        let req = MockRequest::new().with_path_param("org_id", "10");
        let resolver = TenantResolver::new(Some("org_id".to_string()));
        let (id, source) = resolver.resolve(&req).await.unwrap();
        assert_eq!(id, 10);
        assert_eq!(source, TenantResolveSource::Path);
    }
}
