# addons/ — 业务插件目录（运行时约定）

本目录是 sz300 的**运行时插件根目录**，由 addon 加载器/热加载器扫描，
不是 Cargo workspace 成员，也不会被 `cargo build` 编译。

## 目录结构

```text
addons/
├── README.md              # 本文件
└── <addon-name>/          # 每个插件一个目录（与 PHP think-addons 布局对齐）
    ├── info.ini           # 插件清单（名称/版本/描述，解析见 addons-loader/src/manifest.rs）
    ├── Plugin.php         # 兼容层：PHP 插件入口（Rust 侧仅作为清单来源）
    └── <addon>.dll|.so|.dylib  # 热加载产物，导出 addon_init
```

## 为什么默认是空的

- 插件产物（`.dll` / `.so` / `.dylib`）由部署流程构建并投放，**不入库**，
  避免把二进制产物写进源码仓库；
- 空目录 + 本说明可以让人明确"位置在哪、这里应该放什么"，
  避免热加载器对着不存在的目录空转时被误判为功能失效。

## ABI 边界（重要）

当前插件 ABI 为 **`extern "Rust"`**（见 `packages/sz-rust-core/src/runtime/hot_reload.rs`），
即：

- 插件必须用**与本仓库完全相同的 Rust 工具链与依赖版本**编译；
- 不能跨语言（C/C++/C#/Go）加载，也不承诺跨 Rust 版本稳定；
- 跨语言互操作请走进程边界（gRPC/HTTP/消息队列），不要把 FFI 当作插件机制。

若后续需要稳定的跨语言 ABI，需单独设计 `extern "C"` 导出表 + 版本握手，
并补齐生命周期与内存所有权约定后再接入。

## 加载方式（与代码一致）

```rust
// packages/sz-rust-sz300/src/main.rs
#[cfg(feature = "hot-reload")]
let mut loader = sz_rust_core::runtime::hot_reload::HotAddonLoader::new();
loader.add_scan_dir("addons");
let results = loader.scan().await;   // 扫描 .dll/.so/.dylib 并调用 addon_init
```

- 目录路径当前硬编码为相对工作目录的 `addons`；
- 扫描受 feature `hot-reload` 控制（`cargo build -p sz-rust-sz300 --features hot-reload`）；
- 插件导出符号固定为 `addon_init`，返回 `AddonInitResult`。
