// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! WASM 边缘计算运行时 — 基于 wasmi 提供 WASM 模块加载与执行
//!
//! 使用 wasmi（Parity Technologies 纯 Rust WASM 解释器）作为运行时。
//! wasmi 解释执行，无需 JIT 编译，启动快、内存占用低，适合服务端边缘计算。

use thiserror::Error;
use wasmi::{
    Config, Engine, Error as WasmError, Extern, ExternType, Linker, Module, Store,
    StoreLimitsBuilder, Value,
};

/// 单次 WASM 执行的默认燃料上限（wasmi 指令级计量）。
///
/// 燃料耗尽时执行立即返回 `OutOfFuel` 错误，防止无限循环/恶意模块
/// 消耗服务端 CPU（2026-10-27 P0-1 安全修复：原公开白名单 + 无燃料限制可被
/// 单请求永久占死 tokio worker，构成远程 DoS）。
pub const DEFAULT_FUEL_LIMIT: u64 = 10_000_000;

/// 单实例线性内存上限（256 MiB）。
///
/// 防止恶意 WASM 模块声明/增长超大线性内存（wasm 规范上限为 4 GiB）
/// 导致宿主内存耗尽（2026-10-27 P0-1 安全修复配套）。
pub const DEFAULT_MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024;

/// WASM 运行时错误
#[derive(Debug, Error)]
pub enum WasmRuntimeError {
    /// WASM 模块编译/实例化错误
    #[error("WASM 错误: {0}")]
    Wasm(#[from] WasmError),
    /// 函数未找到
    #[error("函数未找到: {0}")]
    FunctionNotFound(String),
    /// 模块实例化失败
    #[error("模块实例化失败: {0}")]
    InstantiationFailed(String),
}

/// WASM 值类型包装（支持 i32/i64）
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum WasmValue {
    /// i32
    I32(i32),
    /// i64
    I64(i64),
}

impl From<i32> for WasmValue {
    fn from(v: i32) -> Self {
        Self::I32(v)
    }
}

impl From<i64> for WasmValue {
    fn from(v: i64) -> Self {
        Self::I64(v)
    }
}

impl WasmValue {
    /// 转为 wasmi Value
    fn to_value(&self) -> Value {
        match self {
            Self::I32(v) => Value::I32(*v),
            Self::I64(v) => Value::I64(*v),
        }
    }

    /// 从 wasmi Value 创建
    fn from_value(val: &Value) -> Option<Self> {
        match val {
            Value::I32(v) => Some(Self::I32(*v)),
            Value::I64(v) => Some(Self::I64(*v)),
            _ => None,
        }
    }

    /// 作为 i32
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Self::I32(v) => Some(*v),
            _ => None,
        }
    }

    /// 作为 i64
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::I64(v) => Some(*v),
            _ => None,
        }
    }
}

/// WASM 运行时 — 管理 Engine 和模块编译
pub struct WasmRuntime {
    engine: Engine,
}

impl WasmRuntime {
    /// 创建新的 WASM 运行时
    pub fn new() -> Self {
        let mut config = Config::default();
        // 启用燃料计量：所有执行按指令扣减燃料，耗尽即报错，杜绝无限循环占死宿主线程
        config.consume_fuel(true);
        Self {
            engine: Engine::new(&config),
        }
    }

    /// 编译 WASM 模块
    pub fn compile(&self, wasm_bytes: &[u8]) -> Result<WasmModule, WasmRuntimeError> {
        let module = Module::new(&self.engine, &mut &wasm_bytes[..])?;
        Ok(WasmModule {
            module,
            engine: self.engine.clone(),
        })
    }
}

impl Default for WasmRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// 已编译的 WASM 模块
pub struct WasmModule {
    module: Module,
    engine: Engine,
}

