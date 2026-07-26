# ADR-0008: Page Size 16KB

- **状态**: Accepted
- **日期**: 2026-07-24
- **决策类型**: 存储引擎
- **相关代码**: `crates/szrsql-storage/src/page.rs`
- **修复编号**: 无

## 背景

SzRSQL 存储引擎需要选择 Page 大小，候选方案：

1. **4KB**：与主流 OS page size 对齐，内存利用率高；缺点：B+Tree fanout 小，树高增大，IO 次数多。
2. **8KB**：MySQL InnoDB 默认；缺点：大行数据需 overflow，索引深度仍偏高。
3. **16KB**：MySQL InnoDB 最大支持；优点：fanout 大、树高浅、SSD 友好；缺点：单页内存占用大。
4. **32KB / 64KB**：更大 page；缺点：内存压力大，buffer pool 命中率下降，写入放大。

需求约束：
- B+Tree 索引深度应 ≤ 4（千万级数据）
- SSD IO 对齐友好（SSD 内部 page 通常 4KB/8KB，16KB 是整数倍）
- buffer pool 内存占用可控
- 大于 MySQL 默认 16KB 对齐，便于跨平台兼容

不选 16KB 的后果：
- 4KB/8KB 下千万级数据 B+Tree 深度 5-6，查询需 5-6 次 IO
- 32KB+ 下 buffer pool 可缓存页数减半，命中率下降
- 与 MySQL 16KB 对齐不一致，迁移工具复杂

## 决策

采用 **16KB Page**，作为存储引擎最小 IO 单位。

关键设计：

- **Page 结构**：`{ header, cell_array, free_space }`
  - header：`{ page_id, page_type, checksum, lsn, prev_page_id, next_page_id }`
  - cell_array：变长记录数组
  - free_space：空闲字节数
- **B+Tree fanout**：16KB page 下，假设单 key 16 字节 + pointer 8 字节，单页可容纳 ~640 个 key，3 层 B+Tree 可索引 640^3 ≈ 2.6 亿行
- **Checksum**：每页尾部 CRC32 校验，防止页损坏
- **Buffer Pool 对齐**：buffer pool 单位为 16KB，与 page size 一致

关键代码（`crates/szrsql-storage/src/page.rs`）：

```rust
pub const PAGE_SIZE: usize = 16 * 1024;  // 16KB

pub struct Page {
    pub data: [u8; PAGE_SIZE],
}

impl Page {
    pub fn header(&self) -> &PageHeader {
        unsafe { &*(self.data.as_ptr() as *const PageHeader) }
    }

    pub fn checksum(&self) -> u32 {
        // 校验 page 尾部 CRC32，防止页损坏
        crc32(&self.data[..PAGE_SIZE - 4])
    }

    pub fn verify(&self) -> Result<(), PageError> {
        let stored = self.header().checksum;
        let computed = self.checksum();
        if stored != computed {
            return Err(PageError::ChecksumMismatch);
        }
        Ok(())
    }
}

pub struct PageHeader {
    pub page_id: u64,
    pub page_type: PageType,  // Leaf | Internal | Overflow | Meta
    pub checksum: u32,
    pub lsn: u64,             // 最近修改的 WAL LSN
    pub prev_page_id: u64,
    pub next_page_id: u64,
    pub free_space_offset: u16,
    pub cell_count: u16,
}
```

## 后果

**正面**：
- B+Tree fanout 大（~640），3 层即可索引 2.6 亿行，查询 IO ≤ 3 次
- 与 MySQL InnoDB 16KB 对齐，迁移工具简单
- SSD 友好（16KB = 2× 8KB SSD page，对齐写入）
- Checksum 防止页损坏

**负面**：
- 单页 16KB 内存占用大，buffer pool 容量受限（1GB 内存仅 65536 页）
- 小行场景下页内空间浪费（如 100 字节行，单页 160 行，剩余 free space）
- 写放大：单行修改需写整个 16KB page
- 与 4KB OS page 不对齐，需 direct IO 绕过 OS buffer

## 注意事项

### 调用方约束
- 所有 IO 必须以 16KB 为单位，不可读写部分 page
- buffer pool 容量必须为 PAGE_SIZE 整数倍
- 写入前必须更新 checksum，读取后必须 verify
- 大行（> 16KB）需走 overflow page 机制

### 迁移路径
- 当前 16KB 固定，未来若支持可配置需考虑：
  1. Page size 作为启动参数（4/8/16/32KB 可选）
  2. Buffer pool 动态调整
  3. B+Tree 兼容不同 page size
- 跨 page size 迁移：需导出 + 重新导入（不可在线转换）

### Bug 定位提示

**如果出现页损坏（数据错乱或 checksum 报错）**：
1. **查 checksum 校验**：`Page::verify()` 是否在读取时调用，`checksum` 是否在写入时更新
2. **查 LSN 一致性**：page LSN 必须 ≤ WAL 持久化 LSN，否则可能读到未持久化的页（崩溃恢复 bug）
3. **查 direct IO 配置**：是否绕过 OS buffer，避免 OS buffer 与 buffer pool 不一致

**如果出现性能低（IO 次数多或延迟高）**：
1. **查 page size vs workload pattern**：
   - 点查多：16KB 适中（fanout 大，树浅）
   - 范围扫描多：16KB 友好（单页容纳多行）
   - 大行多：考虑 overflow page
2. **查 B+Tree 深度**：`EXPLAIN` 查看索引深度，> 4 则考虑重建索引
3. **查 buffer pool 命中率**：命中率 < 95% 则增大 buffer pool 或缩小 page size

**如果出现 buffer pool 内存不足**：
1. **查 page count**：`buffer_pool_size / 16KB` 是否合理（建议 ≥ 100 万页）
2. **查冷热数据分布**：冷数据占 buffer pool 应 < 20%，否则需增大 buffer pool
3. **优化**：对冷数据使用压缩页（page compression）

**如果出现写放大严重（写入 QPS 远低于磁盘 IOPS）**：
1. **查 page 修改模式**：单行修改触发整页 16KB 写入，写入放大 = 16KB / 行大小
2. **查 WAL 集成**：是否启用 WAL + Group Commit 摊销 fsync（见 ADR-0009）
3. **优化**：批量写入 + page merge 减少写放大

**如果跨 page size 迁移失败**：
1. **检查导出导入**：必须导出为逻辑格式（SQL/CSV），不可直接拷贝二进制 page
2. **可排除**：业务逻辑（page size 是存储层决策）
