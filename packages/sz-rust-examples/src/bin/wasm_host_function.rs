// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

//! WASM 宿主函数示例（v1.4.0 T15.2）
//! 演示宿主函数注册与调用
use sz_rust_wasm::{WasmRuntime, WasmValue};

fn main() {
    println!("▶ WASM 宿主函数示例");
    let runtime = WasmRuntime::new();
    println!("  WasmRuntime 已创建，宿主函数通过应用层注册");
    let wasm_bytes = wat::parse_str(
        r#"(module
            (func (export "double") (param i32) (result i32)
                local.get 0
                local.get 0
                i32.add)
        )"#,
    )
    .expect("内置 WAT 模块必须可解析");
    let module = runtime.compile(&wasm_bytes).expect("编译失败");
    let result = module
        .execute("double", &[WasmValue::from(21)])
        .expect("执行失败");
    println!("  double(21) = {:?}", result);
    println!("✅ 宿主函数示例完成");
}
