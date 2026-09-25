//! WASM 边缘部署示例（v1.4.0 T15.4）
//! 包含配置文件与部署步骤
use sz_rust_wasm::{WasmRuntime, WasmValue};

fn main() {
    println!("▶ WASM 边缘部署示例");
    println!("  部署步骤:");
    println!("    1. cargo build -p sz-rust-wasm --target wasm32-unknown-unknown --release");
    println!("    2. 上传 .wasm 文件至边缘节点");
    println!("    3. 配置运行时参数（memory_limit/execution_timeout/fuel_limit）");
    println!("    4. 启动 sz300-server --wasm-dir /opt/wasm-modules/");
    let runtime = WasmRuntime::new();
    let wasm_bytes = wat::parse_str(
        r#"(module
            (func (export "edge_compute") (param i32) (result i32)
                local.get 0
                local.get 0
                i32.mul)
        )"#,
    )
    .expect("内置 WAT 模块必须可解析");
    let module = runtime.compile(&wasm_bytes).expect("编译失败");
    let result = module
        .execute("edge_compute", &[WasmValue::from(7)])
        .expect("执行失败");
    println!("  edge_compute(7) = {:?}", result);
    println!("✅ 边缘部署示例完成");
}
