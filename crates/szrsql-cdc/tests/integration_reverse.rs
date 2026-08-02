//! P5-3 反向链路集成测试 — 端到端验证 SourceConnector + ReverseReplicator
//!
//! 测试策略：
//! 1. 使用 Mock 模式注入事件（避免依赖真实 PG）
//! 2. 验证完整生命周期：连接 → 结构迁移 → 全量快照 → CDC 流
//! 3. 验证错误处理：源端错误、目标端错误、状态机错误
//! 4. 验证位点管理：ack_offset / confirmed_offset

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use szrsql_cdc::decoder::DecodedRow;
use szrsql_cdc::schema::{ColumnDef, DataType, TableSchema};
use szrsql_cdc::source::pg_source::PgSourceConnector;
use szrsql_cdc::source::reverse::{
    ReverseReplicator, ReverseReplicatorError, ReverseReplicatorState,
};
use szrsql_cdc::source::{
    create_source_connector, SchemaProvider, SnapshotProvider, SourceConfig, SourceConnector,
    SourceError, SourceEvent, SourceEventProvider, SourceOffset,
};
use szrsql_cdc::target::{TargetWriter, WriterError};
use szrsql_cdc::{CdcEventOp, ChangeEvent};
use szrsql_types::value::Value as SzValue;

// =====================================================================
// 辅助函数
// =====================================================================

fn make_schema(table_id: u32, table_name: &str) -> TableSchema {
    TableSchema {
        table_id,
        table_name: table_name.to_string(),
        columns: vec![
            ColumnDef {
                name: "id".to_string(),
                data_type: DataType::Int64,
                nullable: false,
            },
            ColumnDef {
                name: "name".to_string(),
                data_type: DataType::Text,
                nullable: true,
            },
            ColumnDef {
                name: "age".to_string(),
                data_type: DataType::Int32,
                nullable: true,
            },
        ],
        version: 1,
    }
}

fn make_row(id: i64, name: &str, age: i32) -> DecodedRow {
    DecodedRow {
        columns: vec![
            ("id".to_string(), SzValue::Int64(id)),
            ("name".to_string(), SzValue::Text(name.to_string())),
            ("age".to_string(), SzValue::Int64(age as i64)),
        ],
    }
}

fn make_events(table_name: &str) -> Vec<SourceEvent> {
    vec![
        SourceEvent::insert(1, "public", table_name, make_row(1, "Alice", 30), 1000),
        SourceEvent::insert(2, "public", table_name, make_row(2, "Bob", 25), 1001),
        SourceEvent::commit(3, 100, 1002),
        SourceEvent::update(
            4,
            "public",
            table_name,
            make_row(1, "Alice", 30),
            make_row(1, "Alice", 31),
            1003,
        ),
        SourceEvent::commit(5, 101, 1004),
        SourceEvent::delete(6, "public", table_name, make_row(2, "Bob", 25), 1005),
        SourceEvent::commit(7, 102, 1006),
    ]
}

// =====================================================================
// Mock Target Writer
// =====================================================================

struct CollectingTargetWriter {
    events: Mutex<Vec<(CdcEventOp, Option<DecodedRow>)>>,
    ensure_calls: AtomicUsize,
    fail_on_write: AtomicBool,
    health_ok: AtomicBool,
}

impl CollectingTargetWriter {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            ensure_calls: AtomicUsize::new(0),
            fail_on_write: AtomicBool::new(false),
            health_ok: AtomicBool::new(true),
        }
    }

    fn events(&self) -> Vec<(CdcEventOp, Option<DecodedRow>)> {
        self.events.lock().unwrap().clone()
    }
}

impl TargetWriter for CollectingTargetWriter {
    fn write_event(
        &self,
        event: &ChangeEvent,
        _schema: &TableSchema,
        row: Option<&DecodedRow>,
    ) -> Result<(), WriterError> {
        if self.fail_on_write.load(Ordering::SeqCst) {
            return Err(WriterError::Connection("mock write failure".to_string()));
        }
        self.events.lock().unwrap().push((event.op, row.cloned()));
        Ok(())
    }

    fn ensure_table(&self, _schema: &TableSchema) -> Result<(), WriterError> {
        self.ensure_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn health_check(&self) -> Result<(), WriterError> {
        if self.health_ok.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(WriterError::Connection("health check failed".to_string()))
        }
    }

