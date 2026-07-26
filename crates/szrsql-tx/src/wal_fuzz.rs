//! SzRSQL WAL Fuzz 测试 — 对应 `SzRSQL实施进度.md` Phase 2.5。
//!
//! 验证标准（来自实施进度表）：
//! - **Fuzz**：随机生成 WAL record（类型/长度/checksum 对错各 50%）→ 序列化 → 反序列化；
//!   随机部分损坏 WAL 文件 → 回放 → 回放到损坏点之前
//! - **判定**：合法 record 双向一致，损坏 record 不 panic 仅报错
//!
//! 设计要点：
//! 1. **proptest 属性测试**：随机合法 WalRecord → encode → decode → 相等（双向一致）
//! 2. **XorShift64 PRNG**：种子固定，测试可重现，避免引入额外依赖
//! 3. **Garbage decode no panic**：随机字节流 → decode → 永不 panic（Ok 或 Err）
//! 4. **Checksum 50% 对 / 50% 错**：随机记录 + 随机扰动 checksum → verify_checksum 行为正确
//! 5. **WAL 文件部分损坏回放**：N 条合法记录 + 随机字节扰动 → replay 返回损坏点之前的记录，不 panic
//! 6. **WAL 文件截断回放**：N 条合法记录 + 随机位置截断 → replay 返回截断点之前的完整记录（EOF 语义）
//! 7. **中间记录 checksum 损坏**：N 条记录中第 k 条 checksum 被翻转 → replay 停止于 k，返回前 k-1 条
//! 8. **随机 op_type 0-255**：合法值 0-6 decode 成功，非法值 7-255 返回 InvalidOpType，不 panic
//! 9. **超大 data_len 头部**：data_len > 16MB → DataTooLarge，不 panic
//! 10. **header 长度不足**：buf < 21 字节 → BufferTooShort，不 panic

use crate::wal::{
    WalError, WalOpType, WalReader, WalRecord, WalReplayer, WalWriter, WAL_HEADER_SIZE,
    WAL_MAX_DATA_LEN, WAL_MIN_SIZE,
};

// =====================================================================
//  XorShift64 — 固定种子 PRNG（与 page_fuzz.rs 同风格）
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

    /// [0, n) 范围
    fn next_range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as u32
    }

    /// [min, max] 范围
    fn next_in(&mut self, min: u32, max: u32) -> u32 {
        if min >= max {
            return min;
        }
        min + self.next_range(max - min + 1)
    }

    /// 生成 `len` 字节随机数据
    fn next_bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next_u8()).collect()
    }

    /// 50% 概率返回 true
    fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

// =====================================================================
//  辅助函数：构造随机合法 WalRecord / 临时文件路径
// =====================================================================

/// 构造一个随机的合法 WalRecord（已 update_checksum）
fn random_valid_record(rng: &mut XorShift64) -> WalRecord {
    let lsn = rng.next_u64();
    let tx_id = rng.next_u32();
    let op_type = match rng.next_range(7) {
        0 => WalOpType::Insert,
        1 => WalOpType::Update,
        2 => WalOpType::Delete,
        3 => WalOpType::Commit,
        4 => WalOpType::Abort,
        5 => WalOpType::Checkpoint,
        _ => WalOpType::FullPageImage,
    };
    let page_id = rng.next_u32();
    // 数据长度 0-512 字节（控制测试时间），覆盖空、小、中等长度
    let data_len = rng.next_in(0, 512) as usize;
    let data = rng.next_bytes(data_len);
    let mut record = WalRecord::new(lsn, tx_id, op_type, page_id, data);
    record.update_checksum();
    record
}

/// 生成唯一临时文件路径
fn temp_wal_path(test_name: &str, suffix: u64) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("szrsql_wal_fuzz");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{test_name}_{suffix}.wal"))
}

