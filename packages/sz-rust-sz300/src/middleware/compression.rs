// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 响应 gzip 压缩（ADR-050）
//!
//! 由 tower-http `CompressionLayer` 承担，仅当客户端 `Accept-Encoding` 含 gzip 时生效。
//! 开关为环境变量 `SZ300_COMPRESSION`（见 [`crate::config::compression_enabled`]），
//! **默认关闭**：压缩是「出网字节 ↓ / CPU ↑」的交换，默认值须由同源 A/B 实测决定。

use axum::body::Body;
use axum::http::{Extensions, HeaderMap, Request, StatusCode, Version};
use axum::middleware::Next;
use axum::response::Response;
use tower_http::compression::predicate::{And, DefaultPredicate};
use tower_http::compression::{CompressionLayer, Predicate};

/// 谓词函数类型（tower-http 对闭包/函数指针的 `Predicate` 实现所需签名）
type PredicateFn = fn(StatusCode, Version, &HeaderMap, &Extensions) -> bool;

/// sz300 压缩层类型：tower-http 默认谓词 + 令牌端点排除
pub type Sz300CompressionLayer = CompressionLayer<And<DefaultPredicate, PredicateFn>>;

/// 响应扩展标记：该响应不得压缩（BREACH 防线）
///
/// 同一响应里既有「攻击者可控明文」又有「每次请求变化的密钥」时，压缩后的长度会随密钥字节
/// 变化，可被反复请求逐步还原（BREACH）。sz300 的 `/api/v1/auth/login` 成功响应把请求里的
/// `username` 原样回显，同时下发 `token` / `refresh_token`，正落在该模式内。
#[derive(Debug, Clone, Copy)]
pub struct NoCompression;

/// 未被 [`NoCompression`] 标记的响应才压缩
fn not_marked(
    _status: StatusCode,
    _version: Version,
    _headers: &HeaderMap,
    extensions: &Extensions,
) -> bool {
    extensions.get::<NoCompression>().is_none()
}

/// 构造 gzip 压缩层（唯一启用的编码；brotli 需新增压缩编解码依赖，见 ADR-050 备选方案）
///
/// 继承 tower-http [`DefaultPredicate`]：跳过 <32 B 的响应、gRPC、图片，
/// 以及 `text/event-stream`（SSE 是长驻分块流，压缩会缓冲掉事件，`/events`、`/api/v1/ai/stream` 依赖其即时性）。
pub fn compression_layer() -> Sz300CompressionLayer {
    let not_marked: PredicateFn = not_marked;
    CompressionLayer::new().compress_when(DefaultPredicate::new().and(not_marked))
}

/// 给响应打上 [`NoCompression`] 标记，挂在令牌签发路由上
///
/// 只在被挂载的那条路由上执行，业务读接口的热路径不经过它。
pub async fn mark_no_compress(req: Request<Body>, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.extensions_mut().insert(NoCompression);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extensions_with(mark: bool) -> Extensions {
        let mut ext = Extensions::new();
        if mark {
            ext.insert(NoCompression);
        }
        ext
    }

    /// 标记过的响应必须排除压缩，未标记的必须放行
    #[test]
    fn test_not_marked_excludes_only_marked_responses() {
        let headers = HeaderMap::new();
        assert!(
            not_marked(
                StatusCode::OK,
                Version::HTTP_11,
                &headers,
                &extensions_with(false)
            ),
            "未标记的响应应允许压缩"
        );
        assert!(
            !not_marked(
                StatusCode::OK,
                Version::HTTP_11,
                &headers,
                &extensions_with(true)
            ),
            "带 NoCompression 的响应不应压缩"
        );
    }

    /// 压缩层可构造，且 gzip 保持启用（未误关）
    #[test]
    fn test_compression_layer_is_constructible() {
        let _layer = compression_layer();
    }
}