impl WasmModule {
    /// 执行模块中的指定函数
    pub fn execute(
        &self,
        func_name: &str,
        args: &[WasmValue],
    ) -> Result<Vec<WasmValue>, WasmRuntimeError> {
        // 线性内存上限：实例化/内存增长超过 256 MiB 时被拒绝，
        // 防止 (memory 65536)（4 GiB）之类的模块直接吃光宿主内存。
        // StoreLimits 作为 Store 的用户数据 T 存在，limiter 闭包从 &mut T 取用。
        let limits = StoreLimitsBuilder::new()
            .memory_size(DEFAULT_MEMORY_LIMIT_BYTES)
            .build();
        let mut store = Store::new(&self.engine, limits);

        // 燃料计量（需 Config::consume_fuel(true) 配合）：
        // 无限循环/高消耗模块在燃料耗尽后立即返回 OutOfFuel，而非无限执行。
        store
            .add_fuel(DEFAULT_FUEL_LIMIT)
            .map_err(WasmError::from)?;

        store.limiter(|data| &mut *data);

        let linker = Linker::new(&self.engine);
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(|e| WasmRuntimeError::InstantiationFailed(e.to_string()))?
            .start(&mut store)?;

        let export = instance
            .get_export(&store, func_name)
            .ok_or_else(|| WasmRuntimeError::FunctionNotFound(func_name.to_string()))?;

        let func = match export {
            Extern::Func(f) => f,
            _ => return Err(WasmRuntimeError::FunctionNotFound(func_name.to_string())),
        };

        let func_type = func.ty(&store);
        let result_count = func_type.results().len();

        let inputs: Vec<Value> = args.iter().map(|v| v.to_value()).collect();
        let mut outputs = vec![Value::I32(0); result_count];
        func.call(&mut store, &inputs, &mut outputs)?;

        let results: Vec<WasmValue> = outputs.iter().filter_map(WasmValue::from_value).collect();
        Ok(results)
    }

    /// 列出模块中所有导出的函数名
    pub fn exports(&self) -> Vec<String> {
        self.module
            .exports()
            .filter(|e| matches!(e.ty(), ExternType::Func(_)))
            .map(|e| e.name().to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
                (func (export "add") (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.add)
                (func (export "mul") (param i32 i32) (result i32)
                    local.get 0
                    local.get 1
                    i32.mul)
                (func (export "double") (param i32) (result i32)
                    local.get 0
                    i32.const 2
                    i32.mul)
            )
        "#,
        )
        .unwrap()
    }

    #[test]
    fn test_wasm_runtime_creation() {
        let rt = WasmRuntime::new();
        let module = rt.compile(&add_wasm()).unwrap();
        let result = module.execute("add", &[1.into(), 2.into()]).unwrap();
        assert_eq!(result[0].as_i32(), Some(3));
    }

