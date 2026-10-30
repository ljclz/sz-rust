# sz-rust 安全 Gauntlet 验证

对 sz-rust 框架与 sz300 应用做「路由绕过 / 畸形输入 / 并发 / panic 恢复 / 容器与配置安全」的确定性验证。所有测试通过真实 TCP 连接发送**原始 HTTP 请求**（不经 `tower::oneshot`，覆盖 hyper 的真实解析路径），在随机端口上启动独立服务，测试结束自动清理。

## 复现命令

```bash
# 一键运行（输出 PASS/FAIL，退出码 0/1）
bash scripts/audit/security-gauntlet.sh

# 或直接运行测试
cargo test -p sz-rust-sz300 security_gauntlet -- --nocapture
```

Windows Git Bash 环境若 `cargo` 不在 PATH，脚本会自动查找 `/c/Users/Administrator/.cargo/bin`；若本机未安装 `sccache`，脚本会通过 `RUSTC_WRAPPER=""` 禁用 workspace 的 sccache 包装。

## 测试清单（26 项）

| 类别 | 测试 | 断言 |
|------|------|------|
| A 鉴权 | `test_protected_route_requires_auth` | 受保护路由无凭证 GET→401 / POST→403 |
| A 鉴权 | `test_public_routes_are_accessible` | 公开路由无凭证→200 |
| A 鉴权 | `test_valid_token_with_csrf_reaches_protected_handler` | 正控制组：有效 JWT+CSRF→200 |
| A 鉴权 | `test_path_normalization_variants_cannot_bypass_auth` | 21 种路径变体（`//`、`/./`、`..`、`%2e`、尾斜杠、分号、大小写、双重编码等）不得 200 |
| A 鉴权 | `test_public_path_whitelist_exact_match_no_bypass` | 公开路径近似/编码变体不得放行 |
| A 鉴权 | `test_method_override_cannot_bypass_auth` | `_method` / `X-HTTP-Method-Override` 不得绕过 |
| A 鉴权 | `test_content_type_confusion_does_not_bypass` | 5 种 Content-Type 混淆不得绕过 |
| A 鉴权 | `test_host_header_injection_does_not_bypass` | Host 头注入不得绕过 |
| A 鉴权 | `test_forwarded_headers_do_not_bypass` | `X-Forwarded-*` 伪造不得绕过 |
| A 中间件顺序 | `test_middleware_order_csrf_before_auth` | 有 JWT 无 CSRF→403（CSRF 先于 auth） |
| B 容器 | `test_di_singleton_returns_same_instance` | 单例生命周期正确 |
| B 容器 | `test_di_transient_returns_new_instances` | 瞬态生命周期正确 |
| B 容器 | `test_di_cycle_detection_panics` | 循环依赖 panic 输出依赖链 |
| B 容器 | `test_di_factory_panic_recovers` | 工厂 panic 后槽位复位、容器可重试 |
| B 配置 | `test_database_config_debug_redacts_password` | `Debug` 输出不泄露 DB 密码 |
| B 配置 | `test_pg_database_config_debug_redacts_password` | `Debug` 输出不泄露 PG 密码 |
| C panic | `test_handler_panic_with_guard_returns_500_no_leak` | panic_guard 捕获→500 通用 JSON，无堆栈/无内部信息 |
| C panic | `test_handler_panic_without_guard_server_stays_alive` | 无 guard 时连接关闭但服务进程继续可用 |
| C 并发 | `test_concurrent_requests_all_succeed` | 100 并发请求全部 200 |
| C 静态资源 | `test_upload_path_traversal_variants_rejected` | 上传目录路径遍历（`..`/编码/反斜杠/双重编码）全部拒绝 |
| D 畸形输入 | `test_malformed_request_line_returns_4xx_no_leak` | 畸形请求行/非法 URI/非法百分号/非法 UTF-8→4xx 或连接关闭，不泄露 |
| D 畸形输入 | `test_oversized_request_rejected_no_hang` | 64KB 路径、8MB Header 被拒绝，不挂起 |
| D 畸形输入 | `test_malformed_body_encoding_fails_closed` | 畸形 Content-Length / chunked fail-closed |
| D 信息泄露 | `test_not_found_response_no_internal_leak` | 未知路径响应不泄露内部路径（未认证→401 防路径枚举） |
| D 契约 | `test_openapi_contract_valid_and_no_internal_paths` | OpenAPI 3.0.3 契约有效、含 Bearer 方案、无内部绝对路径 |
| D 解析器 | `test_parse_path_never_panics_on_weird_input` | 自定义 `parse_path` 对畸形输入绝不 panic |

## 参考实现对比方法

路由/路径规范化行为的参考实现对比（不修改被测代码，仅对照预期）：

- **nginx 行为**：`nginx -V` 后对 `//x`、`/./x`、`/../x`、`%2e` 等路径做 `location` 匹配对比。nginx 会合并 `//`、解析 `.`/`..`；sz300 的鉴权中间件在路由**之前**基于原始路径做精确白名单匹配，因此对这些变体一律 fail-closed（返回 401/403/404），而不会像 nginx 那样归一化后放行——这是有意为之的更强安全姿态。
- **axum 标准行为**：sz300 的路由层就是 `axum::Router`，路径匹配语义与社区标准一致（原始路径匹配 + 参数解码）。本 Gauntlet 额外验证的是「中间件层先于路由执行」这一点，确保任何路径变体都无法触达 handler。
- **对照实验**：若需验证「去掉 auth 中间件后这些变体能否触达路由」，可临时注释 `build_gauntlet_router()` 中的 `.layer(middleware::from_fn(auth_middleware))` 再运行测试——预期受保护路由变体将返回 404/405（axum 路由不匹配），证明拦截确实来自中间件而非路由巧合。

## 清理步骤

- 测试服务全部绑定 `127.0.0.1:0`（随机端口），测试结束随 tokio 运行时自动销毁，无残留进程。
- 上传遍历测试使用 `std::env::temp_dir()/sz300-upload-gauntlet-<pid>`，测试内 `remove_dir_all` 清理；若测试中途崩溃，手动执行：
  ```bash
  rm -rf "$TMPDIR"/sz300-upload-gauntlet-*   # Git Bash / Linux
  ```
- 本脚本不写数据库、不修改仓库文件（仅 `cargo test` 构建产物写入 `CARGO_TARGET_DIR`）。
- 如需清理构建产物：`cargo clean -p sz-rust-sz300`。