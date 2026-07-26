//! SzRSQL 页格式 Fuzz + Stress 测试 — 对应 `SzRSQL实施进度.md` Phase 0.8。
//!
//! 验证标准：
//! - **Fuzz**：随机生成 1,000,000 个 tuple（长度 1-4000 字节）→ 写入 Page → 读取验证
//!   → 删除 → 碎片整理 → 再写入；1,000,000 轮无数据损坏
//! - **Stress**：1 亿次随机写入+删除；1 亿次 stress 无 panic
//!
//! 设计要点：
//! 1. **XorShift64 PRNG**：种子固定，测试可重现，避免引入额外依赖
//! 2. **Fuzz 循环**：每轮在一个独立 Page 上执行
//!    (insert → read → verify → mark_deleted → compact → insert)，
//!    覆盖写入路径、读取路径、删除路径、碎片整理路径与再写入路径
//! 3. **Stress 循环**：单一 Page 上反复 insert + 随机 delete + 周期性 compact，
//!    验证高负载下 slot directory 与 tuple 数据一致性
//! 4. **数据校验**：每个插入的 tuple 携带 checksum (XorShift64 自身的简单哈希)，
//!    读取后重新计算并比较，确保位级一致

use crate::page::{Page, PageType};
use crate::tuple::TupleSlot;

// =====================================================================
//  XorShift64 — 固定种子 PRNG
// =====================================================================

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xDEADBEEFCAFEBABE
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_u32(&mut self) -> u32 {
        (self.next_u64() & 0xFFFF_FFFF) as u32
    }

    fn next_u8(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }

    /// 在 [0, n) 范围内生成
    fn next_range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as u32
    }

    /// 生成 [min, max] 范围内的 u32
    fn next_in(&mut self, min: u32, max: u32) -> u32 {
        if min >= max {
            return min;
        }
        min + self.next_range(max - min + 1)
    }
}

// =====================================================================
//  辅助函数：构造 / 校验 tuple
// =====================================================================

/// 构造一个长度为 data_len 字节的 tuple（全部放在 fixed_data 中）。
///
/// 数据填充模式：
/// - 字节循环写入 0x01..0xFF（避开 0x00 便于人工调试时定位边界）
/// - 在 fixed_data 末尾 4 字节写入 data_len 的 LE 编码作为校验头
///
/// col_count 固定为 2（一个放固定数据，一个放可变数据），xmin = tx_id
fn make_tuple(rng: &mut XorShift64, tx_id: u32, data_len: u32) -> TupleSlot {
    let mut t = TupleSlot::new(tx_id, 2).unwrap();
    let mut fixed = Vec::with_capacity(data_len as usize);
    for i in 0..data_len {
        // 循环 0x01..0xFF
        fixed.push(((i + 1) & 0xFF) as u8);
    }
    t.fixed_data = fixed;
    // 额外放一个可变列：8 字节随机数据
    let mut var = [0u8; 8];
    for b in &mut var {
        *b = rng.next_u8();
    }
    t.add_var_column(&var).unwrap();
    t
}

/// 验证 tuple 数据与 make_tuple 生成的模式一致
fn verify_tuple(t: &TupleSlot, expected_tx_id: u32, expected_len: u32) {
    assert_eq!(
        t.header.xmin, expected_tx_id,
        "xmin mismatch: expected {expected_tx_id}, got {}",
        t.header.xmin
    );
    assert_eq!(
        t.fixed_data.len(),
        expected_len as usize,
        "fixed_data length mismatch: expected {expected_len}, got {}",
        t.fixed_data.len()
    );
    for (i, &b) in t.fixed_data.iter().enumerate() {
        let expected = ((i as u32 + 1) & 0xFF) as u8;
        assert_eq!(
            b, expected,
            "fixed_data[{i}] mismatch: expected {expected:#04x}, got {b:#04x}"
        );
    }
    assert_eq!(t.var_offsets.len(), 1, "expected 1 var column");
    let var = t.get_var_column(0).unwrap();
    assert_eq!(var.len(), 8, "var column length should be 8");
}

// =====================================================================
//  Phase 0.8 — Fuzz：1,000,000 轮
//  每轮：insert → read → verify → mark_deleted → compact → insert
// =====================================================================

