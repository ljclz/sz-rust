//! Cursor 游标 + TABLESAMPLE 表采样 — Phase 6.22
//!
//! 提供 PG 风格的游标（Cursor）和表采样（TABLESAMPLE）功能：
//!
//! - **Cursor**：`DECLARE c CURSOR FOR ...` → `FETCH [direction] n FROM c` → `MOVE n FROM c` → `CLOSE c`
//! - **TABLESAMPLE**：`SELECT * FROM t TABLESAMPLE {BERNOULLI|SYSTEM}(pct) [REPEATABLE(seed)]`
//!
//! # 设计
//!
//! ## Cursor
//!
//! - 游标在 `DECLARE` 时物化查询结果到 `Vec<Row>`（快照语义）
//! - 游标维护当前位置 `position`（0 = 首行之前；i = 已返回第 i 行）
//! - `FETCH` 返回行并移动位置；`MOVE` 仅移动位置不返回行
//! - **SCROLL** 游标支持双向遍历（FORWARD/BACKWARD/ABSOLUTE/RELATIVE/FIRST/LAST/PRIOR）
//! - **NO SCROLL** 游标仅支持前向 FETCH（PG 默认）
//! - 游标关闭后不可再 FETCH/MOVE
//!
//! ## TABLESAMPLE
//!
//! - **BERNOULLI**：行级采样 — 每行独立以概率 `pct/100` 被选中
//! - **SYSTEM**：块级采样 — 将行按 `block_size` 分块，每块以概率 `pct/100` 整体选中
//!   （PG 中 block = 8KB 页面；本实现使用可配置 block_size，默认 1000 行/块）
//! - **REPEATABLE(seed)**：使用种子化 RNG 保证可重复采样
//! - 采样百分比范围：0.0 ~ 100.0
//!
//! # 与 PG 的关系
//!
//! - PG 7.2+ 支持 Cursor（DECLARE/FETCH/MOVE/CLOSE）
//! - PG 9.5+ 支持 TABLESAMPLE（BERNOULLI / SYSTEM）
//! - PG 的 SCROLL 游标支持双向；NO SCROLL 仅前向（默认）
//! - PG 的 TABLESAMPLE SYSTEM 基于数据页（8KB），本实现基于行数块
//! - PG 的 REPEATABLE 使用种子保证同一查询同一数据返回相同结果
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **快照语义**：DECLARE 时物化全部结果（无增量/惰性求值）
//! - **无 WITH HOLD**：PG 的 `DECLARE c CURSOR WITH HOLD FOR ...`（事务外存活）未实现
//! - **无 UPDATE/DELETE WHERE CURRENT OF**：未实现定位修改
//! - **SYSTEM 块大小固定**：PG 基于实际页面大小，本实现使用固定 block_size
//! - **单线程**：无游标并发控制

use crate::executor::Row;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;

// =====================================================================
//  错误类型
// =====================================================================

/// 游标/采样错误
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CursorError {
    /// 游标不存在
    #[error("cursor '{0}' does not exist")]
    NotFound(String),
    /// 游标已关闭
    #[error("cursor '{0}' is closed")]
    Closed(String),
    /// 游标已存在（DECLARE 重名）
    #[error("cursor '{0}' already exists")]
    AlreadyExists(String),
    /// 非 SCROLL 游标不支持反向 FETCH
    #[error("cursor '{0}' is not scrollable; backward/absolute fetch not allowed")]
    NotScrollable(String),
    /// 采样百分比越界
    #[error("sampling percentage must be in [0.0, 100.0], got {0}")]
    InvalidPercentage(f64),
}

// =====================================================================
//  FETCH 方向
// =====================================================================

/// FETCH / MOVE 方向
///
/// 对应 PG 的 `FETCH { direction } FROM cursor` 语法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchDirection {
    /// `FETCH NEXT` — 前进 1 行（默认）
    Next,
    /// `FETCH PRIOR` — 后退 1 行（需 SCROLL）
    Prior,
    /// `FETCH FIRST` — 回到首行（需 SCROLL）
    First,
    /// `FETCH LAST` — 跳到末行（需 SCROLL）
    Last,
    /// `FETCH ABSOLUTE n` — 跳到第 n 行（1-based；0 = before first；-1 = last）（需 SCROLL）
    Absolute(i64),
    /// `FETCH RELATIVE n` — 从当前位置偏移 n 行（需 SCROLL）
    Relative(i64),
    /// `FETCH n` / `FETCH FORWARD n` — 前进 n 行
    Forward(usize),
    /// `FETCH BACKWARD n` — 后退 n 行（需 SCROLL）
    Backward(usize),
    /// `FETCH ALL` / `FETCH FORWARD ALL` — 前进到末尾
    ForwardAll,
    /// `FETCH BACKWARD ALL` — 后退到首行之前（需 SCROLL）
    BackwardAll,
}

impl FetchDirection {
    /// 是否需要 SCROLL 游标（非前向方向）
    fn requires_scroll(self) -> bool {
        matches!(
            self,
            Self::Prior
                | Self::First
                | Self::Last
                | Self::Absolute(_)
                | Self::Relative(_)
                | Self::Backward(_)
                | Self::BackwardAll
        )
    }
}

// =====================================================================
//  Cursor
// =====================================================================

