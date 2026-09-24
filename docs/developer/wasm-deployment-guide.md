# WASM 边缘计算部署指南

> **版本**：v1.4.0 T14
> **依赖 crate**：sz-rust-wasm

## 1. 编译指南

### 1.1 安装 wasm-pack

```bash
cargo install wasm-pack
```

### 1.2 编译 WASM 模块

```bash
# 编译为 wasm32-unknown-unknown 目标
cargo build -p sz-rust-wasm --target wasm32-unknown-unknown --release

# 使用 wasm-pack 打包
wasm-pack build packages/sz-rust-wasm --release
```

### 1.3 目标平台配置

在 `packages/sz-rust-wasm/Cargo.toml` 中确保：

```toml
[lib]
crate-type = ["cdylib", "rlib"]
```

## 2. 部署指南

### 2.1 部署到边缘节点

1. 编译 WASM 模块（上文 1.2）
2. 将 `.wasm` 文件上传至边缘节点
3. 配置 sz-rust-wasm 运行时加载模块路径

```bash
# 上传 WASM 模块至边缘节点
scp target/wasm32-unknown-unknown/release/sz_rust_wasm.wasm edge-node:/opt/wasm-modules/

# 在边缘节点启动 sz-rust-wasm 运行时
sz300-server --wasm-dir /opt/wasm-modules/
```

### 2.2 运行时配置

在应用配置中设置 WASM 运行时参数：

```yaml
wasm:
  module_dir: /opt/wasm-modules/
  memory_limit: 64MB          # 内存上限
  execution_timeout: 5000ms   # 执行超时
  fuel_limit: 100000          # 计算燃料上限
```

## 3. 沙箱安全模型

### 3.1 安全隔离

WASM 模块在沙箱中执行，具有以下安全保证：

- **内存隔离**：每个 WASM 实例拥有独立线性内存，无法访问宿主内存
- **堆栈隔离**：调用栈隔离，WASM 模块无法操纵宿主栈
- **系统调用隔离**：WASM 模块无法直接执行系统调用，所有 I/O 通过宿主函数

### 3.2 宿主函数授权

宿主函数需显式注册后方可被 WASM 模块调用：

```rust
use sz_rust_wasm::WasmRuntime;

let mut runtime = WasmRuntime::new();
runtime.register_host_function("log", host_log_fn);
runtime.register_host_function("http_get", host_http_get_fn);
// 未注册的函数调用将被拒绝
```

### 3.3 未授权操作处理

未授权操作（调用未注册的宿主函数）将被沙箱拒绝，返回明确错误：

```
WasmRuntimeError: UnauthorizedHostFunction("file_read")
```

## 4. 性能指南

### 4.1 性能特征

| 指标 | 数值 | 说明 |
|------|------|------|
| 实例化延迟 | ≤ 10ms | WASM 模块加载+实例化 |
| 执行开销 | ~1-5μs/op | 单次操作开销 |
| 内存开销 | 模块大小 × 2 | 实例内存 = 模块线性内存 |

### 4.2 优化建议

1. **预编译**：使用 `--release` 编译消除调试开销
2. **模块复用**：重复执行时复用 WasmRuntime 实例
3. **燃料限制**：设置合理 `fuel_limit` 防止无限循环
4. **内存限制**：设置 `memory_limit` 防止内存耗尽

## 5. 故障排查

| 问题 | 原因 | 解决方案 |
|------|------|---------|
| `target not found: wasm32-unknown-unknown` | 目标未安装 | `rustup target add wasm32-unknown-unknown` |
| 实例化超时 | 模块过大 | 减小模块体积或增大 `execution_timeout` |
| 燃料耗尽 | 计算量超限 | 增大 `fuel_limit` 或优化算法 |
| 内存超限 | 线性内存超限 | 增大 `memory_limit` 或优化内存使用 |