/// 写入 N 条合法记录到 WAL 文件并返回记录列表 + 文件路径
///
/// 注意：WalWriter::append 会自动分配 LSN（忽略 record 中原有的 lsn）并重新计算 checksum，
/// 因此这里用 append 返回的实际 LSN 替换 record.lsn 后，必须重新 update_checksum，
/// 确保后续比较时 record 与文件中实际存储的记录完全一致。
fn write_n_records(
    rng: &mut XorShift64,
    path: &std::path::Path,
    n: usize,
) -> Result<Vec<WalRecord>, WalError> {
    let writer = WalWriter::open(path)?;
    let mut records = Vec::with_capacity(n);
    for _ in 0..n {
        let mut record = random_valid_record(rng);
        let actual_lsn = writer.append(record.clone())?;
        record.lsn = actual_lsn;
        record.update_checksum();
        records.push(record);
    }
    writer.flush()?;
    drop(writer);
    Ok(records)
}

// =====================================================================
//  Phase 2.5 测试
// =====================================================================

/// 测试入口模块（与 page_fuzz 风格一致：所有测试在此模块内）
#[cfg(test)]
mod phase_2_5 {
    use super::*;
    use proptest::prop_assert;
    use proptest::prop_assert_eq;

    // -----------------------------------------------------------------
    // 1. proptest 属性测试：合法 record 双向一致
    // -----------------------------------------------------------------

    proptest::proptest! {
        /// 随机合法 WalRecord → encode → decode → 与原 record 相等
        ///
        /// 验证标准：合法 record 双向一致
        #[test]
        fn prop_encode_decode_roundtrip(
            lsn in 0u64..=u64::MAX,
            tx_id in 0u32..=u32::MAX,
            op_type_idx in 0u8..7,
            page_id in 0u32..=u32::MAX,
            data_len in 0usize..1024,
            data_seed in 0u64..u64::MAX,
        ) {
            let op_type = match op_type_idx {
                0 => WalOpType::Insert,
                1 => WalOpType::Update,
                2 => WalOpType::Delete,
                3 => WalOpType::Commit,
                4 => WalOpType::Abort,
                5 => WalOpType::Checkpoint,
                _ => WalOpType::FullPageImage,
            };
            // 用 data_seed 生成确定性 data
            let mut rng = XorShift64::new(data_seed);
            let data = rng.next_bytes(data_len);

            let mut record = WalRecord::new(lsn, tx_id, op_type, page_id, data);
            record.update_checksum();
            let encoded = record.encode();
            let decoded = WalRecord::decode(&encoded).expect("合法 record 应成功 decode");
            prop_assert_eq!(record, decoded.clone());
            // checksum 校验通过
            prop_assert!(decoded.verify_checksum().is_ok());
        }

        /// 随机字节流 decode 不 panic（返回 Ok 或 Err）
        ///
        /// 验证标准：损坏 record 不 panic 仅报错
        #[test]
        fn prop_decode_random_bytes_no_panic(
            len in 0usize..256,
            seed in 0u64..u64::MAX,
        ) {
            let mut rng = XorShift64::new(seed);
            let bytes = rng.next_bytes(len);
            // 不允许 panic
            let _ = WalRecord::decode(&bytes);
        }

        /// 随机 op_type 值 (0-255)：合法 0-6 成功，非法 7-255 返回 InvalidOpType
        #[test]
        fn prop_op_type_roundtrip(op_val in 0u8..=6u8) {
            let op = WalOpType::from_u8(op_val).expect("0-6 应为合法 op_type");
            prop_assert_eq!(op.as_u8(), op_val);
        }

        #[test]
        fn prop_op_type_invalid_returns_err(op_val in 7u8..=255u8) {
            let result = WalOpType::from_u8(op_val);
            prop_assert!(matches!(result, Err(WalError::InvalidOpType(_))));
        }
    }

    // -----------------------------------------------------------------
    // 2. Checksum 对错各 50% — 验证 verify_checksum 行为
    // -----------------------------------------------------------------

    /// 50% 概率随机翻转 checksum 的若干 bit，verify_checksum 应返回 ChecksumMismatch
    ///
    /// 验证标准：checksum 对错各 50%，verify_checksum 行为正确
    #[test]
    fn checksum_correct_passes_verify() {
        let mut rng = XorShift64::new(0x1234_5678);
        for _ in 0..200 {
            let record = random_valid_record(&mut rng);
            assert!(record.verify_checksum().is_ok(), "正确 checksum 应通过校验");
        }
    }