/// 游标 — 物化查询结果的迭代器
///
/// PG 语义：
/// - `DECLARE c [SCROLL | NO SCROLL] CURSOR FOR query` 创建游标
/// - `position = 0` 表示在首行之前；`position = i` 表示已返回第 i 行（下一个是第 i+1 行）
/// - `FETCH` 返回行并移动 position；`MOVE` 仅移动 position
/// - 关闭后不可再用
pub struct Cursor {
    /// 游标名
    name: String,
    /// 物化的查询结果（DECLARE 时的快照）
    rows: Vec<Row>,
    /// 当前位置（0 = before first；i = 已返回第 i 行）
    position: i64,
    /// 是否可滚动（SCROLL）
    scrollable: bool,
    /// 是否已关闭
    closed: bool,
}

impl Cursor {
    /// 创建新游标
    ///
    /// - `scrollable = true` → SCROLL（双向）；`false` → NO SCROLL（仅前向）
    pub fn new(name: impl Into<String>, rows: Vec<Row>, scrollable: bool) -> Self {
        Self {
            name: name.into(),
            rows,
            position: 0,
            scrollable,
            closed: false,
        }
    }

    /// 游标名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 当前位置（0 = before first；i = 已返回第 i 行）
    pub fn position(&self) -> i64 {
        self.position
    }

    /// 物化行数
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// 是否可滚动
    pub fn is_scrollable(&self) -> bool {
        self.scrollable
    }

    /// 是否已关闭
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// 剩余行数（当前位置之后）
    pub fn remaining_forward(&self) -> usize {
        let total = self.rows.len() as i64;
        if self.position >= total {
            0
        } else {
            (total - self.position) as usize
        }
    }

    /// 前方已返回行数（当前位置之前，用于 BACKWARD）
    pub fn remaining_backward(&self) -> usize {
        self.position as usize
    }

    /// FETCH：返回行并移动位置
    ///
    /// 返回获取的行（可能为空）。方向需要 SCROLL 时，非 SCROLL 游标返回错误。
    pub fn fetch(&mut self, direction: FetchDirection) -> Result<Vec<Row>, CursorError> {
        if self.closed {
            return Err(CursorError::Closed(self.name.clone()));
        }
        if direction.requires_scroll() && !self.scrollable {
            return Err(CursorError::NotScrollable(self.name.clone()));
        }
        let (start, end, new_pos) = self.compute_range(direction);
        self.position = new_pos;
        Ok(self.rows[start..end].to_vec())
    }

    /// MOVE：仅移动位置，不返回行
    ///
    /// 返回移动的行数。
    pub fn move_cursor(&mut self, direction: FetchDirection) -> Result<usize, CursorError> {
        if self.closed {
            return Err(CursorError::Closed(self.name.clone()));
        }
        if direction.requires_scroll() && !self.scrollable {
            return Err(CursorError::NotScrollable(self.name.clone()));
        }
        let old_pos = self.position;
        let (_, _, new_pos) = self.compute_range(direction);
        self.position = new_pos;
        Ok((new_pos - old_pos).unsigned_abs() as usize)
    }

    /// 关闭游标
    pub fn close(&mut self) {
        self.closed = true;
        self.rows.clear();
        self.position = 0;
    }

    /// 计算给定方向的 [start, end) 区间和新位置
    ///
    /// 返回 (range_start, range_end, new_position)：
    /// - 返回行 = `rows[range_start..range_end]`
    /// - 新位置 = `new_position`
    ///
    /// 位置模型（gap 模型）：
    /// - position = 0：首行之前
    /// - position = k：已返回 k 行（在 row k-1 和 row k 之间，0-based）
    /// - position = total：末行之后
    ///
    /// 前向 FETCH：new_position = range_end（范围的右端）
    /// 后向 FETCH：new_position = range_start（范围的左端）
    fn compute_range(&self, direction: FetchDirection) -> (usize, usize, i64) {
        let total = self.rows.len() as i64;
        let pos = self.position;
        match direction {
            FetchDirection::Next => {
                let start = pos.clamp(0, total) as usize;
                let new_pos = (pos + 1).clamp(0, total);
                (start, new_pos as usize, new_pos)
            }
            FetchDirection::Forward(n) => {
                let start = pos.clamp(0, total) as usize;
                let new_pos = (pos + n as i64).clamp(0, total);
                (start, new_pos as usize, new_pos)
            }
            FetchDirection::ForwardAll => {
                let start = pos.clamp(0, total) as usize;
                (start, total as usize, total)
            }
            // 后向方向：new_position = range_start
            FetchDirection::Prior => {
                // 后退 1 行：返回 [pos-1, pos)，新位置 = pos-1
                let new_pos = (pos - 1).clamp(0, total);
                let end = pos.clamp(0, total) as usize;
                (new_pos as usize, end, new_pos)
            }
            FetchDirection::Backward(n) => {
                // 后退 n 行：返回 [pos-n, pos)，新位置 = pos-n
                let new_pos = (pos - n as i64).clamp(0, pos);
                let end = pos.clamp(0, total) as usize;
                (new_pos as usize, end, new_pos)
            }
            FetchDirection::BackwardAll => {
                // 后退到首行之前：返回 [0, pos)，新位置 = 0
                let end = pos.clamp(0, total) as usize;
                (0, end, 0)
            }
            FetchDirection::First => {
                // 回到首行：返回 [0, 1)，新位置 = 1（空表时 [0, 0)，新位置 = 0）
                if total == 0 {
                    (0, 0, 0)
                } else {
                    (0, 1, 1)
                }
            }
            FetchDirection::Last => {
                // 跳到末行：返回 [total-1, total)，新位置 = total
                if total == 0 {
                    (0, 0, 0)
                } else {
                    ((total - 1) as usize, total as usize, total)
                }
            }
            FetchDirection::Absolute(n) => {
                // ABSOLUTE n：1-based 定位
                // n > 0：定位到 row n（1-based），返回 row[n-1]（0-based），新位置 = n
                // n = 0：回到 before first，返回空，新位置 = 0
                // n < 0：从末尾定位，n=-1 = 最后一行
                //   target = total + n + 1（1-based row number）
                //   返回 row[target-1]（0-based），新位置 = target
                if n == 0 {
                    (0, 0, 0)
                } else if n > 0 {
                    let target = n.min(total);
                    if target == 0 {
                        (0, 0, 0)
                    } else {
                        let start = (target - 1) as usize;
                        (start, target as usize, target)
                    }
                } else {
                    // n < 0
                    let target = total + n + 1;
                    if target <= 0 {
                        (0, 0, 0)
                    } else {
                        let start = (target - 1) as usize;
                        (start, target as usize, target)
                    }
                }
            }
            FetchDirection::Relative(n) => {
                // RELATIVE n：从当前位置偏移 n，返回目标位置的 1 行（与 ABSOLUTE 语义一致）。
                // target = (pos + n).clamp(0, total)。
                // target == 0：游标位于首行之前，无当前行，返回空。
                // 否则：返回 row[target-1]（1-索引的第 target 行），新位置 = target。
                // n == 0：返回当前行 row[pos-1]，位置不变（pos == 0 时无当前行）。
                let target = (pos + n).clamp(0, total);
                if target == 0 {
                    (0, 0, 0)
                } else {
                    let start = (target - 1) as usize;
                    (start, target as usize, target)
                }
            }
        }
    }
}