    #[test]
    fn test_wasm_compile_and_execute_add() {
        let rt = WasmRuntime::new();
        let module = rt.compile(&add_wasm()).unwrap();
        let result = module.execute("add", &[1.into(), 2.into()]).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_i32(), Some(3));
    }

    #[test]
    fn test_wasm_execute_mul() {
        let rt = WasmRuntime::new();
        let module = rt.compile(&add_wasm()).unwrap();
        let result = module.execute("mul", &[3.into(), 4.into()]).unwrap();
        assert_eq!(result[0].as_i32(), Some(12));
    }

    #[test]
    fn test_wasm_execute_double() {
        let rt = WasmRuntime::new();
        let module = rt.compile(&add_wasm()).unwrap();
        let result = module.execute("double", &[21.into()]).unwrap();
        assert_eq!(result[0].as_i32(), Some(42));
    }

    #[test]
    fn test_wasm_function_not_found() {
        let rt = WasmRuntime::new();
        let module = rt.compile(&add_wasm()).unwrap();
        let err = module.execute("nonexistent", &[]).unwrap_err();
        assert!(matches!(err, WasmRuntimeError::FunctionNotFound(_)));
    }

    #[test]
    fn test_wasm_exports() {
        let rt = WasmRuntime::new();
        let module = rt.compile(&add_wasm()).unwrap();
        let exports = module.exports();
        assert!(exports.contains(&"add".to_string()));
        assert!(exports.contains(&"mul".to_string()));
        assert!(exports.contains(&"double".to_string()));
    }

    #[test]
    fn test_wasm_invalid_bytes() {
        let rt = WasmRuntime::new();
        let result = rt.compile(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(result.is_err());
    }

    #[test]
    fn test_wasm_value_conversions() {
        let v = WasmValue::from(42i32);
        assert_eq!(v.as_i32(), Some(42));
        assert_eq!(v.as_i64(), None);

        let v = WasmValue::from(100i64);
        assert_eq!(v.as_i64(), Some(100));
        assert_eq!(v.as_i32(), None);
    }

    // === 边界与极端测试 ===

    #[test]
    fn test_wasm_value_i32_boundaries() {
        for v in [i32::MIN, i32::MAX, 0, -1] {
            let wv = WasmValue::I32(v);
            assert_eq!(wv.as_i32(), Some(v));
            assert_eq!(wv.as_i64(), None);
            assert_eq!(WasmValue::from_value(&wv.to_value()), Some(wv));
        }
    }

    #[test]
    fn test_wasm_value_i64_boundaries() {
        for v in [i64::MIN, i64::MAX, 0, -1] {
            let wv = WasmValue::I64(v);
            assert_eq!(wv.as_i64(), Some(v));
            assert_eq!(wv.as_i32(), None);
            assert_eq!(WasmValue::from_value(&wv.to_value()), Some(wv));
        }
    }

    #[test]
    fn test_wasm_value_roundtrip_boundaries() {
        let values = [
            WasmValue::I32(i32::MIN),
            WasmValue::I32(i32::MAX),
            WasmValue::I32(0),
            WasmValue::I64(i64::MIN),
            WasmValue::I64(i64::MAX),
            WasmValue::I64(0),
        ];
        for v in &values {
            assert_eq!(WasmValue::from_value(&v.to_value()), Some(v.clone()));
        }
    }

    #[test]
    fn test_wasm_compile_empty_bytes() {
        let rt = WasmRuntime::new();
        assert!(rt.compile(&[]).is_err());
    }

    #[test]
    fn test_wasm_compile_single_byte() {
        let rt = WasmRuntime::new();
        assert!(rt.compile(&[0x00]).is_err());
    }

    #[test]
    fn test_wasm_compile_1mb_garbage() {
        let rt = WasmRuntime::new();
        let garbage = vec![0xAA; 1024 * 1024];
        assert!(rt.compile(&garbage).is_err());
    }

    #[test]
    fn test_wasm_compile_damaged_section() {
        let rt = WasmRuntime::new();
        let mut wasm = add_wasm();
        wasm.truncate(wasm.len().saturating_sub(4));
        assert!(rt.compile(&wasm).is_err());
    }

    #[test]
    fn test_wasm_execute_arg_count_mismatch() {
        let rt = WasmRuntime::new();
        let module = rt.compile(&add_wasm()).unwrap();
        assert!(module.execute("add", &[WasmValue::I32(1)]).is_err());
    }

    #[test]
    fn test_wasm_execute_arg_type_mismatch() {
        let rt = WasmRuntime::new();
        let module = rt.compile(&add_wasm()).unwrap();
        assert!(module
            .execute("add", &[WasmValue::I64(1), WasmValue::I64(2)])
            .is_err());
    }

    #[test]
    fn test_wasm_exports_memory_only() {
        let rt = WasmRuntime::new();
        let wasm = wat::parse_str(r#"(module (memory (export "mem") 1))"#).unwrap();
        let module = rt.compile(&wasm).unwrap();
        assert!(module.exports().is_empty());
    }

    #[test]
    fn test_wasm_exports_empty_module() {
        let rt = WasmRuntime::new();
        let wasm = wat::parse_str(r#"(module)"#).unwrap();
        let module = rt.compile(&wasm).unwrap();
        assert!(module.exports().is_empty());
    }

    #[test]
    fn test_wasm_compile_large_memory_not_panic() {
        let rt = WasmRuntime::new();
        let wasm = wat::parse_str(r#"(module (memory 65536))"#).unwrap();
        // 4GiB（65536 页）内存模块：仅编译不实例化，应成功且不 panic
        let compiled = rt.compile(&wasm);
        assert!(compiled.is_ok(), "大内存模块编译应成功");
    }

    #[test]
    fn test_wasm_large_memory_instantiation_rejected() {
        let rt = WasmRuntime::new();
        let wasm = wat::parse_str(r#"(module (memory 65536))"#).unwrap();
        let module = rt.compile(&wasm).unwrap();
        // 实例化（execute 内部）受 256 MiB 内存限制器拦截，应返回错误而非 OOM
        let result = module.execute("missing_func", &[]);
        assert!(
            result.is_err(),
            "4GiB 内存模块实例化应被内存限制器拒绝，实际: {:?}",
            result
        );
    }

    #[test]
    fn test_wasm_infinite_loop_fuel_exhausted() {
        let rt = WasmRuntime::new();
        let wasm = wat::parse_str(r#"(module (func (export "loop") (loop (br 0))))"#).unwrap();
        let module = rt.compile(&wasm).unwrap();
        let start = std::time::Instant::now();
        // 燃料计量应让无限循环快速返回 OutOfFuel 错误，而不是永久占住线程
        let result = module.execute("loop", &[]);
        assert!(
            result.is_err(),
            "无限循环应因燃料耗尽而失败，实际: {:?}",
            result
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "燃料耗尽应在 5 秒内返回，实际耗时 {:?}",
            start.elapsed()
        );
    }
}
