# PR 审查报告（2026-10-05，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ edc8480f`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 0 low）

✅ 未发现问题

## 补充信息

## 变更集
```
 .githooks/pre-commit                               |  10 +-
 .gitignore                                         |   3 +
 Cargo.lock                                         |  33 +-
 packages/sz-rust-data-mask/Cargo.toml              |   8 +
 packages/sz-rust-data-mask/src/layer.rs            | 381 ++++++++++++++++++
 packages/sz-rust-data-mask/src/lib.rs              |   2 +
 packages/sz-rust-data-mask/src/mask.rs             |   3 +
 packages/sz-rust-security-headers/Cargo.toml       |   6 +
 packages/sz-rust-security-headers/src/layer.rs     | 156 ++++++++
 packages/sz-rust-security-headers/src/lib.rs       |   2 +
 packages/sz-rust-sz300/Cargo.toml                  |  43 ++
 packages/sz-rust-sz300/benches/hot_path.rs         | 393 +++++++++++++++++++
 .../docs/performance-baseline-v1.8.0.md            |  78 ++++
 packages/sz-rust-sz300/src/config.rs               | 118 +++++-
 packages/sz-rust-sz300/src/controllers/file.rs     | 132 ++++++-
 packages/sz-rust-sz300/src/graphql/dataloaders.rs  |  82 ++++
 packages/sz-rust-sz300/src/graphql/mod.rs          |  11 +
 packages/sz-rust-sz300/src/graphql/schema.rs       | 270 +++++++++++++
 packages/sz-rust-sz300/src/lib.rs                  |   8 +-
 packages/sz-rust-sz300/src/main.rs                 |  54 ++-
 .../sz-rust-sz300/src/middleware/audit_chain.rs    |  76 ++++
 packages/sz-rust-sz300/src/middleware/mod.rs       |   8 +-
 .../sz-rust-sz300/src/middleware/upload_limit.rs   |  33 ++
 packages/sz-rust-sz300/src/openapi.rs              |  56 ++-
 packages/sz-rust-sz300/src/rbac/guard.rs           |  82 ++++
 packages/sz-rust-sz300/src/rbac/mod.rs             |   9 +
 packages/sz-rust-sz300/src/rbac/roles.rs           | 117 ++++++
 packages/sz-rust-sz300/src/router.rs               | 354 ++++++++++++++++-
 .../sz-rust-sz300/src/services/auth_service.rs     | 120 +++++-
 packages/sz-rust-sz300/src/services/mod.rs         |   8 +-
 packages/sz-rust-sz300/src/services/sse_service.rs | 158 ++++++++
 packages/sz-rust-sz300/src/services/ws_service.rs  |  65 +++
 packages/sz-rust-sz300/src/state.rs                |  32 ++
 packages/sz-rust-sz300/tests/audit_chain_test.rs   | 170 ++++++++
 packages/sz-rust-sz300/tests/common/assertions.rs  | 128 ++++++
 packages/sz-rust-sz300/tests/common/fixtures.rs    | 144 +++++++
 packages/sz-rust-sz300/tests/common/mod.rs         | 209 ++++++++++
 packages/sz-rust-sz300/tests/data_mask_test.rs     | 287 ++++++++++++++
 packages/sz-rust-sz300/tests/e2e_device_test.rs    |  20 +
 packages/sz-rust-sz300/tests/e2e_mqtt_test.rs      |  20 +
 packages/sz-rust-sz300/tests/e2e_ota_test.rs       |  20 +
 packages/sz-rust-sz300/tests/graphql_test.rs       | 415 ++++++++++++++++++++
 packages/sz-rust-sz300/tests/integration_api.rs    | 333 ++++++++++++++++
 packages/sz-rust-sz300/tests/integration_audit.rs  | 181 +++++++++
 packages/sz-rust-sz300/tests/integration_mask.rs   | 163 ++++++++
 packages/sz-rust-sz300/tests/integration_rbac.rs   | 192 +++++++++
 .../sz-rust-sz300/tests/integration_security.rs    | 171 ++++++++
 packages/sz-rust-sz300/tests/key_rotation_test.rs  | 120 ++++++
 packages/sz-rust-sz300/tests/mqtt_dispatch_test.rs |  40 ++
 packages/sz-rust-sz300/tests/rbac_test.rs          | 350 +++++++++++++++++
 .../sz-rust-sz300/tests/security_headers_test.rs   | 172 ++++++++
 .../sz-rust-sz300/tests/service_coverage_test.rs   |  60 +++
 packages/sz-rust-sz300/tests/sse_test.rs           | 339 ++++++++++++++++
 packages/sz-rust-sz300/tests/upload_test.rs        | 436 +++++++++++++++++++++
 packages/sz-rust-sz300/tests/websocket_test.rs     | 289 ++++++++++++++
 scripts/audit/openapi-consistency.js               | 107 +++++
 56 files changed, 7248 insertions(+), 29 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

# PR 评审：DataMaskLayer（axum 数据脱敏中间件）

## 一、最重要的问题

### 1. 🔴 响应 body 被完全缓冲到内存（性能 / DoS 风险）
`apply_mask_to_response` 用 `axum::body::to_bytes(body, 10 * 1024 * 1024)` 一次性读取整个响应体。10MB 上限意味着每个 JSON 响应最多占用 10MB 缓冲，且解析 + 递归遍历 + 重新序列化会产生 **3~4 倍内存峰值**（原始 bytes、`Value` 树、新序列化 bytes）。在大列表接口或高并发下，内存放大非常可观；同时破坏了流式响应（SSE、chunked）。

**建议**：降低上限并按 Content-Length 提前短路；对超大响应直接跳过脱敏并记录告警：

```rust
const MAX_MASK_BODY: usize = 1 * 1024 * 1024; // 1MB 更合理