// =====================================================================
//  CursorManager
// =====================================================================

/// 游标管理器 — 管理多个命名游标
///
/// 对应 PG 的会话级游标命名空间。
pub struct CursorManager {
    cursors: HashMap<String, Cursor>,
}

impl CursorManager {
    /// 创建空管理器
    pub fn new() -> Self {
        Self {
            cursors: HashMap::new(),
        }
    }

    /// DECLARE：创建新游标
    ///
    /// 若游标名已存在返回错误。
    pub fn declare(
        &mut self,
        name: impl Into<String>,
        rows: Vec<Row>,
        scrollable: bool,
    ) -> Result<(), CursorError> {
        let name = name.into();
        if self.cursors.contains_key(&name) {
            return Err(CursorError::AlreadyExists(name));
        }
        let cursor = Cursor::new(name.clone(), rows, scrollable);
        self.cursors.insert(name, cursor);
        Ok(())
    }

    /// FETCH：从游标获取行
    pub fn fetch(
        &mut self,
        name: &str,
        direction: FetchDirection,
    ) -> Result<Vec<Row>, CursorError> {
        let cursor = self
            .cursors
            .get_mut(name)
            .ok_or_else(|| CursorError::NotFound(name.to_string()))?;
        cursor.fetch(direction)
    }

    /// MOVE：移动游标位置
    pub fn move_cursor(
        &mut self,
        name: &str,
        direction: FetchDirection,
    ) -> Result<usize, CursorError> {
        let cursor = self
            .cursors
            .get_mut(name)
            .ok_or_else(|| CursorError::NotFound(name.to_string()))?;
        cursor.move_cursor(direction)
    }

    /// CLOSE：关闭游标
    pub fn close(&mut self, name: &str) -> Result<(), CursorError> {
        let cursor = self
            .cursors
            .get_mut(name)
            .ok_or_else(|| CursorError::NotFound(name.to_string()))?;
        cursor.close();
        Ok(())
    }

    /// 关闭所有游标
    pub fn close_all(&mut self) {
        for cursor in self.cursors.values_mut() {
            cursor.close();
        }
        self.cursors.clear();
    }

    /// 是否存在指定游标
    pub fn contains(&self, name: &str) -> bool {
        self.cursors.contains_key(name)
    }

    /// 获取游标（只读）
    pub fn get(&self, name: &str) -> Option<&Cursor> {
        self.cursors.get(name)
    }

    /// 所有游标名
    pub fn names(&self) -> Vec<String> {
        self.cursors.keys().cloned().collect()
    }

    /// 游标数量
    pub fn len(&self) -> usize {
        self.cursors.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.cursors.is_empty()
    }
}

impl Default for CursorManager {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  TABLESAMPLE
// =====================================================================

/// 采样方法
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleMethod {
    /// 行级采样 — 每行独立以概率 `pct/100` 被选中
    Bernoulli,
    /// 块级采样 — 将行分块，每块以概率 `pct/100` 整体选中
    System,
}

/// TABLESAMPLE 参数
///
/// 对应 PG 的 `TABLESAMPLE {BERNOULLI|SYSTEM}(pct) [REPEATABLE(seed)]`。
#[derive(Debug, Clone)]
pub struct TableSample {
    /// 采样方法
    method: SampleMethod,
    /// 采样百分比 [0.0, 100.0]
    percentage: f64,
    /// REPEATABLE 种子（None = 随机）
    seed: Option<u64>,
    /// SYSTEM 采样的块大小（行数/块；默认 1000）
    block_size: usize,
}

impl TableSample {
    /// 创建 TABLESAMPLE 参数
    ///
    /// - `percentage` 必须在 [0.0, 100.0] 范围内
    /// - `seed = None` → 非确定性采样；`Some(s)` → REPEATABLE
    pub fn new(
        method: SampleMethod,
        percentage: f64,
        seed: Option<u64>,
    ) -> Result<Self, CursorError> {
        if !(0.0..=100.0).contains(&percentage) {
            return Err(CursorError::InvalidPercentage(percentage));
        }
        Ok(Self {
            method,
            percentage,
            seed,
            block_size: 1000,
        })
    }