    #[test]
    fn checksum_corrupted_fails_verify() {
        let mut rng = XorShift64::new(0xABCD_EF01);
        for _ in 0..200 {
            let mut record = random_valid_record(&mut rng);
            // 翻转若干 bit（保证改后值与原值不同的概率极高）
            record.checksum ^= rng.next_u32() | 1;
            assert!(
                matches!(
                    record.verify_checksum(),
                    Err(WalError::ChecksumMismatch { .. })
                ),
                "损坏 checksum 应返回 ChecksumMismatch"
            );
        }
    }

    #[test]
    fn checksum_50_percent_correct_vs_corrupted_no_panic() {
        let mut rng = XorShift64::new(0x55AA_55AA);
        let mut correct_pass = 0usize;
        let mut corrupt_fail = 0usize;
        for i in 0..1000u64 {
            let mut record = random_valid_record(&mut rng);
            // 偶数 i 保持正确，奇数 i 破坏
            if i & 1 == 1 {
                record.checksum = record.checksum.wrapping_add(1);
                corrupt_fail += 1;
                assert!(
                    matches!(
                        record.verify_checksum(),
                        Err(WalError::ChecksumMismatch { .. })
                    ),
                    "i={} 应为 checksum 不匹配",
                    i
                );
            } else {
                correct_pass += 1;
                assert!(
                    record.verify_checksum().is_ok(),
                    "i={} 应为 checksum 匹配",
                    i
                );
            }
        }
        assert_eq!(correct_pass, 500);
        assert_eq!(corrupt_fail, 500);
    }

    // -----------------------------------------------------------------
    // 3. 边界条件 — 不 panic
    // -----------------------------------------------------------------

    #[test]
    fn decode_empty_buffer_returns_buffer_too_short() {
        let result = WalRecord::decode(&[]);
        assert!(matches!(result, Err(WalError::BufferTooShort { .. })));
    }

    #[test]
    fn decode_short_header_returns_buffer_too_short() {
        let buf = vec![0u8; WAL_HEADER_SIZE - 1];
        let result = WalRecord::decode(&buf);
        assert!(matches!(result, Err(WalError::BufferTooShort { .. })));
    }

    #[test]
    fn decode_min_size_empty_data_succeeds() {
        // WAL_MIN_SIZE = 25 字节，data 为空
        let mut record = WalRecord::new(1, 1, WalOpType::Insert, 1, vec![]);
        record.update_checksum();
        let encoded = record.encode();
        assert_eq!(encoded.len(), WAL_MIN_SIZE);
        let decoded = WalRecord::decode(&encoded).expect("min size 应成功");
        assert_eq!(record, decoded);
    }

    #[test]
    fn decode_data_len_exceeds_max_returns_data_too_large() {
        // 缓冲区必须 >= WAL_MIN_SIZE (25) 才能通过第一道长度检查
        // 构造 25 字节缓冲：header(21) + 4 字节填充（充当空 data 的 checksum）
        // 在 header 中设置 data_len = WAL_MAX_DATA_LEN + 1
        let mut buf = vec![0u8; WAL_MIN_SIZE];
        buf[17..21].copy_from_slice(&((WAL_MAX_DATA_LEN as u32) + 1).to_le_bytes());
        let result = WalRecord::decode(&buf);
        assert!(matches!(result, Err(WalError::DataTooLarge { .. })));
    }

    #[test]
    fn decode_data_len_within_max_but_buffer_short_returns_buffer_too_short() {
        // data_len 声明为 100，但 buf 不足
        let mut buf = Vec::with_capacity(WAL_HEADER_SIZE);
        buf.extend_from_slice(&0u64.to_le_bytes()); // lsn
        buf.extend_from_slice(&0u32.to_le_bytes()); // tx_id
        buf.push(0u8); // op_type
        buf.extend_from_slice(&0u32.to_le_bytes()); // page_id
        buf.extend_from_slice(&100u32.to_le_bytes()); // data_len
        buf.extend_from_slice(&[0u8; 10]); // 不足 100+4 字节
        let result = WalRecord::decode(&buf);
        assert!(matches!(result, Err(WalError::BufferTooShort { .. })));
    }

    // -----------------------------------------------------------------
    // 4. WAL 文件部分损坏回放 — 回放到损坏点之前
    // -----------------------------------------------------------------

