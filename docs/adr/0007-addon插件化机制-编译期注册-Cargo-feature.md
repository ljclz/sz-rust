# ADR-007：addon 插件化机制（编译期注册 + Cargo feature）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-001（路由策略）、ADR-003（控制器抽象）
> **相关代码**：`packages/sz-rust-core/src/addons.rs`、`packages/sz-rust-addons-loader/`、`packages/sz-rust-addons-operate/`

## 背景

PHP ThinkPHP 的 addon 插件机制：
- 插件目录：`addons/<plugin_name>/`
- 每个插件有自己的 `controller/`、`model/`、`middleware/`、`config/`
- 插件通过 `AddonsBaseController` 继承链接入框架
- 插件路由通过 `addon_url('plugin/controller/action')` 访问
- 插件可在运行时启用/禁用

PHP 项目使用 addon 机制：
- `addons/operate/`：运营管理插件（商品/订单/客户等）
- 插件内的控制器继承 `AddonsBaseController`，自动获得 `checkLogin` 等方法

sz-rust 需要决定如何实现 addon 插件化机制，在 Rust 的编译期模型下保持灵活性。

## 决策替代方案

在确定编译期注册 + Cargo feature 模式前，曾考虑以下替代方案：

### 方案 A：运行时动态加载（libloading）

```rust
// 运行时加载 .so/.dll
let lib = Library::new("/path/to/addon.so")?;
let init = lib.get::<fn() -> AddonInitResult>(b"addon_init\0")?;
init();
```

**拒绝原因**（详见 ADR-016）：
- **ABI 兼容性**：插件必须与主程序使用完全相同的 Rust 版本和依赖版本，否则 `AddonInitResult` 内存布局不一致会导致 UB
- **生命周期管理**：axum Router 持有插件路由的引用，插件卸载后路由成为悬垂指针
- **安全审计困难**：`unsafe` FFI 调用难以静态分析，插件 panic 可能穿透 FFI 边界
- **版本对齐成本**：每次主程序升级，所有插件需重新编译

> **注**：ADR-016 探索了运行时动态加载方案，但标记为"探索性实现"，不建议生产环境使用。

### 方案 B：HTTP 子进程（Sidecar 模式）

```rust
// 插件作为独立 HTTP 服务运行
// 主程序通过反向代理转发 /addons/<plugin>/ → plugin:8080
```

**拒绝原因**：
- 运维复杂度高（每个插件需要独立进程管理）
- 插件间通信需要额外的服务发现机制
- 无法共享 DI 容器、缓存、数据库连接池
- PHP 端 addon 是进程内插件，Sidecar 模式语义差异大

### 方案 C：WASM 插件（Wasmtime）

```rust
// 插件编译为 WASM 模块
let module = Module::from_file(engine, "addon.wasm")?;
let instance = Instance::new(&module, &imports)?;
```

**拒绝原因**：
- WASM 生态在 Rust 服务端场景尚不成熟
- 插件无法直接使用 Rust 标准库（需要 WASI）
- 性能开销（WASM 沙箱化、内存拷贝）
- 开发体验差（插件需要单独编译为 WASM）

### 最终选择：编译期注册 + Cargo feature

综合以上分析，选择编译期注册方案：
- **类型安全**：插件与主程序共享类型系统，编译期检查
- **零运行时开销**：插件代码直接编译进二进制，无动态加载开销
- **Feature 隔离**：通过 Cargo feature 控制插件编译，不启用则零依赖
- **DI 容器共享**：插件可直接使用主程序的 Service/Repository

## 决策

采用 **编译期注册 + Cargo feature** 模式，放弃运行时动态加载：

### 1. 编译期注册（非运行时动态加载）

```toml
# Cargo.toml
[workspace]
members = [
    "packages/sz-rust-core",
    "packages/sz-rust-addons-operate",  # 运营管理插件
    "packages/sz-rust-addons-loader",   # 插件加载器
]
```

- 插件作为 workspace member 编译
- 通过 Cargo feature 控制是否启用插件
- 编译期确定插件列表，无运行时动态加载

### 2. 插件结构

```
packages/
├── sz-rust-core/              # 框架核心
├── sz-rust-addons-loader/     # 插件加载器
├── sz-rust-addons-operate/    # 运营管理插件
├── sz-rust-pdf/               # PDF 处理插件
├── sz-rust-cli/               # CLI 工具
├── sz-rust-sz300/             # sz300 集成插件
└── sz-rust-examples/          # 示例应用
```

