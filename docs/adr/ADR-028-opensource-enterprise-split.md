# ADR-028: 开源版/企业版双仓库分离

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `scripts/check_license_compliance.py`, `.github/workflows/publish-oss.yml`

## 背景

P0-3 缺口：部分功能（SDD Agent/AI 迁移/可视化画布/插件市场）依赖商业 AI API 和 Tauri 桌面框架，不适合开源（Apache-2.0），需要分离开源版和企业版。

## 决策

1. **双 workspace**：开源仓库 `sz-rust/`（Apache-2.0）+ 企业仓库 `sz-rust-enterprise/`（Commercial）
2. **crates.io/私有 registry 双发布**：开源 crate 发布到 crates.io，企业 crate 发布到私有 registry
3. **许可证合规 CI**：`check_license_compliance.py` 检查开源仓库不依赖企业 crate
4. **路径依赖**：企业版 workspace 依赖通过 path 指向开源仓库中的 crate

## 替代方案

- **Cargo feature flags**：单一仓库用 feature 控制功能，但许可证混合问题无法解决
- **Git submodule**：企业版作为 submodule，但版本管理和 CI 复杂

## Bug 定位提示

- `scripts/check_license_compliance.py` — 许可证合规检查脚本
- `.github/workflows/publish-oss.yml` — 开源版发布 workflow
- `sz-rust-enterprise/Cargo.toml` — 企业版 workspace 根，路径依赖配置

## 影响

- 4 个企业版 crate（sdd-agent/migration/visual/marketplace）移至企业仓库
- 开源 CLI 完全移除 marketplace 依赖
- 企业版仓库 workspace 依赖使用 path 指向开源仓库