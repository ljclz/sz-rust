//! WASM 边缘计算运行时 — 基于 wasmi 提供 WASM 模块加载与执行
//!
//! 使用 wasmi（Parity Technologies 纯 Rust WASM 解释器）作为运行时。
//! wasmi 解释执行，无需 JIT 编译，启动快、内存占用低，适合服务端边缘计算。

use thiserror::Error;
use wasmi::{Engine, Error as WasmError, Extern, ExternType, Linker, Module, Store, Value};

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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
        Self {
            engine: Engine::default(),
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
        let mut store = Store::new(&self.engine, ());
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
        let _rt = WasmRuntime::new();
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
}