### 3. 插件加载器（sz-rust-addons-loader）

负责：
- 收集所有已编译插件的元数据（名称、版本、路由、中间件）
- 在框架启动时注册插件路由和中间件
- 提供插件依赖解析（如果插件 A 依赖插件 B）

### 4. 插件实现（sz-rust-addons-operate）

```rust
// packages/sz-rust-addons-operate/src/lib.rs
// 实现 6 个模型 + 9 个控制器 + 6 个服务
// 通过 Cargo feature 控制是否编译

// packages/sz-rust/Cargo.toml
[features]
default = ["operate"]
operate = ["dep:sz-rust-addons-operate"]
```

### 5. 放弃运行时动态加载的原因

| 维度 | 编译期注册 | 运行时动态加载 |
|------|-----------|---------------|
| 类型安全 | ✅ 编译期检查 | ❌ 运行时反射 |
| 性能 | ✅ 零开销 | ❌ dlopen 开销 |
| Rust 生态 | ✅ Cargo 原生支持 | ❌ 需要 libloading |
| PHP 对齐 | ❌ PHP 是运行时加载 | ✅ 一致 |
| 灵活性 | ❌ 需要重新编译 | ✅ 热插拔 |

选择编译期注册，因为 Rust 的强类型系统与运行时动态加载天然不兼容，且 Cargo feature 已提供足够的灵活性。

## 后果

### 正面后果

- **类型安全**：所有插件在编译期检查类型，无运行时反射错误
- **性能无损**：编译期注册零运行时开销
- **Cargo 原生支持**：无需额外工具，`cargo build --features operate` 即可控制插件
- **依赖管理清晰**：插件的依赖通过 `Cargo.toml` 显式声明
- **编译优化**：未启用的插件不参与编译，减少二进制体积

### 负面后果

- **无运行时热插拔**：启用/禁用插件需要重新编译，无法运行时切换
- **PHP 迁移差异**：PHP 的 `addon_url('plugin/controller/action')` 在 Rust 端需要编译期确定
- **插件依赖复杂**：如果插件 A 依赖插件 B，需要手动管理 Cargo feature 依赖关系
- **二进制体积**：所有启用的插件都编译进同一个二进制，无法按需加载

## 注意事项

- **插件路由注册**：插件的路由在框架启动时通过 `sz-rust-addons-loader` 注册到 `RouteRegistry`
- **插件中间件**：插件可以提供自己的中间件，通过 `MiddlewareChain` 追加到全局中间件之后
- **插件命名规范**：插件包名格式为 `sz-rust-addons-<name>`（如 `sz-rust-addons-operate`）
- **插件与框架核心的边界**：插件只能依赖 `sz-rust-core`，不能依赖其他插件（除非显式声明）
- **`sz-rust-addons-operate` 的特殊性**：这个插件是业务迁移的核心，包含 375 个测试，但它不是框架核心的一部分

## Bug 定位提示

如果生产 Bug 表现为"插件路由 404"或"插件功能不可用"：

1. **L1 决策层**：查阅本 ADR，确认插件是否通过 Cargo feature 启用，是否在 `sz-rust-addons-loader` 中注册
2. **L2 运行时层**：检查 tracing span `addon.load` 中的 `name` 和 `status` 字段
3. **L3 指标层**：检查 `addon.request.count` 指标按 `addon` 标签的分布
4. **L4 代码层**：
   - 路由 404 Bug → 检查插件的路由是否在 `sz-rust-addons-loader` 中注册到 `RouteRegistry`
   - 功能不可用 Bug → 检查 `Cargo.toml` 的 `features` 是否包含该插件
   - 依赖缺失 Bug → 检查插件的 `Cargo.toml` 是否声明了所有依赖
   - 编译错误 Bug → 检查插件是否只依赖 `sz-rust-core`，避免循环依赖
   - **Feature 组合冲突** → 同时启用多个插件时，若两个插件依赖同一 crate 的不同版本，Cargo 解析失败；检查 `cargo tree -d` 排查重复依赖
   - **插件初始化顺序** → `sz-rust-addons-loader` 按 Cargo feature 字母序初始化插件，若插件 B 依赖插件 A 的运行时状态，需确保 A 在 B 之前初始化（可通过 `addons_init_order` 配置调整）
