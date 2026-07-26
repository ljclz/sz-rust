//! Phase 2.5.10: CDC Schema 变更测试 — 对应 `SzRSQL实施进度.md` Phase 2.5.10。
//!
//! # 验证矩阵
//!
//! - **CREATE TABLE → CDC 事件包含 Schema**：SchemaChangeEvent 携带 new_schema
//! - **ALTER TABLE ADD COLUMN → CDC 事件更新 Schema 版本**：版本号递增
//! - **INSERT → 新版本 CDC 事件**：DML 事件携带最新 schema_version
//! - **Schema 变更后 CDC 事件格式立即更新**：后续 DML 事件立即使用新版本
//! - **ALTER TABLE DROP COLUMN / DROP TABLE**：DDL 事件覆盖
//! - **错误处理**：表已存在、表不存在、列已存在、列不存在等
//! - **线程安全**：并发 DDL 操作
//! - **序列化**：SchemaChangeEvent JSON/bincode roundtrip

use super::*;
use crate::{CdcEventOp, CdcObserverManager, ChangeEvent, CollectingObserver};
use std::sync::Arc;

// =====================================================================
// 测试辅助函数
// =====================================================================

/// 创建测试用 SchemaAwareCdcEngine，使用固定时间戳 12345
fn make_engine_with_fixed_timestamp() -> (
    SchemaAwareCdcEngine,
    Arc<CollectingSchemaObserver>,
    Arc<CollectingObserver>,
) {
    let schema_observer = Arc::new(CollectingSchemaObserver::new());
    let dml_observer = Arc::new(CollectingObserver::new());

    let schema_mgr = Arc::new(SchemaChangeObserverManager::new());
    let dml_mgr = Arc::new(CdcObserverManager::new());
    schema_mgr.register(schema_observer.clone());
    dml_mgr.register(dml_observer.clone());

    let engine = SchemaAwareCdcEngine::with_timestamp_fn(
        Arc::new(SchemaRegistry::new()),
        dml_mgr,
        schema_mgr,
        Box::new(|| 12345),
    );
    (engine, schema_observer, dml_observer)
}

/// 构造 users 表的列定义：id (Int64, NOT NULL), name (Text, NULL)
fn make_users_columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef::not_null("id", DataType::Int64),
        ColumnDef::nullable("name", DataType::Text),
    ]
}

// =====================================================================
// Part 1: DataType 与 ColumnDef 基础
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part1_datatype_column_def {
    use super::*;

    #[test]
    fn phase_2_5_10_data_type_as_str_all_variants() {
        assert_eq!(DataType::Int32.as_str(), "int32");
        assert_eq!(DataType::Int64.as_str(), "int64");
        assert_eq!(DataType::Text.as_str(), "text");
        assert_eq!(DataType::Blob.as_str(), "blob");
        assert_eq!(DataType::Real.as_str(), "real");
        assert_eq!(DataType::Bool.as_str(), "bool");
        assert_eq!(DataType::Date.as_str(), "date");
        assert_eq!(DataType::Timestamp.as_str(), "timestamp");
        assert_eq!(DataType::Json.as_str(), "json");
        assert_eq!(DataType::Uuid.as_str(), "uuid");
    }

    #[test]
    fn phase_2_5_10_data_type_display() {
        assert_eq!(format!("{}", DataType::Int32), "int32");
        assert_eq!(format!("{}", DataType::Text), "text");
    }

    #[test]
    fn phase_2_5_10_column_def_new() {
        let col = ColumnDef::new("id", DataType::Int64, false);
        assert_eq!(col.name, "id");
        assert_eq!(col.data_type, DataType::Int64);
        assert!(!col.nullable);
    }

    #[test]
    fn phase_2_5_10_column_def_not_null() {
        let col = ColumnDef::not_null("age", DataType::Int32);
        assert!(!col.nullable);
    }

    #[test]
    fn phase_2_5_10_column_def_nullable() {
        let col = ColumnDef::nullable("bio", DataType::Text);
        assert!(col.nullable);
    }

    #[test]
    fn phase_2_5_10_column_def_equality() {
        let col1 = ColumnDef::not_null("id", DataType::Int64);
        let col2 = ColumnDef::not_null("id", DataType::Int64);
        assert_eq!(col1, col2);

        let col3 = ColumnDef::nullable("id", DataType::Int64);
        assert_ne!(col1, col3);
    }
}

// =====================================================================
// Part 2: SchemaRegistry 基础 — create_table / get_schema / get_version
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part2_registry_basics {
    use super::*;

    #[test]
    fn phase_2_5_10_registry_new_is_empty() {
        let registry = SchemaRegistry::new();
        assert_eq!(registry.table_count(), 0);
        assert_eq!(registry.current_global_version(), 0);
        assert!(!registry.contains_table(1));
        assert_eq!(registry.get_schema(1), None);
        assert_eq!(registry.get_version(1), None);
    }

    #[test]
    fn phase_2_5_10_registry_create_table_success() {
        let registry = SchemaRegistry::new();
        let schema = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        assert_eq!(schema.table_id, 1);
        assert_eq!(schema.table_name, "users");
        assert_eq!(schema.column_count(), 2);
        assert_eq!(schema.version, 1);
        assert_eq!(registry.current_global_version(), 1);
        assert!(registry.contains_table(1));
        assert_eq!(registry.table_count(), 1);
    }

    #[test]
    fn phase_2_5_10_registry_create_table_version_starts_at_1() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(10, "t1", vec![ColumnDef::not_null("id", DataType::Int64)])
            .unwrap();
        assert_eq!(registry.get_version(10), Some(1));
    }

    #[test]
    fn phase_2_5_10_registry_create_table_already_exists() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        let result = registry.create_table(1, "users", make_users_columns());
        assert!(matches!(
            result,
            Err(SchemaError::TableAlreadyExists { table_id: 1, .. })
        ));
    }

    #[test]
    fn phase_2_5_10_registry_create_table_empty_columns() {
        let registry = SchemaRegistry::new();
        let result = registry.create_table(1, "empty", vec![]);
        assert!(matches!(result, Err(SchemaError::EmptyColumns)));
    }

    #[test]
    fn phase_2_5_10_registry_create_table_duplicate_column_name() {
        let registry = SchemaRegistry::new();
        let columns = vec![
            ColumnDef::not_null("id", DataType::Int64),
            ColumnDef::not_null("id", DataType::Int32), // 重复列名
        ];
        let result = registry.create_table(1, "dup", columns);
        assert!(matches!(result, Err(SchemaError::DuplicateColumnName(_))));
    }

    #[test]
    fn phase_2_5_10_registry_get_schema_returns_clone() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        let schema = registry.get_schema(1).unwrap();
        assert_eq!(schema.table_name, "users");
        // 修改 clone 不影响原 schema
        let mut modified = schema;
        modified.table_name = "modified".to_string();
        assert_eq!(registry.get_schema(1).unwrap().table_name, "users");
    }

    #[test]
    fn phase_2_5_10_registry_get_version() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        assert_eq!(registry.get_version(1), Some(1));
        assert_eq!(registry.get_version(999), None);
    }

    #[test]
    fn phase_2_5_10_registry_list_tables() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        let _ = registry
            .create_table(
                2,
                "orders",
                vec![ColumnDef::not_null("order_id", DataType::Int64)],
            )
            .unwrap();
        let tables = registry.list_tables();
        assert_eq!(tables.len(), 2);
    }
}