// 提前根据 Content-Length 短路，避免无谓读取
if let Some(len) = parts
    .headers
    .get(http::header::CONTENT_LENGTH)
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.parse::<usize>().ok())
{
    if len > MAX_MASK_BODY {
        tracing::warn!(len, "response too large for masking, skipping");
        return Response::from_parts(parts, body);
    }
}
```

### 2. 🔴 body 读取失败时静默返回空 body（正确性 / 安全）
```rust
Err(_) => return Response::from_parts(parts, Body::empty()),
```
body 读取错误时返回 `Body::empty()`，但保留了原 headers（包括 `Content-Length`）。客户端会收到 Content-Length 与实际不符的响应，可能挂起或协议错误；更糟的是，**脱敏失败时数据可能以未脱敏形式泄露**（见问题 3 的序列化 fallback 同理）。

**建议**：失败时返回 502/500，而不是空 body：

```rust
let bytes = match axum::body::to_bytes(body, MAX_MASK_BODY).await {
    Ok(b) => b,
    Err(e) => {
        tracing::error!(error = %e, "failed to read response body for masking");
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = http::StatusCode::BAD_GATEWAY;
        return resp;
    }
};
```

### 3. 🟡 递归遍历无深度限制（性能 / 栈溢出）
`mask_json_value` 对嵌套 JSON 递归遍历。恶意或意外构造的深层嵌套 JSON（serde_json 默认递归解析也有此问题，但这里又叠加了一层递归）可能导致栈溢出，直接 crash 进程。

**建议**：加深度限制：

```rust
fn mask_json_value(value: &mut Value, engine: &MaskEngine, scene: MaskScene, depth: usize) {
    if depth > 32 {
        return; // 或 tracing::warn!
    }
    match value {
        Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if let Value::String(s) = val {
                    if let Ok(masked) = engine.mask(scene, key, s) {
                        *s = masked;
                    }
                }
                mask_json_value(val, engine, scene, depth + 1);
            }
        }
        // ...
    }
}
// 调用处：mask_json_value(&mut json, engine, scene, 0);
```

### 4. 🟡 `Content-Length` 头未更新（正确性）
脱敏后 body 长度几乎必然变化，但 `parts.headers` 原样保留，`Content-Length` 会与实际 body 不一致，导致 HTTP 协议错误（hyper 可能会截断或报错）。

**建议**：

```rust
let new_body = serde_json::to_vec(&json).unwrap_or_else(|_| bytes.to_vec());
let mut parts = parts;
parts.headers.remove(http::header::CONTENT_LENGTH);
parts.headers.insert(
    http::header::CONTENT_LENGTH,
    HeaderValue::from(new_body.len()),
);
// 同时考虑移除 ETag / Content-Encoding（若上游压缩过则 JSON 解析已失败，但防御性移除更稳）
parts.headers.remove(http::header::ETAG);
Response::from_parts(parts, Body::from(new_body))
```

### 5. 🟡 `engine.mask` 的错误被静默吞掉（可维护性 / 安全）
```rust
if let Ok(masked) = engine.mask(scene, key, s) { *s = masked; }
```
`Ok/Err` 语义不明：如果 `Err` 表示“无匹配规则”没问题；但如果表示“规则执行失败”（如正则 panic 被捕获），字段会以**明文原样返回**。脱敏中间件应遵循 fail-safe 原则。

**建议**：区分“无规则”与“规则失败”，失败时默认替换为 `***` 并记录日志：

```rust
match engine.mask(scene, key, s) {
    Ok(masked) => *s = masked,
    Err(MaskError::NoRule) => {}
    Err(e) => {
        tracing::warn!(field = %key, error = %e, "mask rule failed, redacting");
        *s = "***".to_string();
    }
}
```

## 二、次要观察

- **Cargo.toml**：`sz-rust-data-mask` 引入 `axum`/`tower`/`http` 作为正式依赖，使纯脱敏库变成 web 框架耦合库。建议拆成 feature gate：`[features] axum-layer = ["axum", "tower", "http"]`，保持核心库轻量。
- **pre-commit**：OpenAPI 一致性检查依赖 `node`，`command -v node` 不存在时静默跳过——CI 中应改为强制失败，避免本地/CI 行为不一致。
- **.gitignore**：`scripts/.server_key` 被忽略说明存在密钥文件落在 scripts 目录的实践，建议密钥走环境变量或 secret manager，而非本地文件（即使被 ignore，也有误提交风险）。
- **diff 截断**：`mask_json_value` 数组分支及后续代码未展示，无法确认数组内对象是否正确递归（截断处 `mask_json_` 疑似已处理，需补看）。

## 三、整体评分

**6 / 10**

功能设计方向正确（tower Layer 模式标准、clone-inner 写法符合惯例），但存在两个必须修复的正确性问题（Content-Length 不更新、错误时返回空 body）和一个安全原则问题（脱敏失败静默放行明文）。修复问题 1/2/4/5 后可达 8 分。


## 结论
✅ 通过（无 ≥ medium 阻塞问题；AI 评审问题不参与阻塞判定）
