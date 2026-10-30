# 二维码生成验证闭环五维审查报告（2026-10-20）

> 触发依据：项目铁律 15 —— 新增公开 API（`QrVersion` / `verify_png_bytes` / `generate_and_verify` / `generate_unicode` / `generate_matrix_with_version`）须写入五维审查报告。
> 变更内容：`sz-rust-core::qr_code` 扩展「生成 → 解码 → 一致性校验」闭环能力，解码后端采用 `rqrr 0.10.1`；顺带暴露 `qrcode` 0.14.1 已有的 Micro QR 版本矩阵与 Unicode 终端输出。
> 对应 ADR：`docs/adr/ADR-042-qr-verify-rqrr-closed-loop.md`。

## 变更清单（file:line 证据）

| # | 变更 | 位置 |
|---|------|------|
| 1 | `QrCodeError` 新增 `Decode` / `DataMismatch` 错误变体 | `packages/sz-rust-core/src/qr_code.rs:98,102` |
| 2 | 新增 `QrVersion` 枚举（Normal 1~40 / Micro 1~4）及到 `qrcode::Version` 的校验转换 | `packages/sz-rust-core/src/qr_code.rs:179,188` |
| 3 | 新增 `generate_matrix_with_version`（手动指定版本生成矩阵） | `packages/sz-rust-core/src/qr_code.rs:435` |
| 4 | 新增 `generate_unicode`（Unicode 终端输出，零文件依赖） | `packages/sz-rust-core/src/qr_code.rs:470` |
| 5 | 新增 `verify_png_bytes`（内存解码验证，`image::load_from_memory` + `rqrr::PreparedImage`） | `packages/sz-rust-core/src/qr_code.rs:501` |
| 6 | 新增 `generate_and_verify`（生成 PNG → 解码回读 → 一致性校验） | `packages/sz-rust-core/src/qr_code.rs:530` |
| 7 | 私有 `check_consistency` 一致性校验辅助函数 | `packages/sz-rust-core/src/qr_code.rs:538` |
| 8 | 新增 11 个单测（Unicode / 版本矩阵 / 解码回读 / 闭环 / 一致性 / 无码图 / 损坏图） | `packages/sz-rust-core/src/qr_code.rs:854-957` |
| 9 | 根 `Cargo.toml` `[workspace.dependencies]` 登记 `rqrr = "0.10"` | `Cargo.toml:216` |
| 10 | `sz-rust-core/Cargo.toml` 引入 `rqrr = { workspace = true }` | `packages/sz-rust-core/Cargo.toml:57` |
| 11 | `mutants.toml` 更新 `**/qr_code.rs` 排除理由（24 个单测覆盖闭环） | `.cargo/mutants.toml:11-13` |

## 一、正确性 ✅

- 解码回读：`generate_png` 输出的 PNG 字节经 `image::load_from_memory` → `into_luma8()` → `rqrr::PreparedImage::prepare` → `detect_grids()` → `grid.decode()`，返回解码文本；单测 `test_verify_png_bytes_roundtrip` / `test_generate_and_verify_ok` 验证 `https://example.com/...` 数据往返一致。
- 一致性校验：`check_consistency` 对原始数据与解码文本做全等比较，不一致返回 `DataMismatch { original, decoded }`，两个分支均有单测覆盖（`test_check_consistency_mismatch` / `test_check_consistency_match`）。
- 错误路径：空数据返回 `Encoding`；非二维码图片（纯白 PNG）与损坏 PNG 字节均返回 `Decode`（`test_verify_png_bytes_no_qr` / `test_verify_png_bytes_corrupted`）。
- 版本矩阵：`Normal(5)` 产出 37×37、`Micro(2)` 产出 13×13 模块矩阵，非法版本（Normal(99)/Micro(9)）返回 `Encoding`。
- 验证证据：`cargo test -p sz-rust-core --lib qr_code` → **24 passed; 0 failed; 0 ignored**（含新增 11 个用例，2026-10-20 实际运行输出：`test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 476 filtered out`）。
- 覆盖率证据（2026-10-21 `cargo llvm-cov -p sz-rust-core --lib` 实际输出）：`qr_code.rs 773 44 94.31% 64 9 85.94% 445 25 94.38% 0 0 -`，行覆盖率 **94.31%** ≥ 铁律 10 的 85% 门禁；sz-rust-core 整体行覆盖率 96.17%。

## 二、可读性 ✅

- 每个新增公开方法带完整文档注释（参数、返回、错误类型），并给出闭环用法示例（`qr_code.rs:58-66`）。
- `QrVersion` 取值区间与模块数公式在文档注释中说明（`qr_code.rs:169-183`）。
- 错误变体语义明确：`Decode`（解码失败链路）与 `DataMismatch`（数据不一致）区分清晰。

## 三、架构 ⚠️

- 落点遵循既有模式：能力保留在 `sz-rust-core::qr_code`（与生成能力同模块），未新增 crate，符合「通用能力在框架层、业务接线在 sz300/企业版」的既有分层。
- 解码后端选 `rqrr 0.10.1`：MSRV 1.64 ≤ workspace 1.81；`image` 依赖 `default-features = false`，与 workspace 的 P0-2 依赖治理一致（无 exr → pulp/paste 链）。
- ⚠️ 已知边界：仅支持 QR 格式解码。若未来需要 Aztec / Data Matrix / PDF417 / Code 128 等多格式扫描，需重新评估 `rxing`（届时涉及 workspace MSRV 1.81→1.85 升级决策，另行 ADR）。
- ⚠️ 已知边界：`rqrr` 0.11.0 起 MSRV 升至 1.85.1，后续升级需确认 MSRV 兼容性，故版本区间锁定 `"0.10"`。

## 四、安全性 ✅

- 无新增 `unsafe`（`#![forbid(unsafe_code)]` 编译通过）。
- 全程内存解码（`image::load_from_memory` + `rqrr`），不落盘，无 `std::fs`，符合铁律 4。
- 解码函数为同步纯函数，返回 owned `String`/`Vec<u8>`，不持有 `Rc` 等非 `Send` 类型跨 `.await`，符合「所有 async fn 必须 `Send + 'static`」约束。
- 依赖来源：`rqrr` 0.10.1 许可证 `(MIT OR Apache-2.0) AND ISC`，与项目 Apache-2.0 兼容；无已知漏洞告警（cargo-audit 待 CI 门禁复跑）。

## 五、性能 ✅

- 解码仅在调用方显式请求时执行（`verify_png_bytes` / `generate_and_verify`），不影响默认生成路径（`generate_png` / `generate_svg` / `generate_matrix` 无额外开销）。
- `rqrr` 解码为纯 CPU 操作，无网络/DB IO；`generate_and_verify` 每次解码一个二维码，复杂度与图片尺寸线性相关，适合 CI 可扫描性自检与低频溯源验证。
- ⚠️ 注意：高吞吐实时扫码场景（摄像头流）不适用本模块，应评估 `rxing` 的 `MultiUseMultiFormatReader` 或专用扫码库（见架构维度边界）。

## 结论

5 个维度均无阻断项（❌ 0 项），本次变更可合入。遗留边界（仅 QR 格式、rqrr 0.11 MSRV 提升、高吞吐扫码场景）已在架构/性能维度标注为已知取舍，跟踪见 ADR-042。