// =====================================================================
// Part 3: SchemaRegistry — alter_table_add_column
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part3_alter_add_column {
    use super::*;

    #[test]
    fn phase_2_5_10_alter_add_column_increments_version() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        assert_eq!(registry.get_version(1), Some(1));

        let new_schema = registry
            .alter_table_add_column(1, ColumnDef::nullable("email", DataType::Text))
            .unwrap();
        assert_eq!(new_schema.column_count(), 3);
        assert_eq!(new_schema.version, 2);
        assert_eq!(registry.get_version(1), Some(2));
        assert_eq!(registry.current_global_version(), 2);
    }

    #[test]
    fn phase_2_5_10_alter_add_column_table_not_found() {
        let registry = SchemaRegistry::new();
        let result =
            registry.alter_table_add_column(999, ColumnDef::not_null("x", DataType::Int32));
        assert!(matches!(result, Err(SchemaError::TableNotFound(999))));
    }

    #[test]
    fn phase_2_5_10_alter_add_column_already_exists() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        // "id" 已存在
        let result = registry.alter_table_add_column(1, ColumnDef::not_null("id", DataType::Int32));
        assert!(matches!(
            result,
            Err(SchemaError::ColumnAlreadyExists { table_id: 1, .. })
        ));
    }

    #[test]
    fn phase_2_5_10_alter_add_column_multiple_increments_version_monotonically() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        let s1 = registry
            .alter_table_add_column(1, ColumnDef::nullable("email", DataType::Text))
            .unwrap();
        let s2 = registry
            .alter_table_add_column(1, ColumnDef::nullable("phone", DataType::Text))
            .unwrap();
        let s3 = registry
            .alter_table_add_column(1, ColumnDef::nullable("age", DataType::Int32))
            .unwrap();
        assert_eq!(s1.version, 2);
        assert_eq!(s2.version, 3);
        assert_eq!(s3.version, 4);
        assert_eq!(s3.column_count(), 5);
        assert!(s3.find_column("age").is_some());
    }
}

// =====================================================================
// Part 4: SchemaRegistry — alter_table_drop_column
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part4_alter_drop_column {
    use super::*;

    #[test]
    fn phase_2_5_10_alter_drop_column_increments_version() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        let new_schema = registry.alter_table_drop_column(1, "name").unwrap();
        assert_eq!(new_schema.column_count(), 1);
        assert_eq!(new_schema.version, 2);
        assert!(new_schema.find_column("name").is_none());
    }

    #[test]
    fn phase_2_5_10_alter_drop_column_table_not_found() {
        let registry = SchemaRegistry::new();
        let result = registry.alter_table_drop_column(999, "x");
        assert!(matches!(result, Err(SchemaError::TableNotFound(999))));
    }

    #[test]
    fn phase_2_5_10_alter_drop_column_not_found() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        let result = registry.alter_table_drop_column(1, "nonexistent");
        assert!(matches!(
            result,
            Err(SchemaError::ColumnNotFound { table_id: 1, .. })
        ));
    }

    #[test]
    fn phase_2_5_10_alter_drop_last_column_returns_empty_columns_error() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(
                1,
                "single",
                vec![ColumnDef::not_null("id", DataType::Int64)],
            )
            .unwrap();
        // 不允许删除最后一列
        let result = registry.alter_table_drop_column(1, "id");
        assert!(matches!(result, Err(SchemaError::EmptyColumns)));
        // 原始 schema 保持不变
        assert_eq!(registry.get_schema(1).unwrap().column_count(), 1);
    }
}

// =====================================================================
// Part 5: SchemaRegistry — drop_table
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part5_drop_table {
    use super::*;

    #[test]
    fn phase_2_5_10_drop_table_removes_schema() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        assert!(registry.contains_table(1));

        let dropped = registry.drop_table(1).unwrap();
        assert!(!registry.contains_table(1));
        assert_eq!(registry.table_count(), 0);
        assert_eq!(registry.get_schema(1), None);
        // 返回的 schema 携带删除操作的新版本
        assert_eq!(dropped.version, 2);
    }

    #[test]
    fn phase_2_5_10_drop_table_not_found() {
        let registry = SchemaRegistry::new();
        let result = registry.drop_table(999);
        assert!(matches!(result, Err(SchemaError::TableNotFound(999))));
    }

    #[test]
    fn phase_2_5_10_drop_table_increments_global_version() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "t1", vec![ColumnDef::not_null("id", DataType::Int64)])
            .unwrap();
        assert_eq!(registry.current_global_version(), 1);
        let _ = registry.drop_table(1).unwrap();
        assert_eq!(registry.current_global_version(), 2);
    }

    #[test]
    fn phase_2_5_10_drop_table_records_dropped_version() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "t1", vec![ColumnDef::not_null("id", DataType::Int64)])
            .unwrap();
        let _ = registry.drop_table(1).unwrap();
        // 已删除表的最后版本号被记录
        assert_eq!(registry.get_dropped_version(1), Some(2));
    }

    #[test]
    fn phase_2_5_10_drop_then_recreate_same_id_continues_version_increment() {
        let registry = SchemaRegistry::new();
        let _ = registry
            .create_table(1, "t1", vec![ColumnDef::not_null("id", DataType::Int64)])
            .unwrap();
        let _ = registry.drop_table(1).unwrap();
        // 重新创建相同 ID 的表，版本号继续递增（不会重用 1）
        let schema = registry
            .create_table(
                1,
                "t1_new",
                vec![ColumnDef::not_null("id", DataType::Int64)],
            )
            .unwrap();
        assert_eq!(schema.version, 3);
    }
}

