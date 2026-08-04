# ADR-016: Addon 热加载探索 — libloading 运行时动态加载 + unsafe_code 策略变更

- **状态**: Accepted（探索性实现）
- **日期**: 2026-08-02
- **相关代码**: `packages/sz-rust-core/src/runtime/hot_reload.rs (L1-L517)`、`packages/sz-rust-core/src/lib.rs (L56, L121)`、`packages/sz-rust-core/Cargo.toml (L83, L106)`
- **修复编号**: P2 能力评估遗留项

## 背景

SZ-Rust 的 addon 插件化机制（ADR-007）采用编译期注册 + Cargo feature 方案，插件在编译时确定，运行时无法增减。SaaS 场景下，客户可能需要动态启用/禁用插件而无需重启服务。

不探索热加载的后果：框架无法支持运行时插件管理，客户每次变更插件都需重启服务，影响可用性。

## 决策

### 方案选择：libloading 运行时动态加载（探索性）

使用 `libloading` crate 在运行时动态加载共享库（`.dylib`/`.dll`/`.so`），调用插件导出的 `addon_init` 符号完成注册。

```rust
// packages/sz-rust-core/src/runtime/hot_reload.rs (L152-L154)
/// Addon 初始化函数签名（extern "C" ABI）
type AddonInitFn = unsafe extern "C" fn() -> AddonInitResult;
```

### unsafe_code 策略变更

sz-rust-core 的根 crate 策略从 `#![forbid(unsafe_code)]` 改为 `#![deny(unsafe_code)]`（`packages/sz-rust-core/src/lib.rs L56`），允许模块级 `#![allow(unsafe_code)]` 豁免。

```rust
// packages/sz-rust-core/src/runtime/hot_reload.rs (L80)
#![allow(unsafe_code)]
```

变更原因：`forbid` 不允许任何层级的豁免，`deny` 允许模块级 `allow`，满足 hot_reload 的 FFI 需求同时保持其他模块的严格约束。

### 跨平台共享库检测

```rust
// hot_reload.rs (L440-L448)
fn is_shared_library(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if matches!(ext, "dylib" | "dll") { return true; }
    if ext == "so" { return true; }
    // 处理版本化 .so（如 addon.so.1）
    path.file_name().and_then(|n| n.to_str())
        .map(|name| name.contains(".so.")).unwrap_or(false)
}
```

### 依赖管理

```toml
# packages/sz-rust-core/Cargo.toml (L83, L106)
libloading = { version = "0.8", optional = true }

[features]
hot-reload = ["dep:libloading"]
```

### 安全约束（v1.2 补强）

- `AddonInitFn` 标记为 `unsafe extern "C"`（L154），调用方必须确保符号存在且类型正确。
- `Library::new` 和 `library.get` 包裹在 `unsafe` 块中（L280, L286），附带 `// SAFETY:` 注释。
- 用 `Arc<Library>` 持有加载的库，确保其生命周期覆盖所有注册的路由（L278-L280）。
- **`addon_init` 调用使用 `catch_unwind` 包装**（L291-L303）：防止插件 panic 穿透 FFI 边界导致未定义行为。插件 panic 会被捕获并转化为 `HotReloadError::InitFailed`。

## 后果

### 正面后果
- 支持运行时动态加载/卸载 addon，无需重启服务。
- `Arc<Library>` 确保库在路由使用期间不会被提前卸载。
- 跨平台共享库检测覆盖 macOS（.dylib）、Windows（.dll）、Linux（.so / .so.N）。

### 负面后果
- **探索性标记**：本实现标记为"探索性"，尚未经过生产环境验证。已知限制：
  - **无法真正卸载**：`libloading` 的 `Library` Drop 后，已注册的函数指针可能成为悬空指针（操作系统可能延迟卸载）。生产环境建议仅使用"加载"，不使用"卸载"。
  - **ABI 兼容性**：插件必须与主程序使用相同的 Rust 版本和依赖版本，否则 `addon_init` 符号类型不匹配会导致未定义行为。
  - **跨平台差异**：Windows DLL 加载路径解析与 Unix 不同，`Library::new` 在 Windows 上可能需要完整路径。
- **unsafe_code 风险**：模块级 `allow(unsafe_code)` 打开了安全缺口，若 hot_reload 模块的 `// SAFETY:` 注释不完整，可能引入内存安全问题。
- **测试覆盖**：hot_reload 的 FFI 路径难以单元测试（需要真实共享库文件），当前测试主要覆盖路径检测逻辑。

## 注意事项

- **仅限探索**：本功能标记为"探索性实现"，不建议在生产环境使用"卸载"功能。
- **SAFETY 注释强制**：hot_reload.rs 中所有 `unsafe` 块必须有 `// SAFETY:` 注释，违反者 clippy 会报错。
- **符号命名**：插件必须导出名为 `addon_init` 的符号（`b"addon_init\0"`），大小写敏感。
- **版本对齐**：插件 crate 必须与主程序使用相同的 `sz-rust-core` 版本，否则 ABI 不兼容。
- **组合 feature**：`p2-addons = ["graphql", "grpc", "hot-reload"]` 包含本功能。

### Bug 定位提示

如果生产出现"MissingInitSymbol 错误"：
1. 检查共享库文件是否导出了 `addon_init` 符号（使用 `nm` / `dumpbin` 工具查看）。
2. 检查插件是否使用了 `#[no_mangle] pub extern "C" fn addon_init()` 正确导出。
3. 检查 `libloading` 版本是否与编译插件时使用的版本一致（ABI 差异可能导致符号查找失败）。

如果生产出现"段错误（SIGSEGV）"：
1. **高度怀疑 unsafe 代码问题**：检查是否在 Library Drop 后仍使用了已注册的函数指针。
2. 检查 `Arc<Library>` 是否被正确持有（路由注册时是否 clone 了 Arc）。
3. 检查插件是否访问了主程序中已释放的静态变量。

如果生产出现"插件加载成功但未生效"：
1. 检查 `addon_init` 的返回值是否被正确处理（`AddInitResult::Success` vs `Error`）。
2. 检查插件注册的路由是否与现有路由冲突（路径重复会被覆盖）。
3. 检查插件的依赖是否在主程序中可用（插件依赖的 crate 版本是否与主程序一致）。

如果 clippy 报 `deny(unsafe_code)` 相关警告：
1. 确认 `unsafe` 块是否在 `hot_reload.rs` 模块内（该模块有 `#![allow(unsafe_code)]`）。
2. 若在其他模块中使用了 `unsafe`，需要为该模块单独添加 `#![allow(unsafe_code)]` 并补充 `// SAFETY:` 注释。
