# ADR-042: 二维码生成验证闭环 — rqrr 轻量解码替代 rxing

**状态**：已接受
**日期**：2026-10-20
**决策者**：SZ-Rust Team
**关联 ADR**：ADR-020（异步文件 I/O，std::fs → tokio::fs）、ADR-041（Feature Gate 隔离）

## 背景

微信文章《qrcode-rs + rxing：Rust 二维码生成与验证的完整闭环实战》（公众号「Rust实战学习」，2026-10-13）提出了「业务数据 → 编码 → 渲染 PNG/SVG → 解码验证 → 数据一致性校验」的闭环实践，并推荐 `qrcode-rs`（生成）+ `rxing`（解码/验证）组合。

本仓库现状：
- `sz-rust-core::qr_code` 已具备完整生成能力（对齐 PHP `endroid/qr-code`，PNG/SVG/矩阵输出，13 个单测，行覆盖 94.58%），底层为 `qrcode` 0.14.1。
- **全 workspace 没有任何二维码/条码解码验证能力**（`barcode` 在 sz300 中仅为商品数据字段）。
- 产品规划（`docs/product-technical-plan.md:186`）含 `food-safety 食安监管（溯源、抽检、公示）`，溯源二维码是闭环验证的典型场景。

候选方案：

1. **照搬文章：引入 `qrcode-rs` + `rxing`**
   - `qrcode-rs` 生成侧与现有 `qrcode` crate API 同源，属重复依赖。
   - `rxing` 0.9.x / 0.8.3+ 要求 **Rust 1.85**（workspace 当前 1.81），引入即全 workspace 升 MSRV。
   - `rxing` 默认 `image_formats` feature 会重新引入 `image/exr → pulp/paste`（unmaintained）传递链，违反项目 P0-2 依赖治理决策。
   - `rxing` 代码示例使用 `std::fs` 与 `std::rc::Rc`，与铁律 4（禁同步文件 IO）和 async `Send + 'static` 约束冲突，需大改。
2. **采用 `rqrr` 轻量解码后端（本文档选择）**
   - `rqrr` 0.10.1 MSRV 1.64，`image` 依赖 `>=0.24,<=0.25, default-features=false`，与 workspace 完全兼容，无 exr 链。
   - 纯内存解码（`PreparedImage::prepare(luma8)`），天然符合铁律 4。
   - 仅支持 QR 格式——对「生成后自验证」场景恰好够用。
3. **暂不吸收**：保留缺口，待多格式扫描需求出现时再评估。

## 决策

选择**方案 2**：在 `sz-rust-core::qr_code` 内扩展「生成 → 解码 → 一致性校验」闭环能力，解码后端采用 `rqrr = "0.10"`（锁定 0.10.x 系列），并顺带暴露 `qrcode` 0.14.1 已有的 Micro QR 版本矩阵与 Unicode 终端输出能力。

## 理由

1. **职责分离且依赖最小化**：生成仍用已对齐 PHP 的 `qrcode` crate；解码只用 `rqrr`（纯 QR）。不引入 `qrcode-rs`（与现有生成依赖重复）与 `rxing`（MSRV + 依赖冲突）。
2. **MSRV 零变更**：`rqrr` 0.10.1 rust-version 1.64 ≤ workspace 1.81；`rxing` 需 1.85，升 MSRV 属架构级变更，影响全部 crate 与 CI。
3. **依赖治理一致性**：`rqrr` 的 `image` 依赖 `default-features = false`，与 workspace 的 image 配置（P0-2 消除 exr → pulp/paste）一致；`rxing` 默认会破坏该决策。
4. **铁律合规**：`verify_png_bytes` 使用 `image::load_from_memory` 内存解码，不落盘，符合铁律 4（禁同步文件 IO）；返回 owned `String`/`Vec<u8>`，不跨 `.await` 持有非 Send 类型，符合 async `Send + 'static` 约束。
5. **产品契合**：闭环验证直接服务 `food-safety` 溯源、支付码、门票码等场景，也可作为 `plugin-marketplace-design.md` 中「二维码工具插件」的基础能力。

## 后果

### 正面后果

- 补齐二维码「生成 → 存储 → 扫描 → 验证」闭环缺口，无新增 crate，MSRV 不变。
- 新增 CI/CD 可扫描性自测模式：`generate_and_verify` 生成后立即解码回读，数据不一致即报错。
- 解码全程内存操作，可安全嵌入 async handler。

### 负面后果

- 仅支持 QR 格式解码（`rqrr` 定位）；若未来需要 Aztec / Data Matrix / PDF417 / Code 128 等多格式扫描，需另行评估 `rxing`（届时需 `default-features = false` 裁剪格式、对齐 image/exr 策略，并做 workspace MSRV 1.81→1.85 的独立决策，另行 ADR）。
- `rqrr` 0.11.0 起 MSRV 升至 1.85.1，后续升级需确认 MSRV 兼容性，锁定 `rqrr = "0.10"` 版本区间。
- 新增公开 API（`QrVersion`、`verify_png_bytes`、`generate_and_verify`、`generate_unicode`、`generate_matrix_with_version`）触发文档同步与覆盖率门禁。

## 注意事项

- 解码必须走内存字节（`image::load_from_memory` + `rqrr::PreparedImage::prepare`），禁止 `std::fs` / `detect_multiple_in_file(路径)` 风格的文件 IO。
- `rqrr` 的 `img` feature（默认启用）提供 `GrayImage → PreparedImage` 的 blanket impl，需保持默认 feature。
- `mutants.toml` 中 `**/qr_code.rs` 排除理由需更新为包含闭环测试覆盖说明。
- 新增/修改文档需同步：CHANGELOG、README、README.en、`docs/implementation-progress.md`、`docs/adr/README.md` 索引。

## Bug 定位提示

- 症状：`generate_and_verify` 返回 `DataMismatch` → 检查 `QrCodeError::DataMismatch` 的 `original`/`decoded` 字段，定位编码/解码字符集差异（UTF-8 中文场景重点排查）。
- 症状：`verify_png_bytes` 返回 `Decode("未检测到二维码")` → 检查图片是否包含足够对比度/纠错级别是否过低、`rqrr` 是否仍为 0.10.x。
- 症状：`Decode("图片解码失败")` → 输入非 PNG 字节或图片损坏。
- 关键代码路径：`packages/sz-rust-core/src/qr_code.rs` 的 `generate_and_verify` / `verify_png_bytes` / `check_consistency`。