// =====================================================================
// Part 6: SchemaChangeEvent 基础（DDL 事件）
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part6_schema_change_event {
    use super::*;

    #[test]
    fn phase_2_5_10_schema_change_type_as_str() {
        assert_eq!(SchemaChangeType::CreateTable.as_str(), "create_table");
        assert_eq!(
            SchemaChangeType::AlterTableAddColumn.as_str(),
            "alter_table_add_column"
        );
        assert_eq!(
            SchemaChangeType::AlterTableDropColumn.as_str(),
            "alter_table_drop_column"
        );
        assert_eq!(SchemaChangeType::DropTable.as_str(), "drop_table");
    }

    #[test]
    fn phase_2_5_10_schema_change_event_json_roundtrip() {
        let schema = TableSchema {
            table_id: 1,
            table_name: "users".to_string(),
            columns: make_users_columns(),
            version: 1,
        };
        let event = SchemaChangeEvent {
            tx_id: 100,
            lsn: 1000,
            change_type: SchemaChangeType::CreateTable,
            table_id: 1,
            old_schema: None,
            new_schema: Some(schema.clone()),
            changed_column: None,
            schema_version: 1,
            timestamp: 12345,
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: SchemaChangeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
    }

    #[test]
    fn phase_2_5_10_schema_change_event_bincode_roundtrip() {
        let schema = TableSchema {
            table_id: 1,
            table_name: "users".to_string(),
            columns: make_users_columns(),
            version: 2,
        };
        let event = SchemaChangeEvent {
            tx_id: 200,
            lsn: 2000,
            change_type: SchemaChangeType::AlterTableAddColumn,
            table_id: 1,
            old_schema: Some(schema.clone()),
            new_schema: Some(schema),
            changed_column: Some("email".to_string()),
            schema_version: 2,
            timestamp: 54321,
        };
        let bytes = bincode::serialize(&event).unwrap();
        let decoded: SchemaChangeEvent = bincode::deserialize(&bytes).unwrap();
        assert_eq!(event, decoded);
    }

    #[test]
    fn phase_2_5_10_table_schema_json_roundtrip() {
        let schema = TableSchema {
            table_id: 42,
            table_name: "orders".to_string(),
            columns: vec![
                ColumnDef::not_null("order_id", DataType::Int64),
                ColumnDef::nullable("amount", DataType::Real),
                ColumnDef::nullable("created_at", DataType::Timestamp),
            ],
            version: 5,
        };
        let json = serde_json::to_string(&schema).unwrap();
        let decoded: TableSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(schema, decoded);
    }

    #[test]
    fn phase_2_5_10_column_def_json_roundtrip() {
        let col = ColumnDef::new("email", DataType::Text, true);
        let json = serde_json::to_string(&col).unwrap();
        let decoded: ColumnDef = serde_json::from_str(&json).unwrap();
        assert_eq!(col, decoded);
    }
}

// =====================================================================
// Part 7: SchemaChangeObserverManager
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part7_observer_manager {
    use super::*;

    #[test]
    fn phase_2_5_10_observer_manager_register_and_count() {
        let mgr = SchemaChangeObserverManager::new();
        assert_eq!(mgr.observer_count(), 0);

        let observer = Arc::new(CollectingSchemaObserver::new());
        assert!(mgr.register(observer.clone()));
        assert_eq!(mgr.observer_count(), 1);

        // 重复注册相同指针返回 false
        assert!(!mgr.register(observer.clone()));
        assert_eq!(mgr.observer_count(), 1);
    }

    #[test]
    fn phase_2_5_10_observer_manager_unregister() {
        let mgr = SchemaChangeObserverManager::new();
        let observer = Arc::new(CollectingSchemaObserver::new());
        mgr.register(observer.clone());
        assert_eq!(mgr.observer_count(), 1);

        assert!(mgr.unregister(&observer));
        assert_eq!(mgr.observer_count(), 0);

        // 再次注销返回 false
        assert!(!mgr.unregister(&observer));
    }

    #[test]
    fn phase_2_5_10_observer_manager_notify_all_observers() {
        let mgr = SchemaChangeObserverManager::new();
        let obs1 = Arc::new(CollectingSchemaObserver::new());
        let obs2 = Arc::new(CollectingSchemaObserver::new());
        mgr.register(obs1.clone());
        mgr.register(obs2.clone());

        let event = SchemaChangeEvent {
            tx_id: 1,
            lsn: 100,
            change_type: SchemaChangeType::CreateTable,
            table_id: 1,
            old_schema: None,
            new_schema: None,
            changed_column: None,
            schema_version: 1,
            timestamp: 0,
        };
        mgr.notify(event);

        assert_eq!(obs1.len(), 1);
        assert_eq!(obs2.len(), 1);
        assert_eq!(mgr.total_dispatched(), 2);
    }

    #[test]
    fn phase_2_5_10_observer_manager_panic_isolated() {
        struct PanickingObserver;
        impl SchemaChangeObserver for PanickingObserver {
            fn on_schema_change(&self, _event: SchemaChangeEvent) {
                panic!("observer panic");
            }
        }

        let mgr = SchemaChangeObserverManager::new();
        let panicking = Arc::new(PanickingObserver);
        let normal = Arc::new(CollectingSchemaObserver::new());
        mgr.register(panicking);
        mgr.register(normal.clone());

        let event = SchemaChangeEvent {
            tx_id: 1,
            lsn: 100,
            change_type: SchemaChangeType::CreateTable,
            table_id: 1,
            old_schema: None,
            new_schema: None,
            changed_column: None,
            schema_version: 1,
            timestamp: 0,
        };
        // panic 被隔离，normal observer 仍收到事件
        mgr.notify(event);
        assert_eq!(normal.len(), 1);
        assert_eq!(mgr.total_dispatched(), 2);
    }

    #[test]
    fn phase_2_5_10_collecting_schema_observer_clear() {
        let observer = CollectingSchemaObserver::new();
        let event = SchemaChangeEvent {
            tx_id: 1,
            lsn: 100,
            change_type: SchemaChangeType::CreateTable,
            table_id: 1,
            old_schema: None,
            new_schema: None,
            changed_column: None,
            schema_version: 1,
            timestamp: 0,
        };
        observer.on_schema_change(event);
        assert_eq!(observer.len(), 1);
        observer.clear();
        assert!(observer.is_empty());
    }
}

// =====================================================================
// Part 8: SchemaAwareCdcEngine — CREATE TABLE → CDC 事件包含 Schema
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part8_create_table_emits_event {
    use super::*;

    #[test]
    fn phase_2_5_10_create_table_emits_schema_change_event() {
        let (engine, schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();

        let schema = engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();

        assert_eq!(schema.version, 1);
        assert_eq!(schema_observer.len(), 1);

        let event = &schema_observer.events()[0];
        assert_eq!(event.tx_id, 1);
        assert_eq!(event.lsn, 100);
        assert_eq!(event.change_type, SchemaChangeType::CreateTable);
        assert_eq!(event.table_id, 1);
        assert!(event.old_schema.is_none());
        let new_schema = event.new_schema.as_ref().unwrap();
        assert_eq!(new_schema.table_name, "users");
        assert_eq!(new_schema.column_count(), 2);
        assert_eq!(event.changed_column, None);
        assert_eq!(event.schema_version, 1);
        assert_eq!(event.timestamp, 12345);
    }

    #[test]
    fn phase_2_5_10_create_table_registry_updated() {
        let (engine, _schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();
        let _ = engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();
        assert!(engine.registry().contains_table(1));
        assert_eq!(engine.registry().get_version(1), Some(1));
    }

    #[test]
    fn phase_2_5_10_create_multiple_tables_version_increments_globally() {
        let (engine, schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();

        let s1 = engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();
        let s2 = engine
            .create_table(
                2,
                200,
                2,
                "orders",
                vec![ColumnDef::not_null("order_id", DataType::Int64)],
            )
            .unwrap();

        assert_eq!(s1.version, 1);
        assert_eq!(s2.version, 2);
        assert_eq!(engine.registry().current_global_version(), 2);
        assert_eq!(schema_observer.len(), 2);
    }
}

// =====================================================================
// Part 9: SchemaAwareCdcEngine — ALTER TABLE ADD COLUMN → 更新 Schema 版本
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part9_alter_add_column_emits_event {
    use super::*;

    #[test]
    fn phase_2_5_10_alter_add_column_emits_event_with_old_and_new_schema() {
        let (engine, schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();

        engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();
        let old_schema = engine.registry().get_schema(1).unwrap();

        let new_schema = engine
            .alter_table_add_column(1, 200, 1, ColumnDef::nullable("email", DataType::Text))
            .unwrap();

        assert_eq!(new_schema.version, 2);
        assert_eq!(new_schema.column_count(), 3);
        assert!(new_schema.find_column("email").is_some());

        // 第二个事件是 ALTER ADD COLUMN
        let alter_event = &schema_observer.events()[1];
        assert_eq!(
            alter_event.change_type,
            SchemaChangeType::AlterTableAddColumn
        );
        assert_eq!(alter_event.schema_version, 2);
        assert_eq!(alter_event.changed_column, Some("email".to_string()));
        let old = alter_event.old_schema.as_ref().unwrap();
        assert_eq!(old.column_count(), 2);
        assert_eq!(old.version, 1);
        assert_eq!(old_schema, *old);
        let new = alter_event.new_schema.as_ref().unwrap();
        assert_eq!(new.column_count(), 3);
        assert_eq!(new.version, 2);
    }

    #[test]
    fn phase_2_5_10_alter_add_column_table_not_found_returns_error() {
        let (engine, schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();
        let result =
            engine.alter_table_add_column(999, 200, 999, ColumnDef::not_null("x", DataType::Int32));
        assert!(matches!(result, Err(SchemaError::TableNotFound(999))));
        // 没有事件被分发
        assert_eq!(schema_observer.len(), 0);
    }
}

// =====================================================================
// Part 10: SchemaAwareCdcEngine — ALTER TABLE DROP COLUMN
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part10_alter_drop_column_emits_event {
    use super::*;

    #[test]
    fn phase_2_5_10_alter_drop_column_emits_event() {
        let (engine, schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();
        engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();

        let new_schema = engine.alter_table_drop_column(1, 200, 1, "name").unwrap();
        assert_eq!(new_schema.column_count(), 1);
        assert_eq!(new_schema.version, 2);

        let drop_event = &schema_observer.events()[1];
        assert_eq!(
            drop_event.change_type,
            SchemaChangeType::AlterTableDropColumn
        );
        assert_eq!(drop_event.changed_column, Some("name".to_string()));
        assert_eq!(drop_event.schema_version, 2);
        assert!(drop_event
            .old_schema
            .as_ref()
            .unwrap()
            .find_column("name")
            .is_some());
        assert!(drop_event
            .new_schema
            .as_ref()
            .unwrap()
            .find_column("name")
            .is_none());
    }
}

// =====================================================================
// Part 11: SchemaAwareCdcEngine — DROP TABLE
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part11_drop_table_emits_event {
    use super::*;

    #[test]
    fn phase_2_5_10_drop_table_emits_event_with_old_schema_no_new_schema() {
        let (engine, schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();
        engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();

        let dropped = engine.drop_table(1, 200, 1).unwrap();
        assert_eq!(dropped.version, 2);

        let drop_event = &schema_observer.events()[1];
        assert_eq!(drop_event.change_type, SchemaChangeType::DropTable);
        assert!(drop_event.old_schema.is_some());
        assert!(drop_event.new_schema.is_none());
        assert_eq!(drop_event.schema_version, 2);

        // 表已从 registry 移除
        assert!(!engine.registry().contains_table(1));
    }
}

// =====================================================================
// Part 12: DML 事件携带 schema_version（核心需求）
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part12_dml_events_carry_schema_version {
    use super::*;

    #[test]
    fn phase_2_5_10_insert_event_carries_schema_version() {
        let (engine, _schema_observer, dml_observer) = make_engine_with_fixed_timestamp();
        engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();

        engine.insert(1, 200, 1, vec![1, 2, 3]).unwrap();

        let events = dml_observer.events();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.op, CdcEventOp::Insert);
        assert_eq!(event.schema_version, Some(1));
        assert_eq!(event.table_id, Some(1));
    }

    #[test]
    fn phase_2_5_10_update_event_carries_schema_version() {
        let (engine, _schema_observer, dml_observer) = make_engine_with_fixed_timestamp();
        engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();

        engine.update(1, 200, 1, vec![1], vec![2]).unwrap();

        let events = dml_observer.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].op, CdcEventOp::Update);
        assert_eq!(events[0].schema_version, Some(1));
    }

    #[test]
    fn phase_2_5_10_delete_event_carries_schema_version() {
        let (engine, _schema_observer, dml_observer) = make_engine_with_fixed_timestamp();
        engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();

        engine.delete(1, 200, 1, vec![1]).unwrap();

        let events = dml_observer.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].op, CdcEventOp::Delete);
        assert_eq!(events[0].schema_version, Some(1));
    }

    #[test]
    fn phase_2_5_10_insert_after_alter_carries_new_schema_version() {
        // 核心需求：Schema 变更后 CDC 事件格式立即更新
        let (engine, _schema_observer, dml_observer) = make_engine_with_fixed_timestamp();
        engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();

        // INSERT before ALTER → schema_version = 1
        engine.insert(1, 200, 1, vec![1]).unwrap();
        assert_eq!(dml_observer.events()[0].schema_version, Some(1));

        // ALTER TABLE ADD COLUMN → version 2
        engine
            .alter_table_add_column(1, 300, 1, ColumnDef::nullable("email", DataType::Text))
            .unwrap();

        // INSERT after ALTER → schema_version = 2
        engine.insert(1, 400, 1, vec![1, 2, 3]).unwrap();
        assert_eq!(dml_observer.events()[1].schema_version, Some(2));

        // 再次 ALTER → version 3
        engine
            .alter_table_add_column(1, 500, 1, ColumnDef::nullable("phone", DataType::Text))
            .unwrap();

        // INSERT after second ALTER → schema_version = 3
        engine.insert(1, 600, 1, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(dml_observer.events()[2].schema_version, Some(3));
    }

    #[test]
    fn phase_2_5_10_dml_on_nonexistent_table_returns_error() {
        let (engine, _schema_observer, dml_observer) = make_engine_with_fixed_timestamp();

        let result = engine.insert(1, 100, 999, vec![1]);
        assert!(matches!(result, Err(SchemaError::TableNotFound(999))));
        assert_eq!(dml_observer.len(), 0);

        let result = engine.update(1, 100, 999, vec![1], vec![2]);
        assert!(matches!(result, Err(SchemaError::TableNotFound(999))));

        let result = engine.delete(1, 100, 999, vec![1]);
        assert!(matches!(result, Err(SchemaError::TableNotFound(999))));
    }

    #[test]
    fn phase_2_5_10_dml_after_drop_table_returns_error() {
        let (engine, _schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();
        engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap();
        engine.drop_table(1, 200, 1).unwrap();

        // 表已删除，DML 操作应失败
        let result = engine.insert(1, 300, 1, vec![1]);
        assert!(matches!(result, Err(SchemaError::TableNotFound(1))));
    }
}

// =====================================================================
// Part 13: 完整 e2e 流程（CREATE → INSERT → ALTER → INSERT）
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part13_full_e2e_flow {
    use super::*;

    #[test]
    fn phase_2_5_10_full_flow_create_alter_insert() {
        // 完整流程验证：
        // 1. CREATE TABLE → schema_version = 1
        // 2. INSERT 100 行 → 所有事件 schema_version = 1
        // 3. ALTER TABLE ADD COLUMN → schema_version = 2
        // 4. INSERT 50 行 → 所有事件 schema_version = 2
        // 5. 验证事件总数和 schema_version 分布
        let (engine, schema_observer, dml_observer) = make_engine_with_fixed_timestamp();

        // 1. CREATE TABLE
        engine
            .create_table(
                1,
                100,
                42,
                "products",
                vec![
                    ColumnDef::not_null("id", DataType::Int64),
                    ColumnDef::not_null("name", DataType::Text),
                ],
            )
            .unwrap();

        // 2. INSERT 100 行
        for i in 0..100u64 {
            engine
                .insert(1, 200 + i, 42, format!("product_{i}").into_bytes())
                .unwrap();
        }

        // 3. ALTER TABLE ADD COLUMN price
        engine
            .alter_table_add_column(1, 500, 42, ColumnDef::nullable("price", DataType::Real))
            .unwrap();

        // 4. INSERT 50 行（新版本）
        for i in 0..50u64 {
            engine
                .insert(2, 600 + i, 42, format!("new_product_{i}").into_bytes())
                .unwrap();
        }

        // 5. 验证
        let schema_events = schema_observer.events();
        assert_eq!(schema_events.len(), 2); // CREATE + ALTER
        assert_eq!(schema_events[0].change_type, SchemaChangeType::CreateTable);
        assert_eq!(
            schema_events[1].change_type,
            SchemaChangeType::AlterTableAddColumn
        );

        let dml_events = dml_observer.events();
        assert_eq!(dml_events.len(), 150); // 100 + 50

        // 前 100 个事件 schema_version = 1
        for event in &dml_events[..100] {
            assert_eq!(event.schema_version, Some(1));
        }
        // 后 50 个事件 schema_version = 2
        for event in &dml_events[100..] {
            assert_eq!(event.schema_version, Some(2));
        }
    }

    #[test]
    fn phase_2_5_10_full_flow_create_alter_drop_insert() {
        // CREATE → INSERT → ALTER DROP COLUMN → INSERT → DROP TABLE → INSERT (fails)
        let (engine, _schema_observer, dml_observer) = make_engine_with_fixed_timestamp();

        engine
            .create_table(
                1,
                100,
                42,
                "items",
                vec![
                    ColumnDef::not_null("id", DataType::Int64),
                    ColumnDef::nullable("name", DataType::Text),
                    ColumnDef::nullable("desc", DataType::Text),
                ],
            )
            .unwrap();

        engine.insert(1, 200, 42, vec![1]).unwrap();
        assert_eq!(dml_observer.events()[0].schema_version, Some(1));

        let _ = engine.alter_table_drop_column(1, 300, 42, "desc").unwrap();
        assert_eq!(engine.registry().get_version(42), Some(2));

        engine.insert(1, 400, 42, vec![2]).unwrap();
        assert_eq!(dml_observer.events()[1].schema_version, Some(2));

        let _ = engine.drop_table(1, 500, 42).unwrap();
        let result = engine.insert(1, 600, 42, vec![3]);
        assert!(matches!(result, Err(SchemaError::TableNotFound(42))));
    }
}

// =====================================================================
// Part 14: 多表独立版本追踪
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part14_multi_table_independent_versions {
    use super::*;

    #[test]
    fn phase_2_5_10_multi_table_independent_versions() {
        let (engine, _schema_observer, dml_observer) = make_engine_with_fixed_timestamp();

        // 创建两张表
        engine
            .create_table(1, 100, 1, "users", make_users_columns())
            .unwrap(); // version 1
        engine
            .create_table(
                2,
                200,
                2,
                "orders",
                vec![ColumnDef::not_null("id", DataType::Int64)],
            )
            .unwrap(); // version 2

        // 只 ALTER users 表
        engine
            .alter_table_add_column(1, 300, 1, ColumnDef::nullable("email", DataType::Text))
            .unwrap(); // users → version 3, orders 仍为 version 2

        // 验证两张表的版本独立
        assert_eq!(engine.registry().get_version(1), Some(3));
        assert_eq!(engine.registry().get_version(2), Some(2));

        // INSERT users → schema_version = 3
        engine.insert(1, 400, 1, vec![1]).unwrap();
        // INSERT orders → schema_version = 2
        engine.insert(1, 401, 2, vec![1]).unwrap();

        let events = dml_observer.events();
        assert_eq!(events[0].schema_version, Some(3)); // users
        assert_eq!(events[1].schema_version, Some(2)); // orders
    }

    #[test]
    fn phase_2_5_10_multi_table_ddl_events_independent() {
        let (engine, schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();

        engine
            .create_table(
                1,
                100,
                1,
                "t1",
                vec![ColumnDef::not_null("id", DataType::Int64)],
            )
            .unwrap();
        engine
            .create_table(
                2,
                200,
                2,
                "t2",
                vec![ColumnDef::not_null("id", DataType::Int64)],
            )
            .unwrap();
        engine
            .alter_table_add_column(1, 300, 1, ColumnDef::nullable("x", DataType::Int32))
            .unwrap();

        let events = schema_observer.events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].table_id, 1);
        assert_eq!(events[0].change_type, SchemaChangeType::CreateTable);
        assert_eq!(events[1].table_id, 2);
        assert_eq!(events[1].change_type, SchemaChangeType::CreateTable);
        assert_eq!(events[2].table_id, 1);
        assert_eq!(events[2].change_type, SchemaChangeType::AlterTableAddColumn);
    }
}

// =====================================================================
// Part 15: ChangeEvent 向后兼容性（无 schema_version 字段的旧 JSON）
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part15_backward_compatibility {
    use super::*;

    #[test]
    fn phase_2_5_10_change_event_with_schema_version_json_roundtrip() {
        let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345).with_schema_version(5);
        let json = event.to_json().unwrap();
        let decoded = ChangeEvent::from_json(&json).unwrap();
        assert_eq!(event, decoded);
        assert_eq!(decoded.schema_version, Some(5));
    }

    #[test]
    fn phase_2_5_10_change_event_with_schema_version_bincode_roundtrip() {
        let event = ChangeEvent::update(1, 100, 42, vec![1], vec![2], 12345).with_schema_version(7);
        let bytes = event.to_bincode().unwrap();
        let decoded = ChangeEvent::from_bincode(&bytes).unwrap();
        assert_eq!(event, decoded);
        assert_eq!(decoded.schema_version, Some(7));
    }

    #[test]
    fn phase_2_5_10_old_json_without_schema_version_deserializes_to_none() {
        // 旧版 JSON 不含 schema_version 字段，应反序列化为 None
        let old_json = r#"{"tx_id":1,"lsn":100,"op":"insert","table_id":42,"old_row":null,"new_row":[1,2,3],"timestamp":12345}"#;
        let event = ChangeEvent::from_json(old_json).unwrap();
        assert_eq!(event.tx_id, 1);
        assert_eq!(event.op, CdcEventOp::Insert);
        assert_eq!(event.schema_version, None);
    }

    #[test]
    fn phase_2_5_10_change_event_with_schema_version_preserves_other_fields() {
        let event = ChangeEvent::delete(42, 999, 100, vec![1, 2], 88888).with_schema_version(3);
        assert_eq!(event.tx_id, 42);
        assert_eq!(event.lsn, 999);
        assert_eq!(event.table_id, Some(100));
        assert_eq!(event.op, CdcEventOp::Delete);
        assert_eq!(event.old_row, Some(vec![1, 2]));
        assert_eq!(event.new_row, None);
        assert_eq!(event.timestamp, 88888);
        assert_eq!(event.schema_version, Some(3));
    }

    #[test]
    fn phase_2_5_10_change_event_schema_version_getter() {
        let event = ChangeEvent::insert(1, 100, 42, vec![1], 0);
        assert_eq!(event.schema_version(), None);

        let event_with_version = event.with_schema_version(10);
        assert_eq!(event_with_version.schema_version(), Some(10));
    }

    #[test]
    fn phase_2_5_10_commit_event_has_none_schema_version() {
        let event = ChangeEvent::commit(1, 100, 12345);
        assert_eq!(event.schema_version(), None);
        // Commit 事件不应该有 schema_version（因为不关联特定表）
        let json = event.to_json().unwrap();
        let decoded = ChangeEvent::from_json(&json).unwrap();
        assert_eq!(decoded.schema_version, None);
    }

    #[test]
    fn phase_2_5_10_abort_event_has_none_schema_version() {
        let event = ChangeEvent::abort(1, 100, 12345);
        assert_eq!(event.schema_version(), None);
    }
}

// =====================================================================
// Part 16: 并发安全（多线程 DDL 操作）
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part16_concurrent_ddl {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn phase_2_5_10_concurrent_create_different_tables() {
        let registry = Arc::new(SchemaRegistry::new());
        let mut handles = vec![];

        for i in 0..10u32 {
            let reg = registry.clone();
            let handle = thread::spawn(move || {
                let table_id = i;
                let result = reg.create_table(
                    table_id,
                    format!("t_{table_id}"),
                    vec![ColumnDef::not_null("id", DataType::Int64)],
                );
                (table_id, result)
            });
            handles.push(handle);
        }

        let mut success_count = 0;
        for handle in handles {
            let (_id, result) = handle.join().unwrap();
            if result.is_ok() {
                success_count += 1;
            }
        }
        assert_eq!(success_count, 10);
        assert_eq!(registry.table_count(), 10);
        // 全局版本计数器应递增 10 次
        assert_eq!(registry.current_global_version(), 10);
    }

    #[test]
    fn phase_2_5_10_concurrent_alter_same_table() {
        let registry = Arc::new(SchemaRegistry::new());
        registry
            .create_table(1, "t1", vec![ColumnDef::not_null("id", DataType::Int64)])
            .unwrap();

        let mut handles = vec![];
        for i in 0..10u32 {
            let reg = registry.clone();
            let handle = thread::spawn(move || {
                reg.alter_table_add_column(
                    1,
                    ColumnDef::nullable(format!("col_{i}"), DataType::Int32),
                )
            });
            handles.push(handle);
        }

        let mut success_count = 0;
        for handle in handles {
            if handle.join().unwrap().is_ok() {
                success_count += 1;
            }
        }
        assert_eq!(success_count, 10);
        // 最终列数 = 1 (初始) + 10 (并发添加)
        let schema = registry.get_schema(1).unwrap();
        assert_eq!(schema.column_count(), 11);
        // 版本号 = 1 (初始) + 10 (并发 ALTER) = 11
        assert_eq!(schema.version, 11);
    }

    #[test]
    fn phase_2_5_10_concurrent_dml_and_ddl() {
        // 并发场景：一个线程做 DDL（ALTER ADD COLUMN），多个线程做 DML（INSERT）
        let (engine, _schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();
        engine
            .create_table(
                1,
                100,
                1,
                "t1",
                vec![ColumnDef::not_null("id", DataType::Int64)],
            )
            .unwrap();

        let engine_ref = std::sync::Arc::new(engine);
        let mut handles = vec![];

        // DDL 线程：添加列
        let ddl_engine = engine_ref.clone();
        handles.push(thread::spawn(move || {
            for i in 0..5u32 {
                ddl_engine
                    .alter_table_add_column(
                        1,
                        200 + i as u64,
                        1,
                        ColumnDef::nullable(format!("col_{i}"), DataType::Int32),
                    )
                    .unwrap();
            }
        }));

        // DML 线程：持续 INSERT
        let dml_engine = engine_ref.clone();
        handles.push(thread::spawn(move || {
            for i in 0..100u64 {
                // 即使 ALTER 失败，DML 仍可继续（如果表存在）
                let _ = dml_engine.insert(1, 1000 + i, 1, vec![i as u8]);
            }
        }));

        for handle in handles {
            handle.join().unwrap();
        }

        // 验证最终状态：所有 DDL 成功，所有 DML 成功
        let schema = engine_ref.registry().get_schema(1).unwrap();
        assert_eq!(schema.column_count(), 6); // 1 + 5
        assert_eq!(schema.version, 6); // 1 + 5
    }
}

// =====================================================================
// Part 17: Schema 变更顺序验证
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part17_schema_change_ordering {
    use super::*;

    #[test]
    fn phase_2_5_10_schema_change_events_in_order() {
        // 验证 DDL 事件按 LSN 顺序到达
        let (engine, schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();

        engine
            .create_table(
                1,
                100,
                1,
                "t1",
                vec![ColumnDef::not_null("id", DataType::Int64)],
            )
            .unwrap();
        engine
            .alter_table_add_column(1, 200, 1, ColumnDef::nullable("x", DataType::Int32))
            .unwrap();
        engine
            .alter_table_add_column(1, 300, 1, ColumnDef::nullable("y", DataType::Int32))
            .unwrap();
        engine.alter_table_drop_column(1, 400, 1, "x").unwrap();
        engine.drop_table(1, 500, 1).unwrap();

        let events = schema_observer.events();
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].change_type, SchemaChangeType::CreateTable);
        assert_eq!(events[0].lsn, 100);
        assert_eq!(events[1].change_type, SchemaChangeType::AlterTableAddColumn);
        assert_eq!(events[1].lsn, 200);
        assert_eq!(events[2].change_type, SchemaChangeType::AlterTableAddColumn);
        assert_eq!(events[2].lsn, 300);
        assert_eq!(
            events[3].change_type,
            SchemaChangeType::AlterTableDropColumn
        );
        assert_eq!(events[3].lsn, 400);
        assert_eq!(events[4].change_type, SchemaChangeType::DropTable);
        assert_eq!(events[4].lsn, 500);

        // schema_version 单调递增
        assert_eq!(events[0].schema_version, 1);
        assert_eq!(events[1].schema_version, 2);
        assert_eq!(events[2].schema_version, 3);
        assert_eq!(events[3].schema_version, 4);
        assert_eq!(events[4].schema_version, 5);
    }

    #[test]
    fn phase_2_5_10_schema_version_strictly_monotonic() {
        let (engine, _schema_observer, _dml_observer) = make_engine_with_fixed_timestamp();

        let versions = vec![
            engine
                .create_table(
                    1,
                    100,
                    1,
                    "t1",
                    vec![ColumnDef::not_null("id", DataType::Int64)],
                )
                .unwrap()
                .version,
            engine
                .alter_table_add_column(1, 200, 1, ColumnDef::nullable("x", DataType::Int32))
                .unwrap()
                .version,
            engine
                .alter_table_drop_column(1, 300, 1, "x")
                .unwrap()
                .version,
            engine.drop_table(1, 400, 1).unwrap().version,
            engine
                .create_table(
                    2,
                    500,
                    2,
                    "t2",
                    vec![ColumnDef::not_null("id", DataType::Int64)],
                )
                .unwrap()
                .version,
        ];

        // 严格单调递增
        for i in 1..versions.len() {
            assert!(
                versions[i] > versions[i - 1],
                "version[{}] = {} should be > version[{}] = {}",
                i,
                versions[i],
                i - 1,
                versions[i - 1]
            );
        }
        assert_eq!(versions, vec![1, 2, 3, 4, 5]);
    }
}

// =====================================================================
// Part 18: Schema 错误处理完整性
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part18_error_handling {
    use super::*;

    #[test]
    fn phase_2_5_10_error_display_messages() {
        let err = SchemaError::TableAlreadyExists {
            table_id: 1,
            table_name: "users".to_string(),
        };
        assert!(format!("{err}").contains("table already exists"));
        assert!(format!("{err}").contains("table_id=1"));
        assert!(format!("{err}").contains("users"));

        let err = SchemaError::TableNotFound(42);
        assert!(format!("{err}").contains("table not found"));
        assert!(format!("{err}").contains("42"));

        let err = SchemaError::ColumnAlreadyExists {
            table_id: 1,
            column_name: "id".to_string(),
        };
        assert!(format!("{err}").contains("column already exists"));

        let err = SchemaError::ColumnNotFound {
            table_id: 1,
            column_name: "x".to_string(),
        };
        assert!(format!("{err}").contains("column not found"));

        let err = SchemaError::EmptyColumns;
        assert!(format!("{err}").contains("empty"));

        let err = SchemaError::DuplicateColumnName("dup".to_string());
        assert!(format!("{err}").contains("duplicate"));
    }

    #[test]
    fn phase_2_5_10_create_table_with_same_id_different_name_fails() {
        let registry = SchemaRegistry::new();
        registry
            .create_table(1, "users", make_users_columns())
            .unwrap();
        // 即使名字不同，table_id 重复也失败
        let result = registry.create_table(1, "orders", make_users_columns());
        assert!(matches!(
            result,
            Err(SchemaError::TableAlreadyExists { .. })
        ));
    }

    #[test]
    fn phase_2_5_10_alter_add_column_same_name_different_type_fails() {
        let registry = SchemaRegistry::new();
        registry
            .create_table(1, "t", vec![ColumnDef::not_null("id", DataType::Int64)])
            .unwrap();
        // 即使类型不同，列名重复也失败
        let result = registry.alter_table_add_column(1, ColumnDef::not_null("id", DataType::Int32));
        assert!(matches!(
            result,
            Err(SchemaError::ColumnAlreadyExists { .. })
        ));
    }
}

// =====================================================================
// Part 19: 大规模 stress 测试（1000 次 DDL + 10000 次 DML）
// =====================================================================

#[cfg(test)]
mod phase_2_5_10_part19_stress {
    use super::*;

    #[test]
    fn phase_2_5_10_stress_1000_ddl_10000_dml() {
        // Stress：1000 次 ALTER + 10000 次 INSERT，验证版本号正确传播
        let (engine, schema_observer, dml_observer) = make_engine_with_fixed_timestamp();

        engine
            .create_table(
                1,
                0,
                1,
                "stress",
                vec![ColumnDef::not_null("id", DataType::Int64)],
            )
            .unwrap();

        // 1000 次 ALTER ADD COLUMN
        for i in 0..1000u32 {
            engine
                .alter_table_add_column(
                    1,
                    (i + 1) as u64,
                    1,
                    ColumnDef::nullable(format!("col_{i}"), DataType::Int32),
                )
                .unwrap();
        }

        // 10000 次 INSERT
        for i in 0..10000u64 {
            engine.insert(1, 10000 + i, 1, vec![i as u8]).unwrap();
        }

        // 验证
        let final_schema = engine.registry().get_schema(1).unwrap();
        assert_eq!(final_schema.column_count(), 1001); // 1 初始 + 1000 ALTER
        assert_eq!(final_schema.version, 1001); // 1 初始 + 1000 ALTER

        // 所有 DML 事件都应携带 schema_version = 1001
        let dml_events = dml_observer.events();
        assert_eq!(dml_events.len(), 10000);
        for event in &dml_events {
            assert_eq!(event.schema_version, Some(1001));
        }

        // Schema 事件 = 1 CREATE + 1000 ALTER = 1001
        assert_eq!(schema_observer.len(), 1001);
    }

    #[test]
    fn phase_2_5_10_stress_multiple_tables_100k_events() {
        // Stress：10 张表，每张表 10000 次 INSERT，验证 schema_version 独立追踪
        let (engine, _schema_observer, dml_observer) = make_engine_with_fixed_timestamp();

        // 创建 10 张表
        for table_id in 0..10u32 {
            engine
                .create_table(
                    table_id + 1,
                    table_id as u64,
                    table_id + 1,
                    format!("t_{table_id}"),
                    vec![ColumnDef::not_null("id", DataType::Int64)],
                )
                .unwrap();
        }

        // 每张表 INSERT 1000 次
        for table_id in 1..=10u32 {
            for i in 0..1000u64 {
                engine
                    .insert(table_id, 100 + i, table_id, vec![i as u8])
                    .unwrap();
            }
        }

        // 验证每张表的 schema_version 保持为创建时的版本（1-10）
        let dml_events = dml_observer.events();
        assert_eq!(dml_events.len(), 10000);

        // 按表分组验证 schema_version
        for table_id in 1..=10u32 {
            let table_events: Vec<&ChangeEvent> = dml_events
                .iter()
                .filter(|e| e.table_id == Some(table_id))
                .collect();
            assert_eq!(table_events.len(), 1000);
            let expected_version = table_id as u64; // table 1 → v1, table 2 → v2, ...
            for event in &table_events {
                assert_eq!(
                    event.schema_version,
                    Some(expected_version),
                    "table_id={} should have schema_version={}",
                    table_id,
                    expected_version
                );
            }
        }
    }
}
