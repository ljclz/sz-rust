# Unmaintained 依赖迁移评估

> 评估日期：2026-07-24
> 评估目标：3 个 unmaintained 依赖的替代方案评估

## 1. paste (RUSTSEC-2024-0436)

| 项目 | 详情 |
|------|------|
| 版本 | 1.0.15 |
| 引入路径 | 大量宏 crate 的传递依赖（proc-macro2 → paste） |
| 安全风险 | 无已知漏洞，仅停止维护 |
| deny.toml | 已 ignore，附理由 |

### 替代方案

- **不迁移**：paste 是 `#[derive]` 宏生态的广泛传递依赖，由 dtolnay 维护（活跃维护者），虽然 crate 本身存档但代码稳定且无 bug。
- **风险**：极低。paste 功能简单（token 拼接），API 不会变化。
- **结论**：**保持现状**。等待上游宏 crate 更新后自然消除。

## 2. rustls-pemfile (RUSTSEC-2025-0134)

| 项目 | 详情 |
|------|------|
| 版本 | 2.2.0 |
| 引入路径 | hyper-rustls → reqwest → sz-rust-pdf |
| 安全风险 | 无已知漏洞，仅停止维护 |
| deny.toml | 已 ignore，附理由 |

### 替代方案

- **rustls-pki-types** 内建 PEM 解析：`rustls-pki-types` 0.2+ 提供了 `pem_section()` 函数，可直接替代 `rustls-pemfile::read_all()`。
- **迁移成本**：低。仅需在 sz-rust-core 的 h2.rs 或 TLS 配置代码中将 `rustls_pemfile::read_all()` 替换为 `rustls_pki_types::pem_section()`。
- **阻碍**：该依赖通过 reqwest 间接引入，sz-rust 无法直接控制 reqwest 的依赖选择。需等待 reqwest 更新。
- **结论**：**短期保持现状，跟踪 reqwest 更新**。当 reqwest 移除对 rustls-pemfile 的依赖后，此警告自动消除。

## 3. ttf-parser (RUSTSEC-2026-0192)

| 项目 | 详情 |
|------|------|
| 版本 | 0.25.1 |
| 引入路径 | ab_glyph → imageproc → sz-rust-core |
| 安全风险 | 无已知漏洞，仅停止维护 |
| deny.toml | 已 ignore，附理由 |

### 替代方案

- **skrifa**（Google Fontations 项目）：现代化的 Rust 字体解析库，活跃维护。
- **迁移成本**：中。需评估 `ab_glyph` 是否已支持 skrifa 后端。如果 `ab_glyph` 仍未迁移，sz-rust 无法单独迁移。
- **备选**：评估 `imageproc` 是否有替代版本使用 `skrifa`。
- **结论**：**短期保持现状，跟踪 ab_glyph/imageproc 更新**。当上游迁移后此警告自动消除。

## 总结

| 依赖 | 风险 | 迁移可行性 | 决策 |
|------|------|-----------|------|
| paste | 极低 | 不可行（传递依赖） | 保持现状 |
| rustls-pemfile | 低 | 需等 reqwest 更新 | 跟踪上游 |
| ttf-parser | 低 | 需等 ab_glyph 更新 | 跟踪上游 |

**总体结论**：3 个 unmaintained 依赖均为传递依赖，无安全漏洞，sz-rust 无法单独迁移。deny.toml 已全部 ignore 并附理由。建议定期（每季度）复查上游迁移状态。