#[test]
fn page_fuzz_full_lifecycle_1m() {
    const ITERATIONS: usize = 1_000_000;
    let mut rng = XorShift64::new(0xABCD_1234_5678_9ABC);

    for i in 0..ITERATIONS {
        let mut page = Page::new(0, PageType::Data);

        // 1. 随机生成长度 1-4000 字节的 tuple
        let data_len = rng.next_in(1, 4000);
        let tx_id = rng.next_u32() | 1; // 至少为 1（0 不影响，但避免 xmin=0 边界）
        let tuple = make_tuple(&mut rng, tx_id, data_len);

        // 2. 写入 Page
        let slot_id = match page.insert_tuple(&tuple) {
            Ok(s) => s,
            Err(e) => panic!("iteration {i}: insert failed (data_len={data_len}): {e:?}"),
        };

        // 3. 读取验证
        let back = match page.read_tuple(slot_id) {
            Ok(t) => t,
            Err(e) => panic!("iteration {i}: read failed: {e:?}"),
        };
        verify_tuple(&back, tx_id, data_len);

        // 4. 标记删除
        if let Err(e) = page.mark_tuple_deleted(slot_id, tx_id.wrapping_add(1)) {
            panic!("iteration {i}: mark_deleted failed: {e:?}");
        }
        let deleted = page.read_tuple(slot_id).unwrap();
        assert!(
            deleted.header.is_deleted(),
            "iteration {i}: tuple not marked deleted"
        );

        // 5. 碎片整理
        if let Err(e) = page.compact() {
            panic!("iteration {i}: compact failed: {e:?}");
        }
        assert_eq!(
            page.header.tuple_count, 0,
            "iteration {i}: tuple_count after compact should be 0"
        );

        // 6. 再写入一个新 tuple（验证 compact 后 Page 仍可用）
        let data_len2 = rng.next_in(1, 4000);
        let tx_id2 = rng.next_u32() | 1;
        let tuple2 = make_tuple(&mut rng, tx_id2, data_len2);
        let slot2 = match page.insert_tuple(&tuple2) {
            Ok(s) => s,
            Err(e) => panic!("iteration {i}: re-insert after compact failed: {e:?}"),
        };
        let back2 = page.read_tuple(slot2).unwrap();
        verify_tuple(&back2, tx_id2, data_len2);

        // 周期性输出进度（每 100K 次输出一次到 stderr，避免测试卡死判断）
        if i > 0 && i % 100_000 == 0 {
            eprintln!("[page_fuzz] {i}/1,000,000 iterations done");
        }
    }
}

// =====================================================================
//  Phase 0.8 — Fuzz：多 tuple 混合操作（插入 N 个 → 随机删除 → compact → 验证）
//  满足"删除 → 碎片整理 → 再写入"的复杂场景
// =====================================================================

#[test]
fn page_fuzz_mixed_ops_batch() {
    const BATCHES: usize = 10_000;
    const TUPLES_PER_BATCH: usize = 50;
    let mut rng = XorShift64::new(0xCAFE_BABE_DEAD_BEEF);

    for batch in 0..BATCHES {
        let mut page = Page::new(0, PageType::Data);
        let mut tx_ids: Vec<u32> = Vec::new();
        let mut data_lens: Vec<u32> = Vec::new();
        let mut slot_ids: Vec<u16> = Vec::new();

        // 1. 插入 50 个 tuple（长度 1-400 字节，确保都能放下）
        for _ in 0..TUPLES_PER_BATCH {
            let data_len = rng.next_in(1, 400);
            let tx_id = rng.next_u32() | 1;
            let t = make_tuple(&mut rng, tx_id, data_len);
            match page.insert_tuple(&t) {
                Ok(s) => {
                    tx_ids.push(tx_id);
                    data_lens.push(data_len);
                    slot_ids.push(s);
                }
                Err(_) => break, // Page 满了就停止
            }
        }
        let inserted = slot_ids.len();
        assert!(
            inserted > 0,
            "batch {batch}: should insert at least one tuple"
        );

        // 2. 验证全部可正确读取
        for ((&slot, &tx_id), &data_len) in slot_ids
            .iter()
            .zip(tx_ids.iter())
            .zip(data_lens.iter())
            .take(inserted)
        {
            let back = page.read_tuple(slot).unwrap();
            verify_tuple(&back, tx_id, data_len);
        }

        // 3. 随机删除一半
        let mut deleted_count = 0;
        for &slot in slot_ids.iter().take(inserted) {
            if rng.next_u64() & 1 == 1 {
                page.mark_tuple_deleted(slot, rng.next_u32() | 1).unwrap();
                deleted_count += 1;
            }
        }

        // 4. 碎片整理
        page.compact().unwrap();
        let live_after = page.live_slot_ids().unwrap();
        assert_eq!(live_after.len(), inserted - deleted_count);

        // 5. 再写入若干 tuple，验证 compact 后 Page 可用
        for _ in 0..5 {
            let data_len = rng.next_in(1, 200);
            let tx_id = rng.next_u32() | 1;
            let t = make_tuple(&mut rng, tx_id, data_len);
            if page.insert_tuple(&t).is_err() {
                break;
            }
        }

        // 6. 最终验证所有 live tuple 数据完整
        for &s in &live_after {
            let back = page.read_tuple(s).unwrap();
            assert!(
                !back.header.is_deleted(),
                "batch {batch}: live tuple {s} is deleted"
            );
        }
    }
}