    fn target_type(&self) -> &'static str {
        "mock_collecting"
    }
}

// =====================================================================
// 工厂方法测试
// =====================================================================

#[test]
fn integration_create_source_connector_pg_postgres_alias() {
    let cfg = SourceConfig::postgres("postgresql://localhost/db");
    let connector = create_source_connector(&cfg);
    assert!(connector.is_ok());
    assert_eq!(connector.unwrap().source_type(), "postgres");
}

#[test]
fn integration_create_source_connector_pg_alias() {
    let mut cfg = SourceConfig::postgres("postgresql://localhost/db");
    cfg.source_type = "pg".to_string();
    let connector = create_source_connector(&cfg);
    assert!(connector.is_ok());
}

#[test]
fn integration_create_source_connector_postgresql_alias() {
    let mut cfg = SourceConfig::postgres("postgresql://localhost/db");
    cfg.source_type = "postgresql".to_string();
    let connector = create_source_connector(&cfg);
    assert!(connector.is_ok());
}

#[test]
fn integration_create_source_connector_unsupported() {
    let mut cfg = SourceConfig::postgres("postgresql://localhost/db");
    cfg.source_type = "redis".to_string();
    let result = create_source_connector(&cfg);
    assert!(result.is_err());
}

// =====================================================================
// PgSourceConnector 集成测试
// =====================================================================

#[test]
fn integration_pg_source_connect_disconnect_cycle() {
    let connector =
        PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
    assert!(!connector.is_connected());
    connector.connect().unwrap();
    assert!(connector.is_connected());
    connector.disconnect().unwrap();
    assert!(!connector.is_connected());
}

#[test]
fn integration_pg_source_with_event_provider_full_flow() {
    let events = make_events("users");
    let event_count = Arc::new(AtomicUsize::new(0));
    let ec = event_count.clone();

    let provider: SourceEventProvider = Arc::new(move || {
        let n = ec.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Ok(Some(events.clone()))
        } else {
            Ok(None)
        }
    });

    let connector = PgSourceConnector::with_providers(
        SourceConfig::postgres("postgresql://localhost/db"),
        provider,
        None,
        None,
    )
    .unwrap();

    connector.connect().unwrap();

    let received = Arc::new(AtomicUsize::new(0));
    let r = received.clone();
    connector
        .start_cdc_stream(0, &move |events| {
            r.fetch_add(events.len(), Ordering::SeqCst);
            Ok(())
        })
        .unwrap();

    assert_eq!(received.load(Ordering::SeqCst), 7);
    assert_eq!(connector.confirmed_offset().unwrap().lsn, 7);
}

#[test]
fn integration_pg_source_schema_discovery_via_provider() {
    let schema = make_schema(1, "users");
    let schema_provider: SchemaProvider = Arc::new(move |_tables| Ok(vec![schema.clone()]));

    let connector = PgSourceConnector::with_providers(
        SourceConfig::postgres("postgresql://localhost/db"),
        Arc::new(|| Ok(None)),
        Some(schema_provider),
        None,
    )
    .unwrap();

    connector.connect().unwrap();
    let schemas = connector.discover_schemas(&[]).unwrap();
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].table_name, "users");
    assert_eq!(schemas[0].columns.len(), 3);
}

#[test]
fn integration_pg_source_snapshot_via_provider() {
    let snapshot: SnapshotProvider = Arc::new(|_table, _batch| {
        Ok((1..=10)
            .map(|i| make_row(i, &format!("user{}", i), 20 + i as i32))
            .collect())
    });

    let connector = PgSourceConnector::with_providers(
        SourceConfig::postgres("postgresql://localhost/db"),
        Arc::new(|| Ok(None)),
        None,
        Some(snapshot),
    )
    .unwrap();

    connector.connect().unwrap();

    let total = Arc::new(AtomicUsize::new(0));
    let t = total.clone();
    let count = connector
        .extract_snapshot("users", 4, &move |rows| {
            t.fetch_add(rows.len(), Ordering::SeqCst);
            Ok(())
        })
        .unwrap();

    assert_eq!(count, 10);
    assert_eq!(total.load(Ordering::SeqCst), 10);
}