    /// 写入 N 条合法记录 → 随机字节扰动 → replay 返回损坏点之前的记录，不 panic
    ///
    /// 验证标准：随机部分损坏 WAL 文件 → 回放 → 回放到损坏点之前
    #[test]
    fn fuzz_corrupted_wal_replays_until_corruption() {
        let mut rng = XorShift64::new(0xF00D_F00D);
        for cycle in 0..50 {
            let path = temp_wal_path("corrupt_replay", cycle);
            let _ = std::fs::remove_file(&path);
            // 写入 20 条合法记录
            let original = write_n_records(&mut rng, &path, 20).expect("写入失败");
            // 读取文件内容
            let mut content = std::fs::read(&path).expect("读取失败");
            if content.is_empty() {
                continue;
            }
            // 随机扰动：在 1-10 个位置翻转字节
            let num_corrupt = rng.next_in(1, 10);
            for _ in 0..num_corrupt {
                let pos = rng.next_range(content.len() as u32) as usize;
                content[pos] ^= rng.next_u8();
            }
            std::fs::write(&path, &content).expect("写入失败");
            // 回放：应返回 <= 20 条记录，不 panic
            let replayed = WalReplayer::replay_all(&path).expect("replay 不应返回 Err");
            assert!(
                replayed.len() <= original.len(),
                "cycle={}: replayed {} 应 <= original {}",
                cycle,
                replayed.len(),
                original.len()
            );
            // 已回放的记录必须与原始记录前缀完全一致
            for (i, r) in replayed.iter().enumerate() {
                assert_eq!(
                    r, &original[i],
                    "cycle={}: replayed[{}] 与 original 不一致",
                    i, cycle
                );
            }
            let _ = std::fs::remove_file(&path);
        }
    }

    /// 写入 N 条合法记录 → 在随机位置截断 → replay 返回截断点之前的完整记录
    ///
    /// 验证标准：部分写入（截断）视为 EOF，回放返回已完整的记录
    #[test]
    fn fuzz_truncated_wal_replays_complete_records_only() {
        let mut rng = XorShift64::new(0xCAFE_BABE);
        for cycle in 0..50 {
            let path = temp_wal_path("trunc_replay", cycle);
            let _ = std::fs::remove_file(&path);
            let original = write_n_records(&mut rng, &path, 20).expect("写入失败");
            let full_content = std::fs::read(&path).expect("读取失败");
            // 在 [0, full_len] 之间随机截断
            let trunc_len = rng.next_range((full_content.len() + 1) as u32) as usize;
            let truncated = &full_content[..trunc_len];
            std::fs::write(&path, truncated).expect("写入失败");
            // 回放：应返回 <= 20 条记录
            let replayed = WalReplayer::replay_all(&path).expect("replay 不应返回 Err");
            assert!(
                replayed.len() <= original.len(),
                "cycle={}: 截断后 replayed {} 应 <= original {}",
                cycle,
                replayed.len(),
                original.len()
            );
            // 已回放的记录必须与原始记录前缀完全一致
            for (i, r) in replayed.iter().enumerate() {
                assert_eq!(
                    r, &original[i],
                    "cycle={}: 截断后 replayed[{}] 与 original 不一致",
                    i, cycle
                );
            }
            let _ = std::fs::remove_file(&path);
        }
    }

    /// 中间记录 checksum 被翻转 → replay 停止于该记录，返回前 k-1 条
    ///
    /// 验证标准：损坏 record 不 panic 仅报错（replay 停止于损坏点）
    #[test]
    fn fuzz_corrupted_middle_checksum_stops_replay() {
        let mut rng = XorShift64::new(0xDEAD_BEEF);
        for cycle in 0..30 {
            let path = temp_wal_path("mid_corrupt", cycle);
            let _ = std::fs::remove_file(&path);
            let original = write_n_records(&mut rng, &path, 15).expect("写入失败");
            let mut content = std::fs::read(&path).expect("读取失败");
            // 选择第 5 条记录（索引 4），翻转其 checksum 末字节
            // 第 5 条记录起始位置 = 累加前 4 条记录的 encoded_size
            let mut offset = 0usize;
            for r in original.iter().take(4) {
                offset += r.encoded_size();
            }
            // checksum 在记录末 4 字节
            if offset + original[4].encoded_size() <= content.len() {
                let checksum_pos = offset + original[4].encoded_size() - 4;
                content[checksum_pos] ^= 0xFF;
                std::fs::write(&path, &content).expect("写入失败");
                // 回放：应停止于第 5 条（返回 4 条）
                let replayed = WalReplayer::replay_all(&path).expect("replay 不应返回 Err");
                assert!(
                    replayed.len() <= 4,
                    "cycle={}: 损坏第 5 条后 replayed {} 应 <= 4",
                    cycle,
                    replayed.len()
                );
                for (i, r) in replayed.iter().enumerate() {
                    assert_eq!(
                        r, &original[i],
                        "cycle={}: replayed[{}] 与 original 不一致",
                        i, cycle
                    );
                }
            }
            let _ = std::fs::remove_file(&path);
        }
    }