    /// 设置 SYSTEM 采样的块大小
    pub fn with_block_size(mut self, block_size: usize) -> Self {
        self.block_size = block_size.max(1);
        self
    }

    /// 采样方法
    pub fn method(&self) -> SampleMethod {
        self.method
    }

    /// 采样百分比
    pub fn percentage(&self) -> f64 {
        self.percentage
    }

    /// REPEATABLE 种子
    pub fn seed(&self) -> Option<u64> {
        self.seed
    }

    /// 块大小（SYSTEM 采样）
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// 执行采样
    ///
    /// 返回采样后的行子集（保持原始顺序）。
    pub fn sample(&self, rows: &[Row]) -> Vec<Row> {
        let mut rng = match self.seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_os_rng(),
        };
        match self.method {
            SampleMethod::Bernoulli => self.sample_bernoulli(rows, &mut rng),
            SampleMethod::System => self.sample_system(rows, &mut rng),
        }
    }

    /// BERNOULLI 采样：每行独立以概率 `pct/100` 被选中
    fn sample_bernoulli(&self, rows: &[Row], rng: &mut StdRng) -> Vec<Row> {
        let prob = self.percentage / 100.0;
        rows.iter()
            .filter(|_| rng.random_bool(prob))
            .cloned()
            .collect()
    }

