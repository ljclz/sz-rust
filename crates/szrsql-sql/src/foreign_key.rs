//! 外键运行时校验 — Phase 3.29
//!
//! # 设计
//!
//! - **`ForeignKeyValidator`** — 静态方法集合，校验 INSERT/UPDATE/DELETE 操作不违反外键约束
//! - **校验类型**：
//!   - **INSERT 子表**：检查父表是否存在被引用的值
//!   - **UPDATE 子表 FK 列**：检查新值在父表中存在
//!   - **DELETE 父表**：根据 ON DELETE 动作（RESTRICT/NO ACTION 报错，CASCADE/SET NULL/SET DEFAULT 级联）
//!   - **UPDATE 父表 PK**：根据 ON UPDATE 动作级联
//! - **级联操作**：通过 `CascadeOp` 枚举返回，由调用方应用到子表
//!
//! # PG 兼容语义
//!
//! - `NO ACTION`（默认）：与 `RESTRICT` 类似，但延迟到事务末尾检查（当前实现等同 RESTRICT）
//! - `RESTRICT`：立即报错
//! - `CASCADE`：DELETE 父行时删除所有引用子行；UPDATE 父 PK 时更新子表 FK
//! - `SET NULL`：将子表 FK 列设为 NULL
//! - `SET DEFAULT`：将子表 FK 列设为默认值（当前未支持默认值，回退为 SET NULL）
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.29。

use crate::ast::ReferenceAction;
use crate::executor::{ExecutionError, Row, TableStorage};
use crate::plan::{ForeignKeyConstraint, ReferencingKey, TableSchema};
use szrsql_types::value::Value;

/// 查找闭包类型 — 输入表名，返回 `&'c dyn TableStorage`（生命周期 `'c` 由调用方指定）
///
/// 调用方需保证闭包返回的引用生命周期不短于 `'c`。
type LookupFn<'a, 'c> = &'a dyn Fn(&str) -> Option<&'c dyn TableStorage>;

// =====================================================================
//  级联操作
// =====================================================================

/// 级联操作（由 DELETE/UPDATE 父表触发）— Phase 3.29
#[derive(Debug, Clone, PartialEq)]
pub enum CascadeOp {
    /// 删除子表的指定行（CASCADE on DELETE）
    DeleteChildRow {
        /// 子表名（小写）
        table: String,
        /// 行 ID
        row_id: usize,
    },
    /// 更新子表的指定行（CASCADE on UPDATE / SET NULL / SET DEFAULT）
    UpdateChildRow {
        /// 子表名（小写）
        table: String,
        /// 行 ID
        row_id: usize,
        /// 列索引 → 新值
        updates: Vec<(usize, Value)>,
    },
}

// =====================================================================
//  ForeignKeyValidator
// =====================================================================

/// 外键校验器 — Phase 3.29
///
/// 所有方法均为静态方法，接收 schema + 表存储 lookup 进行校验。
pub struct ForeignKeyValidator;

impl ForeignKeyValidator {
    // -----------------------------------------------------------------
    //  INSERT 校验（子表侧）
    // -----------------------------------------------------------------