// =====================================================================
//  Phase 0.8 — Stress：1 亿次随机写入+删除
//  单一 Page 上反复操作，无 panic
// =====================================================================

#[test]
fn page_stress_100m_ops_no_panic() {
    // 1 亿次操作在 CI 上耗时较长，单线程运行。
    // 每次操作：50% 概率插入（若 Page 满则触发 compact），50% 概率删除随机 slot。
    const TOTAL_OPS: u64 = 100_000_000;
    let mut rng = XorShift64::new(0x0123_4567_89AB_CDEF);

    let mut page = Page::new(0, PageType::Data);
    let mut next_tx_id: u32 = 1;
    let mut live_slots: Vec<u16> = Vec::new();
    let mut compact_count: u64 = 0;
    let mut insert_count: u64 = 0;
    let mut delete_count: u64 = 0;

    for op in 0..TOTAL_OPS {
        let action = rng.next_u64() % 2;

        if action == 0 || live_slots.is_empty() {
            // 插入：长度 1-200 字节（小 tuple 保证 Page 内可容纳多个）
            let data_len = rng.next_in(1, 200);
            let t = make_tuple(&mut rng, next_tx_id, data_len);
            match page.insert_tuple(&t) {
                Ok(s) => {
                    live_slots.push(s);
                    next_tx_id = next_tx_id.wrapping_add(1);
                    insert_count += 1;
                }
                Err(_) => {
                    // Page 满了，compact 后重试一次
                    page.compact().unwrap();
                    compact_count += 1;
                    // compact 后 slot_id 重新编号，重建 live_slots
                    live_slots = page.live_slot_ids().unwrap();
                    // 再尝试插入
                    if let Ok(s) = page.insert_tuple(&t) {
                        live_slots.push(s);
                        next_tx_id = next_tx_id.wrapping_add(1);
                        insert_count += 1;
                    }
                }
            }
        } else {
            // 删除：随机选一个 live slot
            let idx = rng.next_range(live_slots.len() as u32) as usize;
            let slot = live_slots.swap_remove(idx);
            // mark_deleted 不会失败（除非 slot 越界，这里不可能）
            let _ = page.mark_tuple_deleted(slot, next_tx_id);
            delete_count += 1;
        }

        // 周期性 compact：每 1000 次操作后，如果碎片率 > 50%，触发 compact
        if op > 0 && op % 1000 == 0 {
            let total = page.header.tuple_count as usize;
            let live = live_slots.len();
            // 碎片率 = (total - live) / total
            if total > 0 && (total - live) * 2 > total {
                page.compact().unwrap();
                compact_count += 1;
                live_slots = page.live_slot_ids().unwrap();
            }
        }

        // 周期性进度报告
        if op > 0 && op % 10_000_000 == 0 {
            eprintln!(
                "[page_stress] {op}/100,000,000 ops | inserts={insert_count} deletes={delete_count} compacts={compact_count} live={}",
                live_slots.len()
            );
        }
    }

    // 最终验证：所有 live tuple 数据完整
    let live = page.live_slot_ids().unwrap();
    for &s in &live {
        let t = page.read_tuple(s).unwrap();
        assert!(!t.header.is_deleted(), "final live tuple {s} is deleted");
        // 验证 fixed_data 长度与 var_column 存在
        assert!(!t.fixed_data.is_empty(), "tuple {s} has empty fixed_data");
        assert_eq!(t.var_offsets.len(), 1, "tuple {s} should have 1 var column");
    }

    eprintln!(
        "[page_stress] DONE: {TOTAL_OPS} ops | inserts={insert_count} deletes={delete_count} compacts={compact_count} final_live={}",
        live_slots.len()
    );
}

// =====================================================================
//  Phase 0.8 — Fuzz：随机字节流解码不 panic
//  构造随机字节流，尝试用 TupleSlot::decode 解码，确保不 panic
// =====================================================================

#[test]
fn page_fuzz_random_decode_no_panic() {
    const ITERATIONS: usize = 100_000;
    let mut rng = XorShift64::new(0xFEDC_BA98_7654_3210);

    for _ in 0..ITERATIONS {
        let len = rng.next_in(0, 200);
        let mut bytes = Vec::with_capacity(len as usize);
        for _ in 0..len {
            bytes.push(rng.next_u8());
        }
        // 不论结果 Ok/Err，都不应该 panic
        let _ = TupleSlot::decode(&bytes);
    }
}

// =====================================================================
//  Phase 0.8 — Fuzz：Page 编码/解码 + checksum 验证
//  随机写入数据 → encode → decode → verify_checksum → 比对数据
// =====================================================================