#[test]
fn integration_pg_source_ack_offset_advances() {
    let connector =
        PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
    connector.connect().unwrap();

    assert_eq!(connector.confirmed_offset().unwrap().lsn, 0);
    connector.ack_offset(&SourceOffset::new(100)).unwrap();
    assert_eq!(connector.confirmed_offset().unwrap().lsn, 100);
    connector.ack_offset(&SourceOffset::new(50)).unwrap(); // 较小不应覆盖
    assert_eq!(connector.confirmed_offset().unwrap().lsn, 100);
    connector.ack_offset(&SourceOffset::new(200)).unwrap();
    assert_eq!(connector.confirmed_offset().unwrap().lsn, 200);
}

#[test]
fn integration_pg_source_health_check_state() {
    let connector =
        PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
    assert!(connector.health_check().is_err());
    connector.connect().unwrap();
    assert!(connector.health_check().is_ok());
    connector.disconnect().unwrap();
    assert!(connector.health_check().is_err());
}

#[test]
fn integration_pg_source_pg_type_mapping_complete() {
    // 完整覆盖所有支持的 PG 类型
    let cases = vec![
        ("int2", DataType::Int32),
        ("int4", DataType::Int32),
        ("int8", DataType::Int64),
        ("text", DataType::Text),
        ("varchar", DataType::Text),
        ("bytea", DataType::Blob),
        ("float4", DataType::Real),
        ("float8", DataType::Real),
        ("bool", DataType::Bool),
        ("date", DataType::Date),
        ("timestamp", DataType::Timestamp),
        ("json", DataType::Json),
        ("uuid", DataType::Uuid),
    ];
    for (pg_type, expected) in cases {
        let result = PgSourceConnector::pg_type_to_szrsql(pg_type);
        assert!(result.is_ok(), "failed to map {}", pg_type);
        assert_eq!(result.unwrap(), expected, "mismatch for {}", pg_type);
    }
}

#[test]
fn integration_pg_source_pg_type_with_length_modifier() {
    assert_eq!(
        PgSourceConnector::pg_type_to_szrsql("varchar(255)").unwrap(),
        DataType::Text
    );
    assert_eq!(
        PgSourceConnector::pg_type_to_szrsql("char(10)").unwrap(),
        DataType::Text
    );
    assert_eq!(
        PgSourceConnector::pg_type_to_szrsql("numeric(10,2)").unwrap(),
        DataType::Real
    );
}

#[test]
fn integration_pg_source_make_schema_helper() {
    let schema = PgSourceConnector::make_schema(
        "orders",
        42,
        vec![
            ("order_id".to_string(), "int8".to_string(), false),
            ("customer_id".to_string(), "int4".to_string(), false),
            ("total".to_string(), "numeric(10,2)".to_string(), true),
            ("data".to_string(), "jsonb".to_string(), true),
        ],
    )
    .unwrap();

    assert_eq!(schema.table_id, 42);
    assert_eq!(schema.table_name, "orders");
    assert_eq!(schema.columns.len(), 4);
    assert_eq!(schema.columns[0].data_type, DataType::Int64);
    assert!(!schema.columns[0].nullable);
    assert_eq!(schema.columns[1].data_type, DataType::Int32);
    assert_eq!(schema.columns[2].data_type, DataType::Real);
    assert_eq!(schema.columns[3].data_type, DataType::Json);
}

// =====================================================================
// ReverseReplicator 集成测试
// =====================================================================