    /// 校验 INSERT 行不违反外键约束 — Phase 3.29
    ///
    /// 对每一列参与 FK 的值，检查父表中是否存在对应行。
    /// NULL 值跳过校验（SQL 标准允许 FK 列为 NULL，除非 NOT NULL）。
    pub fn validate_insert<'c>(
        schema: &TableSchema,
        row: &Row,
        fks: &[ForeignKeyConstraint],
        lookup: LookupFn<'_, 'c>,
    ) -> Result<(), ExecutionError> {
        for fk in fks {
            Self::check_parent_exists(schema, row, fk, lookup)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    //  UPDATE 校验（子表侧）
    // -----------------------------------------------------------------

    /// 校验 UPDATE 后的新行不违反外键约束 — Phase 3.29
    ///
    /// 仅检查 FK 列的值是否改变；若改变则验证新值在父表中存在。
    pub fn validate_update<'c>(
        schema: &TableSchema,
        old_row: &Row,
        new_row: &Row,
        fks: &[ForeignKeyConstraint],
        lookup: LookupFn<'_, 'c>,
    ) -> Result<(), ExecutionError> {
        for fk in fks {
            // 获取 FK 列索引
            let col_indices = Self::resolve_column_indices(schema, &fk.columns)?;
            // 检查 FK 列是否改变
            let changed = col_indices.iter().any(|&idx| {
                idx < old_row.len() && idx < new_row.len() && old_row[idx] != new_row[idx]
            });
            if changed {
                Self::check_parent_exists(schema, new_row, fk, lookup)?;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    //  DELETE 校验（父表侧）+ 级联操作收集
    // -----------------------------------------------------------------

    /// 校验 DELETE 不违反外键约束，并收集级联操作 — Phase 3.29
    ///
    /// 对每个引用本表的 FK：
    /// - `RESTRICT` / `NO ACTION`：若子表有引用行，报错
    /// - `CASCADE`：收集子表待删除行
    /// - `SET NULL`：收集子表待 NULL 化的行
    /// - `SET DEFAULT`：当前回退为 SET NULL
    ///
    /// 返回需要应用到子表的级联操作列表。
    pub fn collect_delete_cascades<'c>(
        parent_schema: &TableSchema,
        deleted_rows: &[(usize, Row)],
        referencing_keys: &[ReferencingKey],
        lookup: LookupFn<'_, 'c>,
    ) -> Result<Vec<CascadeOp>, ExecutionError> {
        let mut ops = Vec::new();

        for ref_key in referencing_keys {
            // 解析父表被引用列索引
            let parent_col_indices =
                Self::resolve_column_indices(parent_schema, &ref_key.parent_columns)?;

            // 查找子表
            let child_storage = lookup(&ref_key.child_table.name).ok_or_else(|| {
                ExecutionError::TableNotFound(ref_key.child_table.qualified_name())
            })?;

            // 解析子表 FK 列索引
            let child_schema = child_storage.schema();
            let child_col_indices =
                Self::resolve_column_indices(child_schema, &ref_key.child_columns)?;

            // 收集被删除行的引用键值
            let deleted_keys: Vec<Vec<Value>> = deleted_rows
                .iter()
                .map(|(_, row)| {
                    parent_col_indices
                        .iter()
                        .map(|&idx| row.get(idx).cloned().unwrap_or(Value::Null))
                        .collect()
                })
                .collect();

            // 扫描子表，查找引用了被删除行的子行
            for (child_row_id, child_row) in child_storage.scan_with_ids() {
                let child_key: Vec<Value> = child_col_indices
                    .iter()
                    .map(|&idx| child_row.get(idx).cloned().unwrap_or(Value::Null))
                    .collect();

                // 检查子行是否引用了被删除的父行（键值匹配且非 NULL）
                let references_deleted =
                    child_key
                        .iter()
                        .zip(deleted_keys.iter())
                        .any(|(child_val, deleted_keys)| {
                            !matches!(child_val, Value::Null) && deleted_keys.contains(child_val)
                        });

                if references_deleted {
                    match ref_key.on_delete {
                        ReferenceAction::Restrict | ReferenceAction::NoAction => {
                            return Err(ExecutionError::ForeignKeyViolation(format!(
                                "cannot delete row from {} (referenced by {}.{})",
                                parent_schema.name.qualified_name(),
                                ref_key.child_table.qualified_name(),
                                ref_key.child_columns.join(", ")
                            )));
                        }
                        ReferenceAction::Cascade => {
                            ops.push(CascadeOp::DeleteChildRow {
                                table: ref_key.child_table.name.to_lowercase(),
                                row_id: child_row_id,
                            });
                        }
                        ReferenceAction::SetNull | ReferenceAction::SetDefault => {
                            let updates: Vec<(usize, Value)> = child_col_indices
                                .iter()
                                .map(|&idx| (idx, Value::Null))
                                .collect();
                            ops.push(CascadeOp::UpdateChildRow {
                                table: ref_key.child_table.name.to_lowercase(),
                                row_id: child_row_id,
                                updates,
                            });
                        }
                    }
                }
            }
        }

        Ok(ops)
    }

    // -----------------------------------------------------------------
    //  UPDATE 父表 PK 校验 + 级联操作收集
    // -----------------------------------------------------------------

    /// 校验 UPDATE 父表 PK 不违反外键约束，并收集级联操作 — Phase 3.29
    ///
    /// 当父表被引用列的值改变时：
    /// - `RESTRICT` / `NO ACTION`：若子表有引用旧行，报错
    /// - `CASCADE`：更新子表 FK 列为新值
    /// - `SET NULL`：将子表 FK 列设为 NULL
    /// - `SET DEFAULT`：当前回退为 SET NULL
    pub fn collect_update_cascades<'c>(
        parent_schema: &TableSchema,
        updated_rows: &[(usize, Row, Row)], // (row_id, old_row, new_row)
        referencing_keys: &[ReferencingKey],
        lookup: LookupFn<'_, 'c>,
    ) -> Result<Vec<CascadeOp>, ExecutionError> {
        let mut ops = Vec::new();

        for ref_key in referencing_keys {
            let parent_col_indices =
                Self::resolve_column_indices(parent_schema, &ref_key.parent_columns)?;

            // 收集被更新的键值对 (old_key, new_key)
            let mut changed_keys: Vec<(Vec<Value>, Vec<Value>)> = Vec::new();
            for (_, old_row, new_row) in updated_rows {
                let old_key: Vec<Value> = parent_col_indices
                    .iter()
                    .map(|&idx| old_row.get(idx).cloned().unwrap_or(Value::Null))
                    .collect();
                let new_key: Vec<Value> = parent_col_indices
                    .iter()
                    .map(|&idx| new_row.get(idx).cloned().unwrap_or(Value::Null))
                    .collect();
                if old_key != new_key {
                    changed_keys.push((old_key, new_key));
                }
            }

            if changed_keys.is_empty() {
                continue;
            }

            // 查找子表
            let child_storage = lookup(&ref_key.child_table.name).ok_or_else(|| {
                ExecutionError::TableNotFound(ref_key.child_table.qualified_name())
            })?;
            let child_schema = child_storage.schema();
            let child_col_indices =
                Self::resolve_column_indices(child_schema, &ref_key.child_columns)?;

            // 扫描子表，查找引用了旧键值的子行
            for (child_row_id, child_row) in child_storage.scan_with_ids() {
                let child_key: Vec<Value> = child_col_indices
                    .iter()
                    .map(|&idx| child_row.get(idx).cloned().unwrap_or(Value::Null))
                    .collect();

                // 检查子行是否引用了被更新的父行
                for (old_key, new_key) in &changed_keys {
                    let matches_old = child_key
                        .iter()
                        .zip(old_key.iter())
                        .all(|(c, o)| !matches!(c, Value::Null) && c == o);
                    if !matches_old {
                        continue;
                    }

                    match ref_key.on_update {
                        ReferenceAction::Restrict | ReferenceAction::NoAction => {
                            return Err(ExecutionError::ForeignKeyViolation(format!(
                                "cannot update row in {} (referenced by {}.{})",
                                parent_schema.name.qualified_name(),
                                ref_key.child_table.qualified_name(),
                                ref_key.child_columns.join(", ")
                            )));
                        }
                        ReferenceAction::Cascade => {
                            let updates: Vec<(usize, Value)> = child_col_indices
                                .iter()
                                .zip(new_key.iter())
                                .map(|(&idx, val)| (idx, val.clone()))
                                .collect();
                            ops.push(CascadeOp::UpdateChildRow {
                                table: ref_key.child_table.name.to_lowercase(),
                                row_id: child_row_id,
                                updates,
                            });
                        }
                        ReferenceAction::SetNull | ReferenceAction::SetDefault => {
                            let updates: Vec<(usize, Value)> = child_col_indices
                                .iter()
                                .map(|&idx| (idx, Value::Null))
                                .collect();
                            ops.push(CascadeOp::UpdateChildRow {
                                table: ref_key.child_table.name.to_lowercase(),
                                row_id: child_row_id,
                                updates,
                            });
                        }
                    }
                    break; // 一个子行只匹配一个父键
                }
            }
        }

        Ok(ops)
    }

    // -----------------------------------------------------------------
    //  辅助方法
    // -----------------------------------------------------------------

    /// 检查父表中是否存在被引用的值 — Phase 3.29
    fn check_parent_exists<'c>(
        schema: &TableSchema,
        row: &Row,
        fk: &ForeignKeyConstraint,
        lookup: LookupFn<'_, 'c>,
    ) -> Result<(), ExecutionError> {
        // 解析子表 FK 列索引
        let child_col_indices = Self::resolve_column_indices(schema, &fk.columns)?;

        // 提取 FK 值（任一为 NULL 则跳过）
        let fk_values: Vec<Value> = child_col_indices
            .iter()
            .map(|&idx| row.get(idx).cloned().unwrap_or(Value::Null))
            .collect();

        if fk_values.iter().any(|v| matches!(v, Value::Null)) {
            // SQL 标准：FK 列含 NULL 时跳过校验（MATCH SIMPLE 语义）
            return Ok(());
        }

        // 查找父表
        let parent_storage = lookup(&fk.reference.table.name)
            .ok_or_else(|| ExecutionError::TableNotFound(fk.reference.table.qualified_name()))?;
        let parent_schema = parent_storage.schema();

        // 解析父表被引用列索引
        let parent_columns: Vec<String> = match &fk.reference.columns {
            Some(cols) => cols.clone(),
            None => {
                // 引用父表主键
                parent_schema
                    .columns
                    .iter()
                    .filter(|c| c.primary_key)
                    .map(|c| c.name.clone())
                    .next()
                    .map(|c| vec![c])
                    .ok_or_else(|| {
                        ExecutionError::ForeignKeyViolation(format!(
                            "cannot resolve PK of referenced table {}",
                            fk.reference.table.qualified_name()
                        ))
                    })?
            }
        };
        let parent_col_indices = Self::resolve_column_indices(parent_schema, &parent_columns)?;

        // 扫描父表，查找匹配行
        for parent_row in parent_storage.scan_iter() {
            let parent_values: Vec<Value> = parent_col_indices
                .iter()
                .map(|&idx| parent_row.get(idx).cloned().unwrap_or(Value::Null))
                .collect();
            if parent_values == fk_values {
                return Ok(()); // 找到匹配
            }
        }

        // 未找到匹配 — 违反 FK
        Err(ExecutionError::ForeignKeyViolation(format!(
            "INSERT into {} fails: value ({}) not found in referenced table {}",
            schema.name.qualified_name(),
            fk_values
                .iter()
                .map(|v| format!("{v:?}"))
                .collect::<Vec<_>>()
                .join(", "),
            fk.reference.table.qualified_name()
        )))
    }

    /// 解析列名到列索引 — Phase 3.29
    fn resolve_column_indices(
        schema: &TableSchema,
        column_names: &[String],
    ) -> Result<Vec<usize>, ExecutionError> {
        column_names
            .iter()
            .map(|name| {
                schema
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(name))
                    .ok_or_else(|| ExecutionError::ColumnNotFound(name.clone()))
            })
            .collect()
    }
}