    /// SYSTEM 采样：按块采样，每块以概率 `pct/100` 整体选中
    ///
    /// 块大小 = `self.block_size`。每块所有行要么全选要么全不选。
    fn sample_system(&self, rows: &[Row], rng: &mut StdRng) -> Vec<Row> {
        let prob = self.percentage / 100.0;
        let block_size = self.block_size;
        let mut result = Vec::new();
        for chunk in rows.chunks(block_size) {
            if rng.random_bool(prob) {
                result.extend(chunk.iter().cloned());
            }
        }
        result
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：生成 n 行 Int64 数据
    fn make_rows(n: usize) -> Vec<Row> {
        (0..n).map(|i| vec![Value::Int64(i as i64)]).collect()
    }

    use szrsql_types::value::Value;

    // -----------------------------------------------------------------
    //  Cursor 基本测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cursor_declare_and_fetch_next() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, false);
        assert_eq!(cursor.name(), "c");
        assert_eq!(cursor.row_count(), 5);
        assert_eq!(cursor.position(), 0);
        assert!(!cursor.is_scrollable());
        assert!(!cursor.is_closed());

        let fetched = cursor.fetch(FetchDirection::Next).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(0));
        assert_eq!(cursor.position(), 1);
    }

    #[test]
    fn test_cursor_fetch_forward_n() {
        let rows = make_rows(10);
        let mut cursor = Cursor::new("c", rows, false);

        let fetched = cursor.fetch(FetchDirection::Forward(3)).unwrap();
        assert_eq!(fetched.len(), 3);
        assert_eq!(fetched[0][0], Value::Int64(0));
        assert_eq!(fetched[2][0], Value::Int64(2));
        assert_eq!(cursor.position(), 3);

        let fetched = cursor.fetch(FetchDirection::Forward(3)).unwrap();
        assert_eq!(fetched.len(), 3);
        assert_eq!(fetched[0][0], Value::Int64(3));
        assert_eq!(fetched[2][0], Value::Int64(5));
        assert_eq!(cursor.position(), 6);
    }

    #[test]
    fn test_cursor_fetch_forward_all() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, false);

        let fetched = cursor.fetch(FetchDirection::Forward(2)).unwrap();
        assert_eq!(fetched.len(), 2);
        assert_eq!(cursor.position(), 2);

        let fetched = cursor.fetch(FetchDirection::ForwardAll).unwrap();
        assert_eq!(fetched.len(), 3);
        assert_eq!(fetched[0][0], Value::Int64(2));
        assert_eq!(fetched[2][0], Value::Int64(4));
        assert_eq!(cursor.position(), 5);
    }

    #[test]
    fn test_cursor_fetch_past_end_returns_fewer() {
        let rows = make_rows(3);
        let mut cursor = Cursor::new("c", rows, false);

        let fetched = cursor.fetch(FetchDirection::Forward(10)).unwrap();
        assert_eq!(fetched.len(), 3);
        assert_eq!(cursor.position(), 3);

        // 已到末尾，再 FETCH 返回空
        let fetched = cursor.fetch(FetchDirection::Next).unwrap();
        assert!(fetched.is_empty());
        assert_eq!(cursor.position(), 3);
    }

    #[test]
    fn test_cursor_move_without_fetching() {
        let rows = make_rows(10);
        let mut cursor = Cursor::new("c", rows, false);

        let moved = cursor.move_cursor(FetchDirection::Forward(3)).unwrap();
        assert_eq!(moved, 3);
        assert_eq!(cursor.position(), 3);

        // MOVE 后 FETCH 从新位置开始
        let fetched = cursor.fetch(FetchDirection::Next).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(3));
        assert_eq!(cursor.position(), 4);
    }

    #[test]
    fn test_cursor_move_forward_all() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, false);

        let moved = cursor.move_cursor(FetchDirection::ForwardAll).unwrap();
        assert_eq!(moved, 5);
        assert_eq!(cursor.position(), 5);

        let fetched = cursor.fetch(FetchDirection::Next).unwrap();
        assert!(fetched.is_empty());
    }

    #[test]
    fn test_cursor_close() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, false);

        let _ = cursor.fetch(FetchDirection::Next).unwrap();
        cursor.close();
        assert!(cursor.is_closed());

        // 关闭后 FETCH 返回错误
        let err = cursor.fetch(FetchDirection::Next).unwrap_err();
        assert_eq!(err, CursorError::Closed("c".to_string()));
    }

    #[test]
    fn test_cursor_close_then_move_errors() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, false);
        cursor.close();

        let err = cursor.move_cursor(FetchDirection::Next).unwrap_err();
        assert_eq!(err, CursorError::Closed("c".to_string()));
    }

    #[test]
    fn test_cursor_empty_result_set() {
        let rows: Vec<Row> = Vec::new();
        let mut cursor = Cursor::new("c", rows, false);

        let fetched = cursor.fetch(FetchDirection::Next).unwrap();
        assert!(fetched.is_empty());
        assert_eq!(cursor.position(), 0);

        let fetched = cursor.fetch(FetchDirection::ForwardAll).unwrap();
        assert!(fetched.is_empty());
    }

    // -----------------------------------------------------------------
    //  SCROLL 游标测试
    // -----------------------------------------------------------------

    #[test]
    fn test_scroll_cursor_fetch_prior() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, true);
        assert!(cursor.is_scrollable());

        // 前进 3 行
        let _ = cursor.fetch(FetchDirection::Forward(3)).unwrap();
        assert_eq!(cursor.position(), 3);

        // 后退 1 行（PRIOR）：返回第 3 行（position 2），新位置 = 2
        let fetched = cursor.fetch(FetchDirection::Prior).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(2));
        assert_eq!(cursor.position(), 2);
    }

    #[test]
    fn test_scroll_cursor_fetch_first() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, true);

        let _ = cursor.fetch(FetchDirection::Forward(3)).unwrap();
        assert_eq!(cursor.position(), 3);

        let fetched = cursor.fetch(FetchDirection::First).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(0));
        assert_eq!(cursor.position(), 1);
    }

    #[test]
    fn test_scroll_cursor_fetch_last() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, true);

        let fetched = cursor.fetch(FetchDirection::Last).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(4));
        assert_eq!(cursor.position(), 5);
    }

    #[test]
    fn test_scroll_cursor_fetch_absolute() {
        let rows = make_rows(10);
        let mut cursor = Cursor::new("c", rows, true);

        // ABSOLUTE 4：跳到第 4 行（1-based），返回第 4 行
        let fetched = cursor.fetch(FetchDirection::Absolute(4)).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(3));
        assert_eq!(cursor.position(), 4);

        // ABSOLUTE -1：跳到最后一行
        let fetched = cursor.fetch(FetchDirection::Absolute(-1)).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(9));
        assert_eq!(cursor.position(), 10);

        // ABSOLUTE 0：回到 before first，返回空
        let fetched = cursor.fetch(FetchDirection::Absolute(0)).unwrap();
        assert!(fetched.is_empty());
        assert_eq!(cursor.position(), 0);
    }

    #[test]
    fn test_scroll_cursor_fetch_relative() {
        let rows = make_rows(10);
        let mut cursor = Cursor::new("c", rows, true);

        // RELATIVE 3：从 0 前进 3 → 返回第 3 行
        let fetched = cursor.fetch(FetchDirection::Relative(3)).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(2));
        assert_eq!(cursor.position(), 3);

        // RELATIVE -1：从 3 后退 1 → 返回第 2 行
        let fetched = cursor.fetch(FetchDirection::Relative(-1)).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(1));
        assert_eq!(cursor.position(), 2);

        // RELATIVE 0：返回当前行（第 2 行）
        let fetched = cursor.fetch(FetchDirection::Relative(0)).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(1));
        assert_eq!(cursor.position(), 2);
    }

    #[test]
    fn test_scroll_cursor_fetch_backward_n() {
        let rows = make_rows(10);
        let mut cursor = Cursor::new("c", rows, true);

        // 前进到位置 5
        let _ = cursor.fetch(FetchDirection::Forward(5)).unwrap();
        assert_eq!(cursor.position(), 5);

        // 后退 3 行：返回 position 2..5 的行（3 行）
        let fetched = cursor.fetch(FetchDirection::Backward(3)).unwrap();
        assert_eq!(fetched.len(), 3);
        assert_eq!(fetched[0][0], Value::Int64(2));
        assert_eq!(fetched[1][0], Value::Int64(3));
        assert_eq!(fetched[2][0], Value::Int64(4));
        assert_eq!(cursor.position(), 2);
    }

    #[test]
    fn test_scroll_cursor_fetch_backward_all() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, true);

        let _ = cursor.fetch(FetchDirection::Forward(4)).unwrap();
        assert_eq!(cursor.position(), 4);

        // 后退到首行之前：返回 0..4 的行（4 行）
        let fetched = cursor.fetch(FetchDirection::BackwardAll).unwrap();
        assert_eq!(fetched.len(), 4);
        assert_eq!(cursor.position(), 0);
    }

    #[test]
    fn test_no_scroll_cursor_rejects_backward() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, false);

        let err = cursor.fetch(FetchDirection::Prior).unwrap_err();
        assert_eq!(err, CursorError::NotScrollable("c".to_string()));

        let err = cursor.fetch(FetchDirection::Backward(2)).unwrap_err();
        assert_eq!(err, CursorError::NotScrollable("c".to_string()));

        let err = cursor.fetch(FetchDirection::Absolute(1)).unwrap_err();
        assert_eq!(err, CursorError::NotScrollable("c".to_string()));
    }

    // -----------------------------------------------------------------
    //  CursorManager 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_cursor_manager_declare_and_fetch() {
        let mut mgr = CursorManager::new();
        assert!(mgr.is_empty());

        mgr.declare("c1", make_rows(5), false).unwrap();
        assert_eq!(mgr.len(), 1);
        assert!(mgr.contains("c1"));

        let fetched = mgr.fetch("c1", FetchDirection::Forward(2)).unwrap();
        assert_eq!(fetched.len(), 2);
    }

    #[test]
    fn test_cursor_manager_declare_duplicate_errors() {
        let mut mgr = CursorManager::new();
        mgr.declare("c1", make_rows(5), false).unwrap();

        let err = mgr.declare("c1", make_rows(3), false).unwrap_err();
        assert_eq!(err, CursorError::AlreadyExists("c1".to_string()));
    }

    #[test]
    fn test_cursor_manager_fetch_nonexistent_errors() {
        let mut mgr = CursorManager::new();
        let err = mgr.fetch("nope", FetchDirection::Next).unwrap_err();
        assert_eq!(err, CursorError::NotFound("nope".to_string()));
    }

    #[test]
    fn test_cursor_manager_move_and_fetch() {
        let mut mgr = CursorManager::new();
        mgr.declare("c1", make_rows(10), false).unwrap();

        let moved = mgr.move_cursor("c1", FetchDirection::Forward(3)).unwrap();
        assert_eq!(moved, 3);

        let fetched = mgr.fetch("c1", FetchDirection::Next).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0][0], Value::Int64(3));
    }

    #[test]
    fn test_cursor_manager_close() {
        let mut mgr = CursorManager::new();
        mgr.declare("c1", make_rows(5), false).unwrap();

        mgr.close("c1").unwrap();
        assert!(mgr.get("c1").unwrap().is_closed());

        // 关闭后 FETCH 返回错误
        let err = mgr.fetch("c1", FetchDirection::Next).unwrap_err();
        assert_eq!(err, CursorError::Closed("c1".to_string()));
    }

    #[test]
    fn test_cursor_manager_close_nonexistent_errors() {
        let mut mgr = CursorManager::new();
        let err = mgr.close("nope").unwrap_err();
        assert_eq!(err, CursorError::NotFound("nope".to_string()));
    }

    #[test]
    fn test_cursor_manager_multiple_cursors() {
        let mut mgr = CursorManager::new();
        mgr.declare("c1", make_rows(3), false).unwrap();
        mgr.declare("c2", make_rows(5), true).unwrap();
        mgr.declare("c3", make_rows(7), false).unwrap();

        assert_eq!(mgr.len(), 3);
        let mut names = mgr.names();
        names.sort();
        assert_eq!(names, vec!["c1", "c2", "c3"]);

        // 各游标独立位置
        let _ = mgr.fetch("c1", FetchDirection::Next).unwrap();
        let _ = mgr.fetch("c2", FetchDirection::Forward(3)).unwrap();
        assert_eq!(mgr.get("c1").unwrap().position(), 1);
        assert_eq!(mgr.get("c2").unwrap().position(), 3);
        assert_eq!(mgr.get("c3").unwrap().position(), 0);
    }

    #[test]
    fn test_cursor_manager_close_all() {
        let mut mgr = CursorManager::new();
        mgr.declare("c1", make_rows(3), false).unwrap();
        mgr.declare("c2", make_rows(5), false).unwrap();

        mgr.close_all();
        assert!(mgr.is_empty());
    }

    // -----------------------------------------------------------------
    //  TABLESAMPLE — BERNOULLI 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_tablesample_bernoulli_100_percent() {
        let rows = make_rows(100);
        let ts = TableSample::new(SampleMethod::Bernoulli, 100.0, Some(42)).unwrap();
        let sampled = ts.sample(&rows);
        assert_eq!(sampled.len(), 100);
    }

    #[test]
    fn test_tablesample_bernoulli_0_percent() {
        let rows = make_rows(100);
        let ts = TableSample::new(SampleMethod::Bernoulli, 0.0, Some(42)).unwrap();
        let sampled = ts.sample(&rows);
        assert!(sampled.is_empty());
    }

    #[test]
    fn test_tablesample_bernoulli_50_percent_repeatable() {
        let rows = make_rows(10000);
        let ts1 = TableSample::new(SampleMethod::Bernoulli, 50.0, Some(42)).unwrap();
        let ts2 = TableSample::new(SampleMethod::Bernoulli, 50.0, Some(42)).unwrap();
        let sampled1 = ts1.sample(&rows);
        let sampled2 = ts2.sample(&rows);

        // REPEATABLE → 相同种子相同结果
        assert_eq!(sampled1.len(), sampled2.len());
        assert_eq!(sampled1, sampled2);

        // 50% 采样 10000 行 → 期望 ~5000，允许 ±10% 误差
        let count = sampled1.len();
        assert!(count > 4000 && count < 6000, "expected ~5000, got {count}");
    }

    #[test]
    fn test_tablesample_bernoulli_different_seeds_differ() {
        let rows = make_rows(10000);
        let ts1 = TableSample::new(SampleMethod::Bernoulli, 50.0, Some(1)).unwrap();
        let ts2 = TableSample::new(SampleMethod::Bernoulli, 50.0, Some(2)).unwrap();
        let sampled1 = ts1.sample(&rows);
        let sampled2 = ts2.sample(&rows);

        // 不同种子 → 不同结果（极小概率相同，此处假设不同）
        assert_ne!(sampled1, sampled2);
    }

    #[test]
    fn test_tablesample_bernoulli_preserves_order() {
        let rows = make_rows(100);
        let ts = TableSample::new(SampleMethod::Bernoulli, 100.0, Some(42)).unwrap();
        let sampled = ts.sample(&rows);

        // 100% → 全部行，顺序不变
        for (i, row) in sampled.iter().enumerate() {
            assert_eq!(row[0], Value::Int64(i as i64));
        }
    }

    #[test]
    fn test_tablesample_bernoulli_empty_table() {
        let rows: Vec<Row> = Vec::new();
        let ts = TableSample::new(SampleMethod::Bernoulli, 50.0, Some(42)).unwrap();
        let sampled = ts.sample(&rows);
        assert!(sampled.is_empty());
    }

    #[test]
    fn test_tablesample_bernoulli_single_row() {
        let rows = make_rows(1);
        let ts_100 = TableSample::new(SampleMethod::Bernoulli, 100.0, Some(42)).unwrap();
        assert_eq!(ts_100.sample(&rows).len(), 1);

        let ts_0 = TableSample::new(SampleMethod::Bernoulli, 0.0, Some(42)).unwrap();
        assert_eq!(ts_0.sample(&rows).len(), 0);
    }

    // -----------------------------------------------------------------
    //  TABLESAMPLE — SYSTEM 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_tablesample_system_100_percent() {
        let rows = make_rows(100);
        let ts = TableSample::new(SampleMethod::System, 100.0, Some(42)).unwrap();
        let sampled = ts.sample(&rows);
        assert_eq!(sampled.len(), 100);
    }

    #[test]
    fn test_tablesample_system_0_percent() {
        let rows = make_rows(100);
        let ts = TableSample::new(SampleMethod::System, 0.0, Some(42)).unwrap();
        let sampled = ts.sample(&rows);
        assert!(sampled.is_empty());
    }

    #[test]
    fn test_tablesample_system_block_size() {
        let rows = make_rows(1000);
        // 块大小 100 → 10 个块
        let ts = TableSample::new(SampleMethod::System, 50.0, Some(42))
            .unwrap()
            .with_block_size(100);
        assert_eq!(ts.block_size(), 100);

        let sampled = ts.sample(&rows);
        // 50% 采样 10 个块 → 期望 ~5 块 = ~500 行
        // 块是整体选中，所以行数 = 选中块数 × 100
        assert!(
            sampled.len().is_multiple_of(100),
            "SYSTEM sampling should return whole blocks, got {}",
            sampled.len()
        );
        let blocks_selected = sampled.len() / 100;
        assert!(
            blocks_selected > 0 && blocks_selected < 10,
            "expected ~5 blocks, got {blocks_selected}"
        );
    }

    #[test]
    fn test_tablesample_system_repeatable() {
        let rows = make_rows(1000);
        let ts1 = TableSample::new(SampleMethod::System, 50.0, Some(99))
            .unwrap()
            .with_block_size(100);
        let ts2 = TableSample::new(SampleMethod::System, 50.0, Some(99))
            .unwrap()
            .with_block_size(100);

        let s1 = ts1.sample(&rows);
        let s2 = ts2.sample(&rows);
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_tablesample_system_preserves_order() {
        let rows = make_rows(100);
        let ts = TableSample::new(SampleMethod::System, 100.0, Some(42))
            .unwrap()
            .with_block_size(10);
        let sampled = ts.sample(&rows);

        // 100% → 全部行按原顺序
        for (i, row) in sampled.iter().enumerate() {
            assert_eq!(row[0], Value::Int64(i as i64));
        }
    }

    #[test]
    fn test_tablesample_system_empty_table() {
        let rows: Vec<Row> = Vec::new();
        let ts = TableSample::new(SampleMethod::System, 50.0, Some(42)).unwrap();
        let sampled = ts.sample(&rows);
        assert!(sampled.is_empty());
    }

    #[test]
    fn test_tablesample_system_small_block_size() {
        let rows = make_rows(10);
        // 块大小 1 → 等效于 BERNOULLI
        let ts = TableSample::new(SampleMethod::System, 100.0, Some(42))
            .unwrap()
            .with_block_size(1);
        let sampled = ts.sample(&rows);
        assert_eq!(sampled.len(), 10);
    }

    // -----------------------------------------------------------------
    //  TABLESAMPLE 参数校验
    // -----------------------------------------------------------------

    #[test]
    fn test_tablesample_invalid_percentage_negative() {
        let err = TableSample::new(SampleMethod::Bernoulli, -1.0, None).unwrap_err();
        assert_eq!(err, CursorError::InvalidPercentage(-1.0));
    }

    #[test]
    fn test_tablesample_invalid_percentage_over_100() {
        let err = TableSample::new(SampleMethod::Bernoulli, 100.1, None).unwrap_err();
        assert_eq!(err, CursorError::InvalidPercentage(100.1));
    }

    #[test]
    fn test_tablesample_boundary_0_and_100() {
        let rows = make_rows(10);
        // 0.0 边界
        let ts_0 = TableSample::new(SampleMethod::Bernoulli, 0.0, Some(42)).unwrap();
        assert_eq!(ts_0.sample(&rows).len(), 0);
        // 100.0 边界
        let ts_100 = TableSample::new(SampleMethod::Bernoulli, 100.0, Some(42)).unwrap();
        assert_eq!(ts_100.sample(&rows).len(), 10);
    }

    #[test]
    fn test_tablesample_method_accessors() {
        let ts = TableSample::new(SampleMethod::System, 25.0, Some(7))
            .unwrap()
            .with_block_size(500);
        assert_eq!(ts.method(), SampleMethod::System);
        assert_eq!(ts.percentage(), 25.0);
        assert_eq!(ts.seed(), Some(7));
        assert_eq!(ts.block_size(), 500);
    }

    #[test]
    fn test_tablesample_block_size_min_1() {
        let ts = TableSample::new(SampleMethod::System, 50.0, Some(42))
            .unwrap()
            .with_block_size(0); // 应被钳为 1
        assert_eq!(ts.block_size(), 1);
    }

    // -----------------------------------------------------------------
    //  统计学验证 — 大样本采样率
    // -----------------------------------------------------------------

    #[test]
    fn test_tablesample_bernoulli_statistical_accuracy() {
        // 100000 行 × 10% → 期望 ~10000 ± 5%
        let rows = make_rows(100_000);
        let ts = TableSample::new(SampleMethod::Bernoulli, 10.0, Some(123)).unwrap();
        let sampled = ts.sample(&rows);
        let count = sampled.len();
        let expected = 10_000;
        let tolerance = expected / 20; // 5%
        assert!(
            (count as i64 - expected as i64).unsigned_abs() < tolerance,
            "expected ~{expected}, got {count}"
        );
    }

    #[test]
    fn test_tablesample_system_statistical_accuracy() {
        // 100000 行 × 10% × 块大小 1000 → 100 块 × 10% → ~10 块 × 1000 = ~10000 行
        let rows = make_rows(100_000);
        let ts = TableSample::new(SampleMethod::System, 10.0, Some(123))
            .unwrap()
            .with_block_size(1000);
        let sampled = ts.sample(&rows);
        let count = sampled.len();
        // 块整体选中，所以 count 是 1000 的倍数
        assert!(
            count.is_multiple_of(1000),
            "expected multiple of 1000, got {count}"
        );
        // 期望 ~10 块（10000 行），允许 ±50%（SYSTEM 方差比 BERNOULLI 大）
        let blocks = count / 1000;
        assert!(
            blocks > 3 && blocks < 20,
            "expected ~10 blocks, got {blocks}"
        );
    }

    // -----------------------------------------------------------------
    //  remaining_forward / remaining_backward
    // -----------------------------------------------------------------

    #[test]
    fn test_cursor_remaining_forward() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, true);
        assert_eq!(cursor.remaining_forward(), 5);

        let _ = cursor.fetch(FetchDirection::Forward(2)).unwrap();
        assert_eq!(cursor.remaining_forward(), 3);
        assert_eq!(cursor.remaining_backward(), 2);
    }

    #[test]
    fn test_cursor_remaining_at_end() {
        let rows = make_rows(3);
        let mut cursor = Cursor::new("c", rows, false);
        let _ = cursor.fetch(FetchDirection::ForwardAll).unwrap();
        assert_eq!(cursor.remaining_forward(), 0);
        assert_eq!(cursor.remaining_backward(), 3);
    }

    // -----------------------------------------------------------------
    //  FETCH 方向 requires_scroll
    // -----------------------------------------------------------------

    #[test]
    fn test_fetch_direction_requires_scroll() {
        // 前向方向不需要 SCROLL
        assert!(!FetchDirection::Next.requires_scroll());
        assert!(!FetchDirection::Forward(5).requires_scroll());
        assert!(!FetchDirection::ForwardAll.requires_scroll());

        // 后向/绝对方向需要 SCROLL
        assert!(FetchDirection::Prior.requires_scroll());
        assert!(FetchDirection::First.requires_scroll());
        assert!(FetchDirection::Last.requires_scroll());
        assert!(FetchDirection::Absolute(1).requires_scroll());
        assert!(FetchDirection::Relative(1).requires_scroll());
        assert!(FetchDirection::Backward(5).requires_scroll());
        assert!(FetchDirection::BackwardAll.requires_scroll());
    }

    // -----------------------------------------------------------------
    //  Cursor 默认 NO SCROLL + 前向方向可用
    // -----------------------------------------------------------------

    #[test]
    fn test_no_scroll_cursor_allows_forward_directions() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, false);

        // Next / Forward(n) / ForwardAll 应该都可用
        let _ = cursor.fetch(FetchDirection::Next).unwrap();
        let _ = cursor.fetch(FetchDirection::Forward(2)).unwrap();
        let _ = cursor.fetch(FetchDirection::ForwardAll).unwrap();
        assert_eq!(cursor.position(), 5);
    }

    #[test]
    fn test_scroll_cursor_allows_all_directions() {
        let rows = make_rows(5);
        let mut cursor = Cursor::new("c", rows, true);

        let _ = cursor.fetch(FetchDirection::Forward(3)).unwrap();
        let _ = cursor.fetch(FetchDirection::Prior).unwrap();
        let _ = cursor.fetch(FetchDirection::First).unwrap();
        let _ = cursor.fetch(FetchDirection::Last).unwrap();
        assert_eq!(cursor.position(), 5);
    }
}