#[test]
fn integration_reverse_end_to_end_full_lifecycle() {
    let schema = make_schema(1, "users");
    let events = make_events("users");
    let mut snapshot = HashMap::new();
    snapshot.insert("users".to_string(), vec![make_row(0, "Initial", 0)]);

    let source = Arc::new(MockSourceForIntegration::new(
        events,
        vec![schema],
        snapshot,
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let replicator = ReverseReplicator::new("rev_pg_to_szrsql", source, target.clone());

    // 启动反向复制
    replicator.start().unwrap();

    // 状态应为 Stopped
    assert_eq!(replicator.state(), ReverseReplicatorState::Stopped);

    // 统计验证
    let stats = replicator.stats();
    // 1 快照行 + 7 CDC 事件 = 8 个事件被处理
    assert_eq!(stats.events_processed, 7); // CDC 事件数
    assert_eq!(stats.snapshot_rows, 1);
    assert_eq!(stats.snapshot_tables, 1);
    assert_eq!(stats.confirmed_lsn, 7);

    // 目标端验证：1 快照行 + 4 DML（2 Insert + 1 Update + 1 Delete）
    let written = target.events();
    assert_eq!(written.len(), 5);
    assert_eq!(written[0].0, CdcEventOp::Insert); // 快照
    assert_eq!(written[1].0, CdcEventOp::Insert); // CDC Insert
    assert_eq!(written[2].0, CdcEventOp::Insert); // CDC Insert
    assert_eq!(written[3].0, CdcEventOp::Update); // CDC Update
    assert_eq!(written[4].0, CdcEventOp::Delete); // CDC Delete
}

#[test]
fn integration_reverse_pause_resume_not_supported_in_sync_mode() {
    // 由于 start_cdc_stream 是阻塞的，pause 在测试中难以模拟（需要异步流）
    // 这里仅验证状态机：从 Created 直接 pause 应失败
    let source = Arc::new(MockSourceForIntegration::new(
        vec![],
        vec![],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target);

    // 从 Created 状态 pause 应失败
    let result = r.pause();
    assert!(result.is_err());
}

#[test]
fn integration_reverse_state_machine_transitions() {
    let source = Arc::new(MockSourceForIntegration::new(
        vec![],
        vec![],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target);

    assert_eq!(r.state(), ReverseReplicatorState::Created);

    // Created → stop → Stopped
    r.stop().unwrap();
    assert_eq!(r.state(), ReverseReplicatorState::Stopped);

    // Stopped → start 应失败
    let result = r.start();
    assert!(result.is_err());
}

#[test]
fn integration_reverse_schema_migration_calls_ensure_table() {
    let schema1 = make_schema(1, "users");
    let schema2 = make_schema(2, "orders");
    let source = Arc::new(MockSourceForIntegration::new(
        vec![],
        vec![schema1, schema2],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target.clone());

    r.start().unwrap();

    // 2 个表应触发 2 次 ensure_table
    assert_eq!(target.ensure_calls.load(Ordering::SeqCst), 2);
}

#[test]
fn integration_reverse_target_write_failure_marks_failed() {
    let schema = make_schema(1, "users");
    let events = vec![SourceEvent::insert(
        1,
        "public",
        "users",
        make_row(1, "Alice", 30),
        1000,
    )];
    let source = Arc::new(MockSourceForIntegration::new(
        events,
        vec![schema],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    target.fail_on_write.store(true, Ordering::SeqCst);

    let r = ReverseReplicator::new("task1", source, target)
        .with_max_retries(2)
        .with_retry_interval(10);

    let result = r.start();
    assert!(result.is_err());
    assert_eq!(r.state(), ReverseReplicatorState::Failed);

    let stats = r.stats();
    assert!(stats.errors > 0);
}

#[test]
fn integration_reverse_empty_source_no_writes() {
    let schema = make_schema(1, "users");
    let source = Arc::new(MockSourceForIntegration::new(
        vec![],
        vec![schema],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target.clone());

    r.start().unwrap();

    assert_eq!(target.events().len(), 0);
    assert_eq!(r.stats().events_processed, 0);
    assert_eq!(r.stats().snapshot_rows, 0);
    assert_eq!(r.state(), ReverseReplicatorState::Stopped);
}

#[test]
fn integration_reverse_mixed_dml_events_correct_writes() {
    let schema = make_schema(1, "users");
    let events = vec![
        SourceEvent::insert(1, "public", "users", make_row(1, "Alice", 30), 1000),
        SourceEvent::update(
            2,
            "public",
            "users",
            make_row(1, "Alice", 30),
            make_row(1, "Alice", 31),
            1001,
        ),
        SourceEvent::delete(3, "public", "users", make_row(1, "Alice", 31), 1002),
    ];
    let source = Arc::new(MockSourceForIntegration::new(
        events,
        vec![schema],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target.clone());

    r.start().unwrap();

    let written = target.events();
    assert_eq!(written.len(), 3);
    assert_eq!(written[0].0, CdcEventOp::Insert);
    assert_eq!(written[1].0, CdcEventOp::Update);
    assert_eq!(written[2].0, CdcEventOp::Delete);
}

#[test]
fn integration_reverse_commit_abort_not_written() {
    let schema = make_schema(1, "users");
    let events = vec![
        SourceEvent::commit(1, 100, 1000),
        SourceEvent::abort(2, 101, 1001),
    ];
    let source = Arc::new(MockSourceForIntegration::new(
        events,
        vec![schema],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target.clone());

    r.start().unwrap();

    // Commit/Abort 不应写入目标端
    assert_eq!(target.events().len(), 0);
    // 但应计入 events_processed
    assert_eq!(r.stats().events_processed, 2);
}

#[test]
fn integration_reverse_offset_persistence_through_acks() {
    let schema = make_schema(1, "users");
    let events = vec![
        SourceEvent::insert(100, "public", "users", make_row(1, "Alice", 30), 1000),
        SourceEvent::commit(200, 100, 1001),
        SourceEvent::insert(300, "public", "users", make_row(2, "Bob", 25), 1002),
        SourceEvent::commit(400, 101, 1003),
    ];
    let source = Arc::new(MockSourceForIntegration::new(
        events,
        vec![schema],
        HashMap::new(),
    ));
    let source_for_check = source.clone();
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target);

    r.start().unwrap();

    // 源端 confirmed_offset 应推进到最后 LSN
    let offset = source_for_check.confirmed_offset().unwrap();
    assert_eq!(offset.lsn, 400);
}

#[test]
fn integration_reverse_multiple_tables_snapshot() {
    let users_schema = make_schema(1, "users");
    let orders_schema = make_schema(2, "orders");
    let mut snapshot = HashMap::new();
    snapshot.insert(
        "users".to_string(),
        vec![make_row(1, "Alice", 30), make_row(2, "Bob", 25)],
    );
    snapshot.insert(
        "orders".to_string(),
        vec![make_row(100, "Order1", 1), make_row(101, "Order2", 2)],
    );

    let source = Arc::new(MockSourceForIntegration::new(
        vec![],
        vec![users_schema, orders_schema],
        snapshot,
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target.clone());

    r.start().unwrap();

    // 2 表 × 2 行 = 4 个快照写入
    assert_eq!(target.events().len(), 4);
    let stats = r.stats();
    assert_eq!(stats.snapshot_rows, 4);
    assert_eq!(stats.snapshot_tables, 2);
}

#[test]
fn integration_reverse_task_id_persisted() {
    let source = Arc::new(MockSourceForIntegration::new(
        vec![],
        vec![],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("my_unique_task_id", source, target);

    assert_eq!(r.task_id(), "my_unique_task_id");
}

#[test]
fn integration_reverse_stats_lifecycle() {
    let schema = make_schema(1, "users");
    let events = make_events("users");
    let source = Arc::new(MockSourceForIntegration::new(
        events,
        vec![schema],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target);

    // 启动前 stats 全为 0
    let stats_before = r.stats();
    assert_eq!(stats_before.events_processed, 0);
    assert_eq!(stats_before.started_at, 0);

    r.start().unwrap();

    // 启动后 stats 有值
    let stats_after = r.stats();
    assert!(stats_after.events_processed > 0);
    assert!(stats_after.started_at > 0);
    assert!(stats_after.last_event_at > 0);
    assert!(stats_after.current_source_lsn > 0);
}

#[test]
fn integration_reverse_error_propagation() {
    // 源端连接失败场景：MockSourceForIntegration.connect 总是失败
    struct FailingSource;
    impl SourceConnector for FailingSource {
        fn source_type(&self) -> &str {
            "failing"
        }
        fn connect(&self) -> Result<(), SourceError> {
            Err(SourceError::Connection("simulated failure".to_string()))
        }
        fn disconnect(&self) -> Result<(), SourceError> {
            Ok(())
        }
        fn discover_schemas(&self, _: &[String]) -> Result<Vec<TableSchema>, SourceError> {
            Ok(vec![])
        }
        fn extract_snapshot(
            &self,
            _: &str,
            _: usize,
            _: &dyn Fn(&[DecodedRow]) -> Result<(), SourceError>,
        ) -> Result<u64, SourceError> {
            Ok(0)
        }
        fn current_lsn(&self) -> Result<u64, SourceError> {
            Ok(0)
        }
        fn start_cdc_stream(
            &self,
            _: u64,
            _: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
        ) -> Result<(), SourceError> {
            Ok(())
        }
        fn stop_cdc_stream(&self) -> Result<(), SourceError> {
            Ok(())
        }
        fn ack_offset(&self, _: &SourceOffset) -> Result<(), SourceError> {
            Ok(())
        }
        fn confirmed_offset(&self) -> Result<SourceOffset, SourceError> {
            Ok(SourceOffset::default())
        }
        fn health_check(&self) -> Result<(), SourceError> {
            Ok(())
        }
    }

    let source = Arc::new(FailingSource);
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target);

    let result = r.start();
    assert!(result.is_err());
    assert_eq!(r.state(), ReverseReplicatorState::Failed);

    match result {
        Err(ReverseReplicatorError::Source(SourceError::Connection(_))) => {}
        _ => panic!("expected Source Connection error"),
    }
}

#[test]
fn integration_reverse_health_check_passes_after_connect() {
    let schema = make_schema(1, "users");
    let source = Arc::new(MockSourceForIntegration::new(
        vec![],
        vec![schema],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());

    let r = ReverseReplicator::new("task1", source, target);
    r.start().unwrap();
    // 启动完成后，源端应仍处于 connected 状态（MockSourceForIntegration 不主动断开）
    let health = r.health_check();
    assert!(health.is_ok());
}

#[test]
fn integration_reverse_with_retries_config_no_panic() {
    let source = Arc::new(MockSourceForIntegration::new(
        vec![],
        vec![],
        HashMap::new(),
    ));
    let target = Arc::new(CollectingTargetWriter::new());
    let r = ReverseReplicator::new("task1", source, target)
        .with_max_retries(5)
        .with_retry_interval(50);

    r.start().unwrap();
    assert_eq!(r.state(), ReverseReplicatorState::Stopped);
}

// =====================================================================
// Mock SourceConnector for Integration Tests
// =====================================================================

struct MockSourceForIntegration {
    events: Mutex<Vec<SourceEvent>>,
    schemas: Vec<TableSchema>,
    snapshot_rows: Mutex<HashMap<String, Vec<DecodedRow>>>,
    connected: AtomicBool,
    confirmed_offset: Mutex<SourceOffset>,
}

impl MockSourceForIntegration {
    fn new(
        events: Vec<SourceEvent>,
        schemas: Vec<TableSchema>,
        snapshot_rows: HashMap<String, Vec<DecodedRow>>,
    ) -> Self {
        Self {
            events: Mutex::new(events),
            schemas,
            snapshot_rows: Mutex::new(snapshot_rows),
            connected: AtomicBool::new(false),
            confirmed_offset: Mutex::new(SourceOffset::default()),
        }
    }
}

impl SourceConnector for MockSourceForIntegration {
    fn source_type(&self) -> &str {
        "mock_integration"
    }

    fn connect(&self) -> Result<(), SourceError> {
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn disconnect(&self) -> Result<(), SourceError> {
        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn discover_schemas(&self, _tables: &[String]) -> Result<Vec<TableSchema>, SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        Ok(self.schemas.clone())
    }

    fn extract_snapshot(
        &self,
        table: &str,
        _batch_size: usize,
        callback: &dyn Fn(&[DecodedRow]) -> Result<(), SourceError>,
    ) -> Result<u64, SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        let rows = self
            .snapshot_rows
            .lock()
            .unwrap()
            .get(table)
            .cloned()
            .unwrap_or_default();
        let count = rows.len() as u64;
        if !rows.is_empty() {
            callback(&rows)?;
        }
        Ok(count)
    }

    fn current_lsn(&self) -> Result<u64, SourceError> {
        Ok(self.confirmed_offset.lock().unwrap().lsn)
    }

    fn start_cdc_stream(
        &self,
        _start_lsn: u64,
        callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
    ) -> Result<(), SourceError> {
        let events = self.events.lock().unwrap().clone();
        if !events.is_empty() {
            callback(&events)?;
            let max_lsn = events.iter().map(|e| e.lsn).max().unwrap_or(0);
            let mut offset = self.confirmed_offset.lock().unwrap();
            if max_lsn > offset.lsn {
                offset.lsn = max_lsn;
            }
        }
        Ok(())
    }

    fn stop_cdc_stream(&self) -> Result<(), SourceError> {
        Ok(())
    }

    fn ack_offset(&self, offset: &SourceOffset) -> Result<(), SourceError> {
        let mut current = self.confirmed_offset.lock().unwrap();
        if offset.lsn >= current.lsn {
            *current = offset.clone();
        }
        Ok(())
    }

    fn confirmed_offset(&self) -> Result<SourceOffset, SourceError> {
        Ok(self.confirmed_offset.lock().unwrap().clone())
    }

    fn health_check(&self) -> Result<(), SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        Ok(())
    }
}
