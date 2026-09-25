//! WASM 基础示例（v1.4.0 T15.1）
//! 演示 WASM 模块加载与执行
use sz_rust_wasm::{WasmRuntime, WasmValue};

fn main() {
    println!("▶ WASM 基础示例");
    let runtime = WasmRuntime::new();
    let wasm_bytes = wat::parse_str(
        r#"(module
            (func (export "add") (param i32 i32) (result i32)
                local.get 0
                local.get 1
                i32.add)
        )"#,
    )
    .expect("内置 WAT 模块必须可解析");
    let module = runtime.compile(&wasm_bytes).expect("编译失败");
    let result = module
        .execute("add", &[WasmValue::from(3), WasmValue::from(4)])
        .expect("执行失败");
    println!("  add(3, 4) = {:?}", result);
    println!("✅ 基础示例完成");
}