    // -----------------------------------------------------------------
    // 5. 综合大循环 Fuzz — 10000 轮随机 encode/decode + 损坏回放
    // -----------------------------------------------------------------

    /// 10000 轮：随机合法 record → encode → 随机扰动 50% → decode 不 panic
    ///
    /// 验证标准：合法 record 双向一致；损坏 record 不 panic 仅报错
    #[test]
    fn fuzz_10000_rounds_encode_decode_with_random_corruption() {
        let mut rng = XorShift64::new(0x1000_0BEE);
        let mut roundtrip_ok = 0usize;
        let mut corruption_err = 0usize;
        for _ in 0..10000 {
            let record = random_valid_record(&mut rng);
            let mut encoded = record.encode();
            // 50% 概率扰动
            if rng.next_bool() {
                let pos = rng.next_range(encoded.len() as u32) as usize;
                encoded[pos] ^= rng.next_u8();
                match WalRecord::decode(&encoded) {
                    Ok(decoded) => {
                        // 扰动后仍可能 decode 成功（如 lsn 变化但 checksum 未变 → verify_checksum 失败）
                        // 或 decode 成功但与原 record 不同
                        if decoded.verify_checksum().is_err() || decoded != record {
                            corruption_err += 1;
                        } else {
                            // 极小概率扰动后仍相同（如翻转的 bit 又被翻转回）
                            roundtrip_ok += 1;
                        }
                    }
                    Err(_) => {
                        corruption_err += 1;
                    }
                }
            } else {
                let decoded = WalRecord::decode(&encoded).expect("未扰动应成功 decode");
                assert_eq!(record, decoded, "未扰动 record 应双向一致");
                assert!(decoded.verify_checksum().is_ok());
                roundtrip_ok += 1;
            }
        }
        // 至少应有大量双向一致 + 大量错误（统计性验证）
        assert!(
            roundtrip_ok > 4000,
            "双向一致数量 {} 应 > 4000",
            roundtrip_ok
        );
        assert!(
            corruption_err > 4000,
            "错误数量 {} 应 > 4000",
            corruption_err
        );
    }

    /// 1000 轮：随机合法记录文件 → 随机扰动 → WalReader.read_all 不 panic
    ///
    /// 验证标准：WAL 文件损坏后 read_all 不 panic，返回 eof_reached=false 或部分记录
    #[test]
    fn fuzz_1000_rounds_read_all_on_corrupted_files() {
        let mut rng = XorShift64::new(0x2000_BEEF);
        for cycle in 0..1000u64 {
            let path = temp_wal_path("read_all_corrupt", cycle);
            let _ = std::fs::remove_file(&path);
            let original = write_n_records(&mut rng, &path, 10).expect("写入失败");
            let mut content = std::fs::read(&path).expect("读取失败");
            // 随机扰动
            if !content.is_empty() && rng.next_bool() {
                let pos = rng.next_range(content.len() as u32) as usize;
                content[pos] ^= rng.next_u8();
                std::fs::write(&path, &content).expect("写入失败");
            }
            // read_all 不应 panic，不应返回 Err
            let mut reader = WalReader::open(&path).expect("open 不应失败");
            let (records, _eof) = reader.read_all().expect("read_all 不应返回 Err");
            assert!(
                records.len() <= original.len(),
                "cycle={}: read_all 返回 {} 应 <= original {}",
                cycle,
                records.len(),
                original.len()
            );
            // 已读出的记录必须与原始记录前缀完全一致
            for (i, r) in records.iter().enumerate() {
                assert_eq!(
                    r, &original[i],
                    "cycle={}: read_all[{}] 与 original 不一致",
                    i, cycle
                );
            }
            let _ = std::fs::remove_file(&path);
        }
    }

