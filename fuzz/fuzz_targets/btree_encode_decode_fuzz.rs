//! SzRSQL B-Tree 节点 encode/decode cargo-fuzz target — M1 里程碑 Fuzz 测试。
//!
//! ## 验证标准
//!
//! `cargo fuzz run btree_encode_decode_fuzz` → 任意字节流 decode 不 panic。
//!
//! ## 设计
//!
//! libFuzzer 提供任意字节流，本 target 尝试：
//! 1. `BTreeNode::decode(buf)` — 任意字节流解码，必须不 panic（可返回 Err）
//! 2. 若 decode 成功，验证 `encode(decode(buf))` round-trip 一致性
//! 3. 若 decode 成功，验证 `validate()` 不 panic（可返回 Err）
//!
//! 这是典型的"鲁棒性 fuzz" — 确保损坏的输入不会导致 panic / 内存越界。

#![no_main]

use libfuzzer_sys::fuzz_target;
use szrsql_storage::btree::{BTreeNode, NodeType};

fuzz_target!(|data: &[u8]| {
    // 尝试解码任意字节流
    match BTreeNode::decode(data) {
        Ok(node) => {
            // decode 成功 — 验证 encode round-trip
            let reencoded = node.encode();
            let redecoded = match BTreeNode::decode(&reencoded) {
                Ok(n) => n,
                Err(_) => {
                    panic!(
                        "encode(decode(buf)) round-trip failed: re-decode returned Err, original len={}",
                        data.len()
                    );
                }
            };
            if node != redecoded {
                panic!(
                    "encode(decode(buf)) round-trip mismatch: nodes not equal, original len={}",
                    data.len()
                );
            }

            // 验证 validate() 不 panic（可返回 Err）
            let _ = node.validate();

            // 验证 NodeType::from_u8 不 panic（对 node.node_type.as_u8() 必须成功）
            let _ = NodeType::from_u8(node.node_type.as_u8()).expect("as_u8 must round-trip");
        }
        Err(_) => {
            // decode 失败是合法的 — 损坏的输入返回 Err 而非 panic
        }
    }
});
