//! WASM 沙箱安全示例（v1.4.0 T15.3）
//! 展示未授权操作被沙箱拒绝
use sz_rust_wasm::{WasmRuntime, WasmValue};

fn main() {
    println!("▶ WASM 沙箱安全示例");
    let runtime = WasmRuntime::new();
    let wasm_bytes = wat::parse_str(
        r#"(module
            (func (export "safe_add") (param i32 i32) (result i32)
                local.get 0
                local.get 1
                i32.add)
        )"#,
    )
    .expect("内置 WAT 模块必须可解析");
    let module = runtime.compile(&wasm_bytes).expect("编译失败");
    let result = module.execute("safe_add", &[WasmValue::from(1), WasmValue::from(2)]);
    match result {
        Ok(v) => println!("  授权操作成功: safe_add(1, 2) = {:?}", v),
        Err(e) => println!("  操作被拒绝: {}", e),
    }
    let unauthorized = module.execute("unauthorized_fn", &[]);
    match unauthorized {
        Ok(_) => println!("  ⚠️ 未授权操作不应成功"),
        Err(e) => println!("  ✅ 未授权操作被拒绝: {}", e),
    }
    println!("✅ 沙箱安全示例完成");
}
