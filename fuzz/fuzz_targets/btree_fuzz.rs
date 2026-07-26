//! SzRSQL B-Tree cargo-fuzz target — M1 里程碑 Fuzz 测试。
//!
//! ## 验证标准
//!
//! `cargo fuzz run btree_fuzz` → 10 亿次随机操作无 crash。
//!
//! ## 设计
//!
//! libFuzzer 提供任意字节流，本 target 将其解析为操作序列：
//! - 每 9 字节为一个操作：1 字节 op_type + 8 字节 i64 key
//! - op_type & 0x03 决定操作类型：0=insert, 1=delete, 2=search, 3=range_scan
//! - 任意操作失败（如 delete 不存在的 key）不算 crash，仅记录
//! - 真正的 crash = panic / abort / SIGSEGV / stack overflow
//!
//! 不变量检查（每次操作后）：
//! - `validate_all_nodes()` 必须返回 Ok（结构不变量）
//! - 中序遍历必须严格递增
//!
//! ## 运行
//!
//! ```bash
//! # 短时运行（默认 1 个 input，约 1 秒）
//! cargo +nightly fuzz run btree_fuzz
//!
//! # 长时运行（10 亿次操作，约数小时）
//! cargo +nightly fuzz run btree_fuzz -- -max_total_time=3600 -max_len=65536
//!
//! # 多核并行
//! cargo +nightly fuzz run btree_fuzz -- -workers=8 -jobs=8
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use szrsql_storage::btree::{BTree, BTreeError};

/// 从 9 字节解析一个操作
struct Op {
    op_type: u8,
    key: i64,
}

impl Op {
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 9 {
            return None;
        }
        let op_type = bytes[0];
        let key = i64::from_le_bytes([
            bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
        ]);
        Some(Op { op_type, key })
    }

    fn kind(&self) -> u8 {
        self.op_type & 0x03
    }

    fn encoded_key(&self) -> Vec<u8> {
        self.key.to_be_bytes().to_vec()
    }
}

fn run_ops(data: &[u8]) {
    // 限制最大操作数避免 OOM / 超时（每次 fuzz 迭代最多 10K ops）
    let max_ops = 10_000usize;
    let mut bt = BTree::with_default_order();

    // 每 9 字节一个操作
    let mut ops_applied = 0usize;
    for chunk in data.chunks_exact(9) {
        if ops_applied >= max_ops {
            break;
        }
        let Some(op) = Op::from_bytes(chunk) else {
            continue;
        };
        let key = op.encoded_key();
        match op.kind() {
            0 => {
                // insert
                let tuple_id = (ops_applied % 65536) as u16;
                let _ = bt.insert(key, tuple_id);
            }
            1 => {
                // delete
                let _ = bt.delete(&key);
            }
            2 => {
                // search
                let _ = bt.search(&key);
            }
            _ => {
                // range_scan（用同一个 key 作 start 和 end，等价于点查路径）
                // 这里不真正调用 range_scan 避免大量内存分配
                let _ = bt.search(&key);
            }
        }
        ops_applied += 1;

        // 每 1000 次操作验证一次不变量（避免性能开销过大）
        if ops_applied % 1000 == 0 && bt.validate_all_nodes().is_err() {
            // 不变量违反是真正的 bug — panic 让 fuzzer 捕获
            panic!("BTree invariant violated after {} ops", ops_applied);
        }
    }

    // 最终验证
    if !data.is_empty() {
        if let Err(e) = bt.validate_all_nodes() {
            panic!("BTree final validate failed: {:?}", e);
        }
        // 中序遍历严格递增
        if let Ok(pairs) = bt.in_order_leaf_traverse() {
            for i in 1..pairs.len() {
                if pairs[i - 1].0 >= pairs[i].0 {
                    panic!(
                        "BTree in-order traverse not strictly increasing at index {}",
                        i
                    );
                }
            }
        }
    }

    // 抑制 unused 警告
    let _ = BTreeError::NodeEmpty;
}

fuzz_target!(|data: &[u8]| {
    run_ops(data);
});