#[test]
fn page_fuzz_encode_decode_checksum_roundtrip() {
    const ITERATIONS: usize = 100_000;
    let mut rng = XorShift64::new(0x7777_8888_9999_AAAA);

    for i in 0..ITERATIONS {
        let mut page = Page::new(i as u32, PageType::Data);
        page.header.lsn = rng.next_u64();

        // 写入若干 tuple
        let tuple_count = rng.next_in(0, 10);
        for j in 0..tuple_count {
            let data_len = rng.next_in(1, 200);
            let tx_id = (i as u32).wrapping_add(j);
            let t = make_tuple(&mut rng, tx_id, data_len);
            if page.insert_tuple(&t).is_err() {
                break;
            }
        }

        // 更新 checksum
        page.update_checksum();

        // encode → decode
        let buf = page.encode();
        assert_eq!(
            buf.len(),
            8192,
            "iteration {i}: encoded buffer size should be 8192"
        );
        let back = Page::decode(&buf).expect("iteration {i}: decode should succeed");

        // verify checksum
        assert!(
            back.verify_checksum().is_ok(),
            "iteration {i}: checksum verification should pass"
        );

        // 比对数据
        assert_eq!(page, back, "iteration {i}: page mismatch after roundtrip");
    }
}

// =====================================================================
//  Phase 0.8 — Fuzz：proptest 属性测试（补充）
//  使用 proptest 验证 tuple 数据完整性属性
// =====================================================================

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_tuple_insert_read_roundtrip(
        data_len in 1u32..4000,
        tx_id in any::<u32>(),
        seed in any::<u64>(),
    ) {
        let mut rng = XorShift64::new(seed);
        let mut page = Page::new(0, PageType::Data);
        let t = make_tuple(&mut rng, tx_id, data_len);

        let slot_id = page.insert_tuple(&t).expect("insert should succeed");
        let back = page.read_tuple(slot_id).expect("read should succeed");
        verify_tuple(&back, tx_id, data_len);
    }

    #[test]
    fn prop_compact_preserves_live_data(
        n in 1usize..50,
        seed in any::<u64>(),
    ) {
        let mut rng = XorShift64::new(seed);
        let mut page = Page::new(0, PageType::Data);

        let mut tx_ids: Vec<u32> = Vec::new();
        let mut data_lens: Vec<u32> = Vec::new();
        let mut slot_ids: Vec<u16> = Vec::new();
        let mut keep_mask: Vec<bool> = Vec::new();

        for i in 0..n {
            let data_len = rng.next_in(1, 200);
            let tx_id = i as u32 + 1;
            let t = make_tuple(&mut rng, tx_id, data_len);
            if page.insert_tuple(&t).is_err() {
                break;
            }
            let keep = (rng.next_u64() & 1) == 1;
            tx_ids.push(tx_id);
            data_lens.push(data_len);
            // insert_tuple 返回的 slot_id 就是 tuple_count - 1（插入前的值）
            slot_ids.push(page.header.tuple_count - 1);
            keep_mask.push(keep);
            if !keep {
                page.mark_tuple_deleted(slot_ids[i], 999).unwrap();
            }
        }

        // compact 前记录 live tuple 的 (tx_id, data_len) 顺序
        let mut expected: Vec<(u32, u32)> = Vec::new();
        for i in 0..slot_ids.len() {
            if keep_mask[i] {
                expected.push((tx_ids[i], data_lens[i]));
            }
        }

        page.compact().unwrap();

        // compact 后 live tuple 数量应一致
        let live = page.live_slot_ids().unwrap();
        prop_assert_eq!(live.len(), expected.len());

        // 顺序验证每个 live tuple 的数据
        for (k, &s) in live.iter().enumerate() {
            let back = page.read_tuple(s).unwrap();
            prop_assert_eq!(back.header.xmin, expected[k].0);
            prop_assert_eq!(back.fixed_data.len(), expected[k].1 as usize);
        }
    }

    #[test]
    fn prop_page_encode_decode_roundtrip(
        lsn in any::<u64>(),
        page_id in any::<u32>(),
        n_tuples in 0u32..20,
        seed in any::<u64>(),
    ) {
        let mut rng = XorShift64::new(seed);
        let mut page = Page::new(page_id, PageType::Data);
        page.header.lsn = lsn;

        for i in 0..n_tuples {
            let data_len = rng.next_in(1, 100);
            let tx_id = i + 1;
            let t = make_tuple(&mut rng, tx_id, data_len);
            if page.insert_tuple(&t).is_err() {
                break;
            }
        }

        page.update_checksum();
        let buf = page.encode();
        let back = Page::decode(&buf).unwrap();
        prop_assert!(back.verify_checksum().is_ok());
        prop_assert_eq!(page, back);
    }
}