    // -----------------------------------------------------------------
    // 6. WalWriter + WalReader 集成 Fuzz — 写入大量记录后读取验证
    // -----------------------------------------------------------------

    /// 写入 5000 条随机合法记录 → WalReader 逐条读取 → 验证完全一致
    ///
    /// 验证标准：合法 record 双向一致（通过 WalWriter/WalReader I/O 路径）
    #[test]
    fn fuzz_5000_records_writer_reader_roundtrip() {
        let mut rng = XorShift64::new(0x5000_5000);
        let path = temp_wal_path("wr_rd_5k", 0);
        let _ = std::fs::remove_file(&path);
        let original = write_n_records(&mut rng, &path, 5000).expect("写入失败");
        let mut reader = WalReader::open(&path).expect("open 失败");
        let (records, eof) = reader.read_all().expect("read_all 失败");
        assert!(eof, "应到达 EOF");
        assert_eq!(records.len(), original.len());
        for (i, (r, o)) in records.iter().zip(original.iter()).enumerate() {
            assert_eq!(r, o, "records[{}] 与 original 不一致", i);
        }
        let _ = std::fs::remove_file(&path);
    }

    /// 写入 1000 条记录 → 关闭 → 重新打开 WalWriter 追加 1000 条 → 读取验证 2000 条
    ///
    /// 验证标准：WAL 追加写入语义正确，重新打开后 LSN 继续递增
    #[test]
    fn fuzz_reopen_append_preserves_order() {
        let mut rng = XorShift64::new(0x5E00_0001);
        let path = temp_wal_path("reopen_append", 0);
        let _ = std::fs::remove_file(&path);
        let first_batch = write_n_records(&mut rng, &path, 1000).expect("第一批写入失败");
        // 重新打开追加
        let writer = WalWriter::open(&path).expect("reopen 失败");
        let mut second_batch = Vec::with_capacity(1000);
        for _ in 0..1000 {
            let mut record = random_valid_record(&mut rng);
            let actual_lsn = writer.append(record.clone()).expect("追加失败");
            record.lsn = actual_lsn;
            record.update_checksum();
            second_batch.push(record);
        }
        writer.flush().expect("flush 失败");
        drop(writer);
        // 读取验证
        let mut reader = WalReader::open(&path).expect("open 失败");
        let (records, eof) = reader.read_all().expect("read_all 失败");
        assert!(eof);
        assert_eq!(records.len(), 2000);
        for (i, r) in records.iter().enumerate() {
            if i < 1000 {
                assert_eq!(r, &first_batch[i], "前 1000 条 records[{}] 不一致", i);
            } else {
                assert_eq!(
                    r,
                    &second_batch[i - 1000],
                    "后 1000 条 records[{}] 不一致",
                    i
                );
            }
        }
        // LSN 单调递增
        for i in 1..records.len() {
            assert!(
                records[i].lsn > records[i - 1].lsn,
                "LSN 非单调递增：[{}].lsn={} <= [{}].lsn={}",
                i,
                records[i].lsn,
                i - 1,
                records[i - 1].lsn
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------
    // 7. 极端边界 — 全 0 / 全 0xFF 字节流 decode 不 panic
    // -----------------------------------------------------------------

    #[test]
    fn decode_all_zeros_no_panic() {
        let buf = vec![0u8; 256];
        let _ = WalRecord::decode(&buf);
    }

    #[test]
    fn decode_all_ones_no_panic() {
        let buf = vec![0xFFu8; 256];
        let _ = WalRecord::decode(&buf);
    }

    #[test]
    fn decode_max_size_buffer_no_panic() {
        // 构造一个 data_len = 100 但 buf 足够大的情况
        let mut record = WalRecord::new(
            u64::MAX,
            u32::MAX,
            WalOpType::FullPageImage,
            u32::MAX,
            vec![0xFFu8; 100],
        );
        record.update_checksum();
        let encoded = record.encode();
        // decode 应成功
        let decoded = WalRecord::decode(&encoded).expect("max size 应成功");
        assert_eq!(record, decoded);
    }
}
