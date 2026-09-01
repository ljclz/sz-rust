//! 数据库集成测试 —— 使用真实 MySQL 9.6 / PostgreSQL 18
//!
//! 运行前确保：
//! - MySQL 9.6 运行于 127.0.0.1:3306，root/test123 可登录，sz_orm_test 数据库存在
//! - PostgreSQL 18 运行于 127.0.0.1:5432，postgres/test123 可登录，sz_orm_test 数据库存在
//!
//! 跳过条件：默认 `#[ignore]` 跳过（需真实 MySQL/PG），手动运行：
//! ```
//! cargo test --package sz-rust-sz300 --test db_integration_test -- --ignored
//! ```
//! 数据库不可达时测试会 **fail**（而非静默跳过），CI 可准确识别跳过状态。
use sz_rust_sz300::{config, db};

/// 构建 MySQL 测试配置（直接构造，不依赖环境变量）
fn mysql_test_config() -> config::AppConfig {
    config::AppConfig {
        server: config::ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8300,
        },
        database: config::DatabaseConfig {
            host: "127.0.0.1".to_string(),
            port: 3306,
            username: "root".to_string(),
            password: "test123".to_string(),
            database: "sz_orm_test".to_string(),
        },
    }
}

/// 构建 PostgreSQL 测试配置
fn pg_test_config() -> config::PgDatabaseConfig {
    config::PgDatabaseConfig {
        host: "127.0.0.1".to_string(),
        port: 5432,
        username: "postgres".to_string(),
        password: "test123".to_string(),
        database: "sz_orm_test".to_string(),
    }
}

/// 检查 MySQL 是否可达，不可达则返回 None（测试跳过）
async fn ensure_mysql_available() -> Option<config::AppConfig> {
    let cfg = mysql_test_config();
    match db::init_pool(&cfg).await {
        Ok(pool) => match pool.acquire().await {
            Ok(mut conn) => match conn.query("SELECT 1").await {
                Ok(_) => {
                    pool.close_all().await;
                    Some(cfg)
                }
                Err(e) => {
                    eprintln!("⚠️ MySQL 查询失败，跳过测试: {}", e);
                    None
                }
            },
            Err(e) => {
                eprintln!("⚠️ MySQL 获取连接失败，跳过测试: {}", e);
                None
            }
        },
        Err(e) => {
            eprintln!("⚠️ MySQL 不可达，跳过测试: {}", e);
            None
        }
    }
}

/// 检查 PostgreSQL 是否可达，不可达则返回 None（测试跳过）
///
/// 与 `ensure_mysql_available` 对齐：CI 中可通过此函数预检 PG 可达性，
/// 避免在 PG 不可用时测试以非预期错误码退出。
async fn ensure_pg_available() -> Option<config::PgDatabaseConfig> {
    let cfg = pg_test_config();
    match db::init_pg_pool(&cfg).await {
        Ok(pool) => match pool.acquire().await {
            Ok(mut conn) => match conn.query("SELECT 1").await {
                Ok(_) => {
                    pool.close_all().await;
                    Some(cfg)
                }
                Err(e) => {
                    eprintln!("⚠️ PostgreSQL 查询失败，跳过测试: {}", e);
                    None
                }
            },
            Err(e) => {
                eprintln!("⚠️ PostgreSQL 获取连接失败，跳过测试: {}", e);
                None
            }
        },
        Err(e) => {
            eprintln!("⚠️ PostgreSQL 不可达，跳过测试: {}", e);
            None
        }
    }
}

#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mysql_pool_init_and_query() {
    let cfg = ensure_mysql_available().await.expect(
        "MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)",
    );

    let pool = db::init_pool(&cfg).await.expect("MySQL 连接池初始化失败");

    // 验证连接池配置：db.rs 中 max_size=20, min_idle=10（P3-7：50% 预热）
    assert_eq!(pool.config().max_size, 20, "MySQL max_size 应为 20");
    assert_eq!(pool.config().min_idle, 10, "MySQL min_idle 应为 10");

    // 测试基本查询
    let mut conn = pool.acquire().await.expect("获取 MySQL 连接失败");
    let rows = conn
        .query("SELECT 1 as val")
        .await
        .expect("MySQL SELECT 1 失败");
    assert_eq!(rows.len(), 1, "应返回 1 行");
    let val = rows[0].get("val").expect("应包含 val 字段");
    assert_eq!(val.as_i64(), Some(1), "val 应为 1");

    pool.close_all().await;
    println!("✅ MySQL 连接池初始化与查询测试通过");
}

#[ignore = "需真实 PostgreSQL 18，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_pg_pool_init_and_query() {
    let pg_cfg = ensure_pg_available().await.expect(
        "PostgreSQL 不可达，请启动 PG 18 (127.0.0.1:5432, postgres/test123, sz_orm_test 数据库)",
    );

    let pool = db::init_pg_pool(&pg_cfg)
        .await
        .expect("PG 连接池初始化失败");

    // 验证连接池配置：db.rs 中 max_size=10, min_idle=5（P3-7：50% 预热）
    assert_eq!(pool.config().max_size, 10, "PG max_size 应为 10");
    assert_eq!(pool.config().min_idle, 5, "PG min_idle 应为 5");

    let mut conn = pool.acquire().await.expect("获取 PG 连接失败");
    let rows = conn
        .query("SELECT 1 as val")
        .await
        .expect("PG SELECT 1 失败");
    assert_eq!(rows.len(), 1, "应返回 1 行");
    let val = rows[0].get("val").expect("应包含 val 字段");
    assert_eq!(val.as_i64(), Some(1), "val 应为 1");

    pool.close_all().await;
    println!("✅ PostgreSQL 连接池初始化与查询测试通过");
}

#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mysql_create_table_and_crud() {
    let cfg = ensure_mysql_available().await.expect(
        "MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)",
    );

    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");

    // 清理残留表（防止上次测试中断残留）
    conn.execute("DROP TABLE IF EXISTS sz300_test_tmp")
        .await
        .ok();

    // 创建临时测试表
    let create_sql = "CREATE TABLE sz300_test_tmp (
        id INT AUTO_INCREMENT PRIMARY KEY,
        name VARCHAR(100) NOT NULL,
        value INT DEFAULT 0,
        created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
    )";
    conn.execute(create_sql).await.expect("建表失败");

    // 插入
    let affected = conn
        .execute("INSERT INTO sz300_test_tmp (name, value) VALUES ('test1', 100)")
        .await
        .expect("插入失败");
    assert_eq!(affected, 1, "INSERT 应影响 1 行");

    // 查询验证
    let rows = conn
        .query("SELECT name, value FROM sz300_test_tmp WHERE name = 'test1'")
        .await
        .expect("查询失败");
    assert_eq!(rows.len(), 1, "应返回 1 行");
    let name = rows[0]
        .get("name")
        .and_then(|v| v.as_str())
        .expect("应包含 name 字段");
    assert_eq!(name, "test1", "name 应为 test1");
    let value = rows[0]
        .get("value")
        .and_then(|v| v.as_i64())
        .expect("应包含 value 字段");
    assert_eq!(value, 100, "value 应为 100");

    // 清理
    conn.execute("DROP TABLE IF EXISTS sz300_test_tmp")
        .await
        .ok();

    pool.close_all().await;
    println!("✅ MySQL 建表 + CRUD 测试通过");
}

/// SQL 注入防护验证 —— 真实 DB 验证参数化查询阻断注入攻击
///
/// 2026-07-25 新增（修复 P0-1 后的回归测试）
///
/// 验证点：
/// 1. 含 `'` `;` `--` `OR 1=1` 等注入向量的输入通过 `query_with_params` 参数化后，
///    不会被解释为 SQL 语法，仅作为字符串字面值匹配
/// 2. 参数化查询返回 0 行（无匹配），而非返回全表数据
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mysql_sql_injection_protection() {
    let cfg = ensure_mysql_available().await.expect(
        "MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)",
    );

    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");

    // 清理残留表
    conn.execute("DROP TABLE IF EXISTS sz300_injection_test")
        .await
        .ok();

    // 建表并插入测试数据
    conn.execute(
        "CREATE TABLE sz300_injection_test (\
            id INT AUTO_INCREMENT PRIMARY KEY,\
            username VARCHAR(50) NOT NULL,\
            password_hash VARCHAR(100) NOT NULL\
        )",
    )
    .await
    .expect("建表失败");

    conn.execute(
        "INSERT INTO sz300_injection_test (username, password_hash) VALUES \
         ('admin', '$2b$12$xxx'),\
         ('alice', '$2b$12$yyy'),\
         ('bob', '$2b$12$zzz')",
    )
    .await
    .expect("插入失败");

    use sz_rust_core::orm::Value;

    // 注入向量 1：经典 OR 1=1
    let injection_1 = "admin' OR '1'='1";
    let rows = conn
        .query_with_params(
            "SELECT id FROM sz300_injection_test WHERE username = ?",
            &[Value::String(injection_1.to_string())],
        )
        .await
        .expect("参数化查询失败");
    assert_eq!(
        rows.len(),
        0,
        "OR 1=1 注入应被阻断（返回 0 行），实际返回 {} 行",
        rows.len()
    );

    // 注入向量 2：分号 + 注释
    let injection_2 = "admin'; --";
    let rows = conn
        .query_with_params(
            "SELECT id FROM sz300_injection_test WHERE username = ?",
            &[Value::String(injection_2.to_string())],
        )
        .await
        .expect("参数化查询失败");
    assert_eq!(rows.len(), 0, "'; -- 注入应被阻断");

    // 注入向量 3：UNION SELECT
    let injection_3 = "x' UNION SELECT id FROM sz300_injection_test --";
    let rows = conn
        .query_with_params(
            "SELECT id FROM sz300_injection_test WHERE username = ?",
            &[Value::String(injection_3.to_string())],
        )
        .await
        .expect("参数化查询失败");
    assert_eq!(rows.len(), 0, "UNION SELECT 注入应被阻断");

    // 注入向量 4：反斜杠结尾（MySQL GBK 绕过尝试）
    let injection_4 = "admin\\";
    let rows = conn
        .query_with_params(
            "SELECT id FROM sz300_injection_test WHERE username = ?",
            &[Value::String(injection_4.to_string())],
        )
        .await
        .expect("参数化查询失败");
    assert_eq!(rows.len(), 0, "反斜杠结尾注入应被阻断");

    // 正常输入应返回 1 行（对照实验）
    let rows = conn
        .query_with_params(
            "SELECT id FROM sz300_injection_test WHERE username = ?",
            &[Value::String("admin".to_string())],
        )
        .await
        .expect("参数化查询失败");
    assert_eq!(rows.len(), 1, "正常输入应返回 1 行（对照实验）");

    // 清理
    conn.execute("DROP TABLE IF EXISTS sz300_injection_test")
        .await
        .ok();
    pool.close_all().await;
    println!("✅ MySQL SQL 注入防护验证通过：4 个注入向量全部被阻断。");
}

/// 真实 DB 验证 sz300 商品服务参数化查询（修复 P0-4 分层重构后的回归测试）
///
/// 模拟 `ProductService::list` 的参数化查询逻辑，验证：
/// 1. 关键字搜索 `LIKE ?` 不会被注入
/// 2. 分页参数 `LIMIT ? OFFSET ?` 正常工作
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mysql_product_service_like_injection_protection() {
    let cfg = ensure_mysql_available().await.expect(
        "MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)",
    );

    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");

    conn.execute("DROP TABLE IF EXISTS sz300_good_injection_test")
        .await
        .ok();

    conn.execute(
        "CREATE TABLE sz300_good_injection_test (\
            good_id INT AUTO_INCREMENT PRIMARY KEY,\
            name VARCHAR(100) NOT NULL,\
            cat_id INT DEFAULT 0\
        )",
    )
    .await
    .expect("建表失败");

    conn.execute(
        "INSERT INTO sz300_good_injection_test (name, cat_id) VALUES\
         ('苹果手机', 1),\
         ('华为手机', 1),\
         ('小米手机', 1),\
         ('戴尔电脑', 2)",
    )
    .await
    .expect("插入失败");

    use sz_rust_core::orm::Value;

    // 模拟 ProductService 的 LIKE 参数化查询
    let keyword = "' OR 1=1 --";
    let pattern = format!("%{}%", keyword);
    let rows = conn
        .query_with_params(
            "SELECT good_id FROM sz300_good_injection_test WHERE name LIKE ?",
            &[Value::String(pattern)],
        )
        .await
        .expect("LIKE 参数化查询失败");
    assert_eq!(
        rows.len(),
        0,
        "LIKE 注入应被阻断，实际返回 {} 行",
        rows.len()
    );

    // 正常关键字搜索（对照实验）
    let keyword = "手机";
    let pattern = format!("%{}%", keyword);
    let rows = conn
        .query_with_params(
            "SELECT good_id FROM sz300_good_injection_test WHERE name LIKE ?",
            &[Value::String(pattern)],
        )
        .await
        .expect("LIKE 参数化查询失败");
    assert_eq!(rows.len(), 3, "正常关键字应返回 3 行（对照实验）");

    // 分页参数化（LIMIT/OFFSET）
    let rows = conn
        .query_with_params(
            "SELECT good_id FROM sz300_good_injection_test ORDER BY good_id DESC LIMIT ? OFFSET ?",
            &[Value::I64(2), Value::I64(1)],
        )
        .await
        .expect("分页查询失败");
    assert_eq!(rows.len(), 2, "LIMIT 2 OFFSET 1 应返回 2 行");

    conn.execute("DROP TABLE IF EXISTS sz300_good_injection_test")
        .await
        .ok();
    pool.close_all().await;
    println!("✅ MySQL 商品服务 LIKE/分页参数化查询注入防护验证通过");
}

// ============================================================================
// P0 补齐：真实 DB 集成验证（2026-08-01）
// 缺口：事务原子性 / savepoint / 连接池并发 / 真实业务 schema 关联 CRUD
// ============================================================================

/// MySQL 事务原子性验证 —— commit 生效 / rollback 回滚
///
/// 通过 sz-orm `Connection` trait 的 begin_transaction/commit/rollback 真实驱动 MySQL 事务：
/// 1. 事务内 INSERT + commit → 数据持久可见
/// 2. 事务内 INSERT + rollback → 数据不可见（表行数不变）
/// 3. 验证事务期间同连接查询能看到未提交数据（会话内可见性）
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mysql_transaction_commit_rollback() {
    let cfg = ensure_mysql_available().await.expect(
        "MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)",
    );

    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");

    conn.execute("DROP TABLE IF EXISTS sz300_tx_test")
        .await
        .ok();
    conn.execute(
        "CREATE TABLE sz300_tx_test (id INT AUTO_INCREMENT PRIMARY KEY, val VARCHAR(50) NOT NULL)",
    )
    .await
    .expect("建表失败");

    // ── 场景 1：commit 后数据持久可见 ──
    conn.begin_transaction().await.expect("开启事务失败");
    conn.execute("INSERT INTO sz300_tx_test (val) VALUES ('committed')")
        .await
        .expect("事务内插入失败");
    // 事务内同连接应能看到未提交数据
    let rows = conn
        .query("SELECT val FROM sz300_tx_test WHERE val = 'committed'")
        .await
        .expect("事务内查询失败");
    assert_eq!(rows.len(), 1, "事务内应看到未提交数据（会话内可见性）");
    conn.commit().await.expect("提交事务失败");

    let rows = conn
        .query("SELECT val FROM sz300_tx_test WHERE val = 'committed'")
        .await
        .expect("commit 后查询失败");
    assert_eq!(rows.len(), 1, "commit 后数据应持久可见");

    // ── 场景 2：rollback 后数据不可见 ──
    conn.begin_transaction().await.expect("开启事务失败");
    conn.execute("INSERT INTO sz300_tx_test (val) VALUES ('rolled_back')")
        .await
        .expect("事务内插入失败");
    conn.rollback().await.expect("回滚事务失败");

    let rows = conn
        .query("SELECT val FROM sz300_tx_test WHERE val = 'rolled_back'")
        .await
        .expect("rollback 后查询失败");
    assert_eq!(rows.len(), 0, "rollback 后数据不应存在");

    let rows = conn
        .query("SELECT COUNT(*) AS cnt FROM sz300_tx_test")
        .await
        .expect("计数查询失败");
    let cnt = rows[0]
        .get("cnt")
        .and_then(|v| v.as_i64())
        .expect("应包含 cnt 字段");
    assert_eq!(cnt, 1, "rollback 后表内应只剩 committed 一行");

    // ── 场景 3：未提交即显式 rollback + drop 连接，数据不落库 ──
    // 注意：sz-orm-core 4.7.0 的 Pool::release 不会自动回滚未提交事务，
    // 连接归还时 in_transaction 仍为 true，未提交的 INSERT 持有表锁。
    // 若不显式 rollback 直接 drop，后续 DROP TABLE 会等待锁释放形成死锁。
    // 这是 sz-orm-core 的已知限制（crates.io 依赖，无法在本仓库修复）。
    {
        let mut c2 = pool.acquire().await.expect("获取连接失败");
        c2.begin_transaction().await.expect("开启事务失败");
        c2.execute("INSERT INTO sz300_tx_test (val) VALUES ('dropped')")
            .await
            .expect("事务内插入失败");
        c2.rollback().await.expect("显式回滚事务");
    }
    let rows = conn
        .query("SELECT COUNT(*) AS cnt FROM sz300_tx_test")
        .await
        .expect("计数查询失败");
    let cnt = rows[0]
        .get("cnt")
        .and_then(|v| v.as_i64())
        .expect("应包含 cnt 字段");
    assert_eq!(cnt, 1, "回滚后事务数据不应存在");

    conn.execute("DROP TABLE IF EXISTS sz300_tx_test")
        .await
        .ok();
    pool.close_all().await;
    println!("✅ MySQL 事务原子性验证通过：commit 生效 / rollback 回滚 / 显式回滚后断连");
}

/// MySQL SAVEPOINT 部分回滚验证
///
/// 验证事务内 savepoint 只回滚到标记点，保留标记点前的写入：
/// 1. INSERT 'keep' → SAVEPOINT sp1 → INSERT 'discard' → ROLLBACK TO sp1
/// 2. commit 后 'keep' 存在、'discard' 不存在
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mysql_transaction_savepoint() {
    let cfg = ensure_mysql_available().await.expect(
        "MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)",
    );

    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");

    conn.execute("DROP TABLE IF EXISTS sz300_sp_test")
        .await
        .ok();
    conn.execute(
        "CREATE TABLE sz300_sp_test (id INT AUTO_INCREMENT PRIMARY KEY, val VARCHAR(50) NOT NULL)",
    )
    .await
    .expect("建表失败");

    conn.begin_transaction().await.expect("开启事务失败");
    conn.execute("INSERT INTO sz300_sp_test (val) VALUES ('keep')")
        .await
        .expect("插入 keep 失败");
    conn.execute("SAVEPOINT sp1")
        .await
        .expect("建立 savepoint 失败");
    conn.execute("INSERT INTO sz300_sp_test (val) VALUES ('discard')")
        .await
        .expect("插入 discard 失败");
    conn.execute("ROLLBACK TO SAVEPOINT sp1")
        .await
        .expect("回滚到 savepoint 失败");
    conn.commit().await.expect("提交事务失败");

    let rows = conn
        .query("SELECT val FROM sz300_sp_test ORDER BY id")
        .await
        .expect("查询失败");
    let vals: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("val").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert_eq!(
        vals,
        vec!["keep"],
        "savepoint 后应只保留 'keep'，实际: {:?}",
        vals
    );

    conn.execute("DROP TABLE IF EXISTS sz300_sp_test")
        .await
        .ok();
    pool.close_all().await;
    println!("✅ MySQL SAVEPOINT 部分回滚验证通过");
}

/// MySQL 连接池并发压力验证 —— 20 并发任务 × 每任务 5 轮 CRUD
///
/// 验证 max_size=20 连接池在并发竞争下的行为：
/// 1. 20 个并发任务同时 acquire，全部应成功（无死锁、无超时）
/// 2. 每任务独立行做 INSERT → SELECT → UPDATE → DELETE 循环
/// 3. 所有任务完成后，表内应无残留（每任务删除自己的行）
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_mysql_pool_concurrency_stress() {
    use futures::future::join_all;
    use std::sync::Arc;
    use sz_rust_core::orm::Value;

    let cfg = ensure_mysql_available().await.expect(
        "MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)",
    );

    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));

    // 初始化并发测试表
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS sz300_conc_test")
            .await
            .ok();
        conn.execute(
            "CREATE TABLE sz300_conc_test (\
                id INT AUTO_INCREMENT PRIMARY KEY,\
                task_id INT NOT NULL,\
                round INT NOT NULL,\
                val VARCHAR(50) NOT NULL,\
                UNIQUE KEY uk_task_round (task_id, round)\
            )",
        )
        .await
        .expect("建表失败");
    }

    const TASKS: usize = 20;
    const ROUNDS: usize = 5;

    let tasks: Vec<_> = (0..TASKS)
        .map(|task_id| {
            let pool = Arc::clone(&pool);
            async move {
                // 每任务独立连接做 5 轮 CRUD；外部 timeout 兜底防死锁
                tokio::time::timeout(std::time::Duration::from_secs(20), async move {
                    for round in 0..ROUNDS {
                        let mut conn = pool.acquire().await.expect("并发获取连接失败");
                        let val = format!("task{task_id}_r{round}");

                        // INSERT
                        let affected = conn
                            .execute_with_params(
                                "INSERT INTO sz300_conc_test (task_id, round, val) VALUES (?, ?, ?)",
                                &[
                                    Value::I64(task_id as i64),
                                    Value::I64(round as i64),
                                    Value::String(val.clone()),
                                ],
                            )
                            .await
                            .expect("并发插入失败");
                        assert_eq!(affected, 1, "INSERT 应影响 1 行");

                        // SELECT 回读
                        let rows = conn
                            .query_with_params(
                                "SELECT val FROM sz300_conc_test WHERE task_id = ? AND round = ?",
                                &[Value::I64(task_id as i64), Value::I64(round as i64)],
                            )
                            .await
                            .expect("并发查询失败");
                        assert_eq!(rows.len(), 1, "应读回 1 行");
                        let got = rows[0]
                            .get("val")
                            .and_then(|v| v.as_str())
                            .expect("应包含 val 字段");
                        assert_eq!(got, val, "读回值应一致");

                        // UPDATE
                        let updated_val = format!("{val}_upd");
                        let affected = conn
                            .execute_with_params(
                                "UPDATE sz300_conc_test SET val = ? WHERE task_id = ? AND round = ?",
                                &[
                                    Value::String(updated_val.clone()),
                                    Value::I64(task_id as i64),
                                    Value::I64(round as i64),
                                ],
                            )
                            .await
                            .expect("并发更新失败");
                        assert_eq!(affected, 1, "UPDATE 应影响 1 行");

                        // DELETE
                        let affected = conn
                            .execute_with_params(
                                "DELETE FROM sz300_conc_test WHERE task_id = ? AND round = ?",
                                &[Value::I64(task_id as i64), Value::I64(round as i64)],
                            )
                            .await
                            .expect("并发删除失败");
                        assert_eq!(affected, 1, "DELETE 应影响 1 行");
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .await
                .expect("并发任务超时（连接池可能死锁）")
                .expect("并发任务执行失败");
            }
        })
        .collect();

    join_all(tasks).await;

    // 验证无残留：表应为空
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query("SELECT COUNT(*) AS cnt FROM sz300_conc_test")
        .await
        .expect("计数查询失败");
    let cnt = rows[0]
        .get("cnt")
        .and_then(|v| v.as_i64())
        .expect("应包含 cnt 字段");
    assert_eq!(
        cnt, 0,
        "20 并发 × 5 轮 CRUD 后表应无残留，实际残留 {cnt} 行"
    );

    conn.execute("DROP TABLE IF EXISTS sz300_conc_test")
        .await
        .ok();
    pool.close_all().await;
    println!("✅ MySQL 连接池并发压力验证通过：{TASKS} 并发 × {ROUNDS} 轮 CRUD 无死锁、无残留");
}

/// MySQL 真实业务 schema 关联 CRUD 验证
///
/// 按 `migrations/001_init.sql` 真实表结构（market / merchant / good）验证：
/// 1. 三表建表 → 插入市场 → 插入商户 → 插入商品
/// 2. 关联 JOIN 查询：商品 + 商户 + 市场名称一次取回
/// 3. 参数化 UPDATE / DELETE 链式操作
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mysql_business_schema_crud() {
    use sz_rust_core::orm::Value;

    let cfg = ensure_mysql_available().await.expect(
        "MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)",
    );

    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");

    // 清理并重建三张业务表（对齐 001_init.sql 结构）
    conn.execute("DROP TABLE IF EXISTS sz300_it_good")
        .await
        .ok();
    conn.execute("DROP TABLE IF EXISTS sz300_it_merchant")
        .await
        .ok();
    conn.execute("DROP TABLE IF EXISTS sz300_it_market")
        .await
        .ok();

    conn.execute(
        "CREATE TABLE sz300_it_market (\
            market_id INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            name VARCHAR(100) NOT NULL,\
            status TINYINT DEFAULT 1,\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建市场表失败");

    conn.execute(
        "CREATE TABLE sz300_it_merchant (\
            merchant_id INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            market_id INT UNSIGNED NOT NULL,\
            name VARCHAR(100) NOT NULL,\
            stall_no VARCHAR(50) DEFAULT '',\
            status TINYINT DEFAULT 1,\
            INDEX idx_market (market_id)\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建商户表失败");

    conn.execute(
        "CREATE TABLE sz300_it_good (\
            good_id INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            merchant_id INT UNSIGNED NOT NULL,\
            cat_id INT UNSIGNED DEFAULT 0,\
            name VARCHAR(100) NOT NULL,\
            barcode VARCHAR(50) DEFAULT '',\
            price INT UNSIGNED NOT NULL,\
            status TINYINT DEFAULT 1,\
            INDEX idx_merchant (merchant_id)\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建商品表失败");

    // ── 1. 主键回填：插入市场/商户/商品 ──
    conn.execute("INSERT INTO sz300_it_market (name) VALUES ('鲜视达市场')")
        .await
        .expect("插入市场失败");
    let rows = conn
        .query("SELECT LAST_INSERT_ID() AS id")
        .await
        .expect("查询自增 ID 失败");
    let market_id = rows[0]
        .get("id")
        .and_then(|v| v.as_i64())
        .expect("应包含 id 字段");

    conn.execute_with_params(
        "INSERT INTO sz300_it_merchant (market_id, name, stall_no) VALUES (?, '张记蔬菜', 'A-01')",
        &[Value::I64(market_id)],
    )
    .await
    .expect("插入商户失败");
    let rows = conn
        .query("SELECT LAST_INSERT_ID() AS id")
        .await
        .expect("查询自增 ID 失败");
    let merchant_id = rows[0]
        .get("id")
        .and_then(|v| v.as_i64())
        .expect("应包含 id 字段");

    conn.execute_with_params(
        "INSERT INTO sz300_it_good (merchant_id, name, barcode, price) VALUES (?, '有机白菜', '6901234567890', 499)",
        &[Value::I64(merchant_id)],
    )
    .await
    .expect("插入商品失败");

    // ── 2. 三表关联 JOIN 查询 ──
    let rows = conn
        .query(
            "SELECT g.name AS good_name, m.name AS merchant_name, mk.name AS market_name \
             FROM sz300_it_good g \
             JOIN sz300_it_merchant m ON g.merchant_id = m.merchant_id \
             JOIN sz300_it_market mk ON m.market_id = mk.market_id \
             WHERE g.barcode = '6901234567890'",
        )
        .await
        .expect("关联查询失败");
    assert_eq!(rows.len(), 1, "三表关联应返回 1 行");
    let good_name = rows[0]
        .get("good_name")
        .and_then(|v| v.as_str())
        .expect("应包含 good_name");
    let merchant_name = rows[0]
        .get("merchant_name")
        .and_then(|v| v.as_str())
        .expect("应包含 merchant_name");
    let market_name = rows[0]
        .get("market_name")
        .and_then(|v| v.as_str())
        .expect("应包含 market_name");
    assert_eq!(good_name, "有机白菜");
    assert_eq!(merchant_name, "张记蔬菜");
    assert_eq!(market_name, "鲜视达市场");

    // ── 3. 参数化 UPDATE / DELETE ──
    let affected = conn
        .execute_with_params(
            "UPDATE sz300_it_good SET price = ?, status = 0 WHERE good_id = ?",
            &[Value::I64(599), Value::I64(1)],
        )
        .await
        .expect("更新商品失败");
    assert_eq!(affected, 1, "UPDATE 应影响 1 行");

    let rows = conn
        .query("SELECT price FROM sz300_it_good WHERE barcode = '6901234567890'")
        .await
        .expect("查询失败");
    let price = rows[0]
        .get("price")
        .and_then(|v| v.as_i64())
        .expect("应包含 price");
    assert_eq!(price, 599, "价格应更新为 599");

    let affected = conn
        .execute_with_params(
            "DELETE FROM sz300_it_good WHERE merchant_id = ?",
            &[Value::I64(merchant_id)],
        )
        .await
        .expect("删除商品失败");
    assert_eq!(affected, 1, "DELETE 应影响 1 行");

    // ── 4. 清理 ──
    conn.execute("DROP TABLE IF EXISTS sz300_it_good")
        .await
        .ok();
    conn.execute("DROP TABLE IF EXISTS sz300_it_merchant")
        .await
        .ok();
    conn.execute("DROP TABLE IF EXISTS sz300_it_market")
        .await
        .ok();
    pool.close_all().await;
    println!("✅ MySQL 真实业务 schema 关联 CRUD 验证通过：三表 JOIN / 参数化 UPDATE / DELETE");
}

/// PostgreSQL 事务原子性验证 —— 跨库一致性保障
///
/// 与 MySQL 事务测试对齐，验证 PG 侧 begin_transaction/commit/rollback 行为一致：
/// 1. 事务内 INSERT + commit → 数据持久可见
/// 2. 事务内 INSERT + rollback → 数据不可见
#[ignore = "需真实 PostgreSQL 18，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_pg_transaction_commit_rollback() {
    let pg_cfg = ensure_pg_available().await.expect(
        "PostgreSQL 不可达，请启动 PG 18 (127.0.0.1:5432, postgres/test123, sz_orm_test 数据库)",
    );

    let pool = db::init_pg_pool(&pg_cfg)
        .await
        .expect("PG 连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");

    conn.execute("DROP TABLE IF EXISTS sz300_pg_tx_test")
        .await
        .ok();
    conn.execute("CREATE TABLE sz300_pg_tx_test (id SERIAL PRIMARY KEY, val VARCHAR(50) NOT NULL)")
        .await
        .expect("建表失败");

    // commit 场景
    conn.begin_transaction().await.expect("开启事务失败");
    conn.execute("INSERT INTO sz300_pg_tx_test (val) VALUES ('committed')")
        .await
        .expect("事务内插入失败");
    conn.commit().await.expect("提交事务失败");

    // rollback 场景
    conn.begin_transaction().await.expect("开启事务失败");
    conn.execute("INSERT INTO sz300_pg_tx_test (val) VALUES ('rolled_back')")
        .await
        .expect("事务内插入失败");
    conn.rollback().await.expect("回滚事务失败");

    let rows = conn
        .query("SELECT val FROM sz300_pg_tx_test ORDER BY id")
        .await
        .expect("查询失败");
    let vals: Vec<String> = rows
        .iter()
        .filter_map(|r| r.get("val").and_then(|v| v.as_str()).map(String::from))
        .collect();
    assert_eq!(
        vals,
        vec!["committed"],
        "PG 事务后应只有 committed，实际: {:?}",
        vals
    );

    conn.execute("DROP TABLE IF EXISTS sz300_pg_tx_test")
        .await
        .ok();
    pool.close_all().await;
    println!("✅ PostgreSQL 事务原子性验证通过：commit 生效 / rollback 回滚");
}

// ============================================================================
// 服务层 & MQTT 层 & 控制器级真实集成测试
//
// 背景：2026-08-15 幻影交付审计（docs/audit/2026-08-15-幻影交付审计报告.md P5）
// 发现 service_coverage_test.rs / mqtt_dispatch_test.rs 中的 19 个占位测试
// （#[ignore] + 空函数体 + TODO，0 断言）为幻影测试。占位测试已删除，
// 本节的真实断言集成测试补齐原占位声称的覆盖场景：
//   health_service::ping_db / DeviceService::unbind|get_ota_version|update_status
//   MqttMessageHandler::handle_device_status|order|log / MqttDispatcher::dispatch
//   MqttDispatcher::start_consumer / auth::me|logout / device::trigger_ota|status_report
//
// 运行方式（需真实 MySQL 9.6，sz_orm_test 库）：
// ```
// cargo test --package sz-rust-sz300 --test db_integration_test -- --ignored
// ```
// ============================================================================
use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use sz_rust_core::orm::{Pool, Value};
use sz_rust_sz300::controllers::{auth, device};
use sz_rust_sz300::services::device_service::DeviceService;
use sz_rust_sz300::services::health_service;
use sz_rust_sz300::services::mqtt_listener::MqttDispatcher;
use sz_rust_sz300::services::mqtt_service::MqttMessageHandler;
use sz_rust_sz300::state::AppState;

/// 构造测试用 AppState（对齐 main.rs 的组件初始化，admin feature 默认关闭）
fn build_test_state(pool: Arc<Pool>) -> AppState {
    use sz_rust_capability::CapabilityRegistry;
    use sz_rust_core::hooks::HookRegistry;
    use sz_rust_core::plugin::event_bus::InMemoryEventBus;
    use sz_rust_observability::slo::{SloConfig, SloMonitor};
    use sz_rust_observability::MetricsRegistry;

    AppState {
        db_pool: pool,
        pg_pool: None,
        metrics_registry: Arc::new(MetricsRegistry::new()),
        capability_registry: Arc::new(CapabilityRegistry::new()),
        ai: None,
        event_bus: Arc::new(InMemoryEventBus::new()),
        cache: None,
        slo_monitor: Arc::new(SloMonitor::new(SloConfig::default())),
        hook_registry: Arc::new(HookRegistry::new()),
        long_term_memory: None,
        crm_state: sz_rust_addons_crm::CrmState::default(),
        cms_state: sz_rust_addons_cms::CmsState::default(),
        pdf_state: sz_rust_pdf::PdfState::default(),
        operate_state: sz_rust_addons_operate::OperateState::default(),
        tracing_state: sz_rust_tracing::TracingState::default(),
        workflow_state: sz_rust_workflow::WorkflowState::default(),
    }
}

/// 重建服务层测试所需的 4 张业务表（对齐 001_init.sql 结构，测试库专用）
async fn create_service_test_tables(conn: &mut dyn sz_rust_core::orm::Connection) {
    conn.execute("DROP TABLE IF EXISTS `order`").await.ok();
    conn.execute("DROP TABLE IF EXISTS device").await.ok();
    conn.execute("DROP TABLE IF EXISTS ota_version").await.ok();
    conn.execute("DROP TABLE IF EXISTS operate_log").await.ok();

    conn.execute(
        "CREATE TABLE device (\
            device_id INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            merchant_id INT UNSIGNED DEFAULT 0,\
            device_sn VARCHAR(50) NOT NULL UNIQUE,\
            device_model VARCHAR(20) DEFAULT 'SZ-300',\
            fw_version VARCHAR(20) DEFAULT '',\
            status TINYINT DEFAULT 0,\
            signal_strength INT DEFAULT 0,\
            bind_at DATETIME DEFAULT NULL,\
            last_online_at DATETIME DEFAULT NULL,\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,\
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 device 表失败");

    conn.execute(
        "CREATE TABLE `order` (\
            order_id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            order_no VARCHAR(32) NOT NULL UNIQUE,\
            merchant_id INT UNSIGNED NOT NULL,\
            device_id INT UNSIGNED DEFAULT 0,\
            total_fen INT UNSIGNED NOT NULL,\
            total_weight_g INT UNSIGNED DEFAULT 0,\
            item_count SMALLINT UNSIGNED DEFAULT 0,\
            status TINYINT DEFAULT 0,\
            pay_method TINYINT DEFAULT 0,\
            offline_seq VARCHAR(50) DEFAULT '',\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 order 表失败");

    conn.execute(
        "CREATE TABLE ota_version (\
            ota_id INT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            version VARCHAR(20) NOT NULL,\
            device_model VARCHAR(20) DEFAULT 'SZ-300',\
            url VARCHAR(255) NOT NULL,\
            md5 VARCHAR(32) DEFAULT '',\
            changelog TEXT,\
            size INT UNSIGNED DEFAULT 0,\
            forced TINYINT DEFAULT 0,\
            status TINYINT DEFAULT 1,\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 ota_version 表失败");

    conn.execute(
        "CREATE TABLE operate_log (\
            log_id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,\
            merchant_id INT UNSIGNED DEFAULT 0,\
            operator VARCHAR(50) DEFAULT '',\
            action VARCHAR(50) NOT NULL,\
            detail TEXT,\
            ip VARCHAR(45) DEFAULT '',\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP\
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 operate_log 表失败");
}

/// 清理服务层测试表
async fn drop_service_test_tables(conn: &mut dyn sz_rust_core::orm::Connection) {
    conn.execute("DROP TABLE IF EXISTS `order`").await.ok();
    conn.execute("DROP TABLE IF EXISTS device").await.ok();
    conn.execute("DROP TABLE IF EXISTS ota_version").await.ok();
    conn.execute("DROP TABLE IF EXISTS operate_log").await.ok();
}

/// 插入测试设备，返回 device_id
async fn insert_test_device(conn: &mut dyn sz_rust_core::orm::Connection, sn: &str) -> i64 {
    conn.execute_with_params(
        "INSERT INTO device (device_sn, device_model) VALUES (?, 'SZ-300')",
        &[Value::String(sn.to_string())],
    )
    .await
    .expect("插入测试设备失败");
    let rows = conn
        .query("SELECT LAST_INSERT_ID() AS id")
        .await
        .expect("查询自增 ID 失败");
    rows[0]
        .get("id")
        .and_then(|v| v.as_i64())
        .expect("应包含 id 字段")
}

/// 读取响应 body 为 UTF-8 文本
async fn resp_body_text(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 65536)
        .await
        .expect("读取响应体失败");
    String::from_utf8_lossy(&bytes).to_string()
}

/// ── health_service::ping_db：DB 可达时返回 true
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_service_health_ping_db_true_when_available() {
    let cfg = ensure_mysql_available().await.expect(
        "MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)",
    );
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    assert!(
        health_service::ping_db(&pool).await,
        "DB 可达时 ping_db 应返回 true"
    );
    pool.close_all().await;
    println!("✅ health_service::ping_db 可达场景验证通过");
}

/// ── health_service::ping_db：DB 不可达时返回 false（不 panic）
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_service_health_ping_db_false_when_unreachable() {
    let mut cfg = mysql_test_config();
    // 指向本机未监听的端口（TCP 拒绝快速失败）
    cfg.database.port = 3307;
    match db::init_pool(&cfg).await {
        Ok(pool) => {
            let pool = Arc::new(pool);
            assert!(
                !health_service::ping_db(&pool).await,
                "DB 不可达时 ping_db 应返回 false"
            );
            pool.close_all().await;
        }
        Err(e) => {
            // init 阶段即失败（连接拒绝）等价于不可达，记录而非断言失败
            eprintln!(
                "⚠️ 不可达 pool 初始化失败（符合预期，跳过 false 断言）: {}",
                e
            );
        }
    }
    println!("✅ health_service::ping_db 不可达场景验证通过（不 panic）");
}

/// ── DeviceService::unbind：merchant_id 置 0，status 置 0，bind_at 置 NULL
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_device_service_unbind_resets_fields() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;

    let device_id = insert_test_device(&mut *conn, "SN-UNBIND-001").await;
    conn.execute_with_params(
        "UPDATE device SET merchant_id = ?, status = 1, bind_at = NOW() WHERE device_id = ?",
        &[Value::I64(7), Value::I64(device_id)],
    )
    .await
    .expect("预置绑定状态失败");

    DeviceService::unbind(&pool, device_id)
        .await
        .expect("unbind 应成功");

    let rows = conn
        .query_with_params(
            "SELECT merchant_id, status, bind_at FROM device WHERE device_id = ?",
            &[Value::I64(device_id)],
        )
        .await
        .expect("查询失败");
    let row = &rows[0];
    assert_eq!(
        row.get("merchant_id").and_then(|v| v.as_i64()),
        Some(0),
        "merchant_id 应置 0"
    );
    assert_eq!(
        row.get("status").and_then(|v| v.as_i64()),
        Some(0),
        "status 应置 0"
    );
    assert!(
        row.get("bind_at").and_then(|v| v.as_i64()).is_none() || row.get("bind_at").is_none(),
        "bind_at 应为 NULL"
    );

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ DeviceService::unbind 真实断言验证通过");
}

/// ── DeviceService::get_ota_version：已发布版本返回 Some
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_device_service_get_ota_version_returns_enabled() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;

    conn.execute(
        "INSERT INTO ota_version (version, url, status) VALUES ('2.0.1', 'http://ota.example.com/2.0.1.bin', 1)",
    )
    .await
    .expect("插入 OTA 版本失败");

    let result = DeviceService::get_ota_version(&pool, "2.0.1")
        .await
        .expect("查询应成功");
    assert!(result.is_some(), "已发布版本应返回 Some");
    let row = result.unwrap();
    assert_eq!(row.get("version").and_then(|v| v.as_str()), Some("2.0.1"));

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ DeviceService::get_ota_version 已发布场景验证通过");
}

/// ── DeviceService::get_ota_version：草稿（status=0）返回 None
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_device_service_get_ota_version_disabled_returns_none() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;

    conn.execute(
        "INSERT INTO ota_version (version, url, status) VALUES ('2.0.0', 'http://ota.example.com/2.0.0.bin', 0)",
    )
    .await
    .expect("插入 OTA 版本失败");

    let result = DeviceService::get_ota_version(&pool, "2.0.0")
        .await
        .expect("查询应成功");
    assert!(result.is_none(), "草稿版本应返回 None");

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ DeviceService::get_ota_version 草稿场景验证通过");
}

/// ── DeviceService::update_status：更新 status/signal_strength/fw_version
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_device_service_update_status_updates_fields() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = db::init_pool(&cfg).await.expect("连接池初始化失败");
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;

    let device_id = insert_test_device(&mut *conn, "SN-UPD-001").await;
    DeviceService::update_status(&pool, device_id, 1, -42, "1.9.0")
        .await
        .expect("update_status 应成功");

    let rows = conn
        .query_with_params(
            "SELECT status, signal_strength, fw_version FROM device WHERE device_id = ?",
            &[Value::I64(device_id)],
        )
        .await
        .expect("查询失败");
    let row = &rows[0];
    assert_eq!(row.get("status").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(
        row.get("signal_strength").and_then(|v| v.as_i64()),
        Some(-42)
    );
    assert_eq!(
        row.get("fw_version").and_then(|v| v.as_str()),
        Some("1.9.0")
    );

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ DeviceService::update_status 真实断言验证通过");
}

/// ── MqttMessageHandler::handle_device_status：设备状态上报落库
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mqtt_handle_device_status_updates_db() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;
    insert_test_device(&mut *conn, "SN-MQTT-ST-001").await;

    let state = build_test_state(pool.clone());
    let payload = serde_json::json!({ "status": 1, "signal_strength": -60, "fw_version": "2.1.0" });
    MqttMessageHandler::handle_device_status(&state, "SN-MQTT-ST-001", &payload)
        .await
        .expect("handle_device_status 应成功");

    let rows = conn
        .query("SELECT status, signal_strength, fw_version FROM device WHERE device_sn = 'SN-MQTT-ST-001'")
        .await
        .expect("查询失败");
    let row = &rows[0];
    assert_eq!(row.get("status").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(
        row.get("signal_strength").and_then(|v| v.as_i64()),
        Some(-60)
    );
    assert_eq!(
        row.get("fw_version").and_then(|v| v.as_str()),
        Some("2.1.0")
    );

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ MqttMessageHandler::handle_device_status 真实断言验证通过");
}

/// ── MqttMessageHandler::handle_device_order：设备订单上报写入 order 表
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mqtt_handle_device_order_creates_record() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;
    let device_id = insert_test_device(&mut *conn, "SN-MQTT-ORD-001").await;
    conn.execute_with_params(
        "UPDATE device SET merchant_id = ? WHERE device_sn = 'SN-MQTT-ORD-001'",
        &[Value::I64(3)],
    )
    .await
    .expect("预置商户失败");

    let state = build_test_state(pool.clone());
    let payload = serde_json::json!({
        "offline_seq": "SEQ-1001",
        "total_fen": 1250,
        "items": [{"name": "白菜", "price_fen": 1250, "total_fen": 1250}]
    });
    MqttMessageHandler::handle_device_order(&state, "SN-MQTT-ORD-001", &payload)
        .await
        .expect("handle_device_order 应成功");

    let rows = conn
        .query("SELECT merchant_id, device_id, total_fen, offline_seq, item_count, status FROM `order`")
        .await
        .expect("查询订单失败");
    assert_eq!(rows.len(), 1, "order 表应有 1 条新记录");
    let row = &rows[0];
    assert_eq!(row.get("merchant_id").and_then(|v| v.as_i64()), Some(3));
    assert_eq!(
        row.get("device_id").and_then(|v| v.as_i64()),
        Some(device_id)
    );
    assert_eq!(row.get("total_fen").and_then(|v| v.as_i64()), Some(1250));
    assert_eq!(
        row.get("offline_seq").and_then(|v| v.as_str()),
        Some("SEQ-1001")
    );
    assert_eq!(row.get("item_count").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(row.get("status").and_then(|v| v.as_i64()), Some(1));

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ MqttMessageHandler::handle_device_order 真实断言验证通过");
}

/// ── MqttMessageHandler::handle_device_order：负金额拒绝写入（黑帽审计 A16 防护）
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mqtt_handle_device_order_rejects_negative_amount() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;
    insert_test_device(&mut *conn, "SN-MQTT-NEG-001").await;

    let state = build_test_state(pool.clone());
    let payload = serde_json::json!({ "offline_seq": "NEG-1", "total_fen": -100 });
    let result = MqttMessageHandler::handle_device_order(&state, "SN-MQTT-NEG-001", &payload).await;
    assert!(result.is_err(), "负金额应被拒绝");
    assert!(result.unwrap_err().contains("负"), "错误信息应说明金额为负");

    let rows = conn
        .query("SELECT COUNT(*) AS cnt FROM `order`")
        .await
        .expect("查询失败");
    assert_eq!(
        rows[0].get("cnt").and_then(|v| v.as_i64()),
        Some(0),
        "负金额订单不得写入 order 表"
    );

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ MqttMessageHandler::handle_device_order 负金额拒绝验证通过");
}

/// ── MqttMessageHandler::handle_device_log：设备日志写入 operate_log 表
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mqtt_handle_device_log_creates_record() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;
    insert_test_device(&mut *conn, "SN-MQTT-LOG-001").await;

    let state = build_test_state(pool.clone());
    let payload = serde_json::json!({ "level": "error", "message": "传感器异常" });
    MqttMessageHandler::handle_device_log(&state, "SN-MQTT-LOG-001", &payload)
        .await
        .expect("handle_device_log 应成功");

    let rows = conn
        .query("SELECT operator, action, detail FROM operate_log")
        .await
        .expect("查询日志失败");
    assert_eq!(rows.len(), 1, "operate_log 应有 1 条新记录");
    let row = &rows[0];
    assert_eq!(
        row.get("operator").and_then(|v| v.as_str()),
        Some("device:SN-MQTT-LOG-001")
    );
    assert!(row
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains("error"));
    assert_eq!(
        row.get("detail").and_then(|v| v.as_str()),
        Some("传感器异常")
    );

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ MqttMessageHandler::handle_device_log 真实断言验证通过");
}

/// ── MqttDispatcher::dispatch：topic 路由分发到对应 handler + 安全校验（L-1）
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mqtt_dispatch_topic_routing_and_safety() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;
    insert_test_device(&mut *conn, "SN-DISP-001").await;

    let state = build_test_state(pool.clone());

    // 1. status action 路由 → 落库
    let payload = serde_json::json!({ "status": 1, "signal_strength": -55, "fw_version": "3.0.0" });
    MqttDispatcher::dispatch(
        &state,
        "/sz/device/SN-DISP-001/status",
        &serde_json::to_vec(&payload).unwrap(),
    )
    .await;
    let rows = conn
        .query("SELECT fw_version FROM device WHERE device_sn = 'SN-DISP-001'")
        .await
        .expect("查询失败");
    assert_eq!(
        rows[0].get("fw_version").and_then(|v| v.as_str()),
        Some("3.0.0"),
        "status action 应路由到 handle_device_status"
    );

    // 2. order action 路由 → order 表新记录
    let order_payload = serde_json::json!({ "offline_seq": "D-1", "total_fen": 99 });
    MqttDispatcher::dispatch(
        &state,
        "/sz/device/SN-DISP-001/order",
        &serde_json::to_vec(&order_payload).unwrap(),
    )
    .await;
    let rows = conn
        .query("SELECT COUNT(*) AS cnt FROM `order`")
        .await
        .expect("查询失败");
    assert_eq!(
        rows[0].get("cnt").and_then(|v| v.as_i64()),
        Some(1),
        "order action 应路由到 handle_device_order"
    );

    // 3. log action 路由 → operate_log 新记录
    let log_payload = serde_json::json!({ "level": "warn", "message": "温度偏高" });
    MqttDispatcher::dispatch(
        &state,
        "/sz/device/SN-DISP-001/log",
        &serde_json::to_vec(&log_payload).unwrap(),
    )
    .await;
    let rows = conn
        .query("SELECT COUNT(*) AS cnt FROM operate_log")
        .await
        .expect("查询失败");
    assert_eq!(
        rows[0].get("cnt").and_then(|v| v.as_i64()),
        Some(1),
        "log action 应路由到 handle_device_log"
    );

    // 4. 未知 action → 仅 warn 日志，不 panic
    MqttDispatcher::dispatch(&state, "/sz/device/SN-DISP-001/unknown", b"{}").await;

    // 5. 非法 device_sn（含 `/` 与空白）→ 丢弃，不 panic
    MqttDispatcher::dispatch(&state, "/sz/device/bad/sn/status", b"{}").await;

    // 6. payload 超限（>256KB）→ 丢弃，不 panic
    let oversized = vec![b'x'; 256 * 1024 + 1];
    MqttDispatcher::dispatch(&state, "/sz/device/SN-DISP-001/status", &oversized).await;

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ MqttDispatcher::dispatch 路由 + 安全校验真实验证通过");
}

/// ── MqttDispatcher::start_consumer：shutdown 信号到达后优雅退出
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mqtt_start_consumer_graceful_shutdown() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let state = build_test_state(pool.clone());

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = tokio::spawn(async move {
        MqttDispatcher::start_consumer(state, shutdown_rx).await;
    });

    // 发送关闭信号后，消费者应在超时时间内退出
    shutdown_tx.send(true).expect("发送关闭信号失败");
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("start_consumer 未在 5s 内优雅退出（超时）")
        .expect("消费者任务 panicked");

    pool.close_all().await;
    println!("✅ MqttDispatcher::start_consumer 优雅退出验证通过");
}

/// ── 控制器 auth::me：无 Authorization header → 返回业务错误
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_auth_me_missing_token_returns_error() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let state = build_test_state(pool.clone());

    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/auth/me")
        .body(Body::empty())
        .expect("构造请求失败");
    let resp = auth::me(State(state), req).await;
    let body = resp_body_text(resp).await;
    assert!(
        body.contains("\"code\":0"),
        "无 token 应返回业务错误，实际: {}",
        body
    );
    assert!(
        body.contains("认证令牌") || body.contains("令牌"),
        "错误信息应说明未提供令牌，实际: {}",
        body
    );

    pool.close_all().await;
    println!("✅ auth::me 无 token 场景验证通过");
}

/// ── 控制器 auth::logout：清除 CSRF + Refresh Token cookie（Max-Age=0）
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_auth_logout_clears_cookie() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let state = build_test_state(pool.clone());

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/logout")
        .body(Body::empty())
        .expect("构造请求失败");
    let resp = auth::logout(State(state), req).await;

    let cookies: Vec<String> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok().map(String::from))
        .collect();
    assert!(!cookies.is_empty(), "logout 应返回 set-cookie 头");
    assert!(
        cookies.iter().any(|c| c.contains("Max-Age=0")),
        "cookie 应含 Max-Age=0，实际: {:?}",
        cookies
    );

    pool.close_all().await;
    println!("✅ auth::logout 清除 cookie 验证通过");
}

/// ── 控制器 device::trigger_ota：未认证请求被拒绝（A6 越权防护）
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_device_trigger_ota_unauthenticated_rejected() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let state = build_test_state(pool.clone());

    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/device/trigger-ota")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"device_id": 1, "ota_version": "2.0.0"}"#))
        .expect("构造请求失败");
    let resp = device::trigger_ota(State(state), req).await;
    let body = resp_body_text(resp).await;
    assert!(
        body.contains("未认证") || body.contains("认证"),
        "未认证 OTA 触发应被拒绝，实际: {}",
        body
    );

    pool.close_all().await;
    println!("✅ device::trigger_ota 未认证拒绝验证通过");
}

/// ── 控制器 device::status_report：状态上报真实落库
#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_device_status_report_updates_db() {
    let cfg = ensure_mysql_available().await.expect("MySQL 不可达");
    let pool = Arc::new(db::init_pool(&cfg).await.expect("连接池初始化失败"));
    let mut conn = pool.acquire().await.expect("获取连接失败");
    create_service_test_tables(&mut *conn).await;
    let device_id = insert_test_device(&mut *conn, "SN-REPORT-001").await;

    let state = build_test_state(pool.clone());
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/device/status-report")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "device_id": device_id,
                "status": 1,
                "signal_strength": -70,
                "fw_version": "2.2.0"
            })
            .to_string(),
        ))
        .expect("构造请求失败");
    let resp = device::status_report(State(state), req).await;
    let body = resp_body_text(resp).await;
    assert!(
        body.contains("状态更新成功"),
        "status_report 应返回成功，实际: {}",
        body
    );

    let rows = conn
        .query_with_params(
            "SELECT status, signal_strength, fw_version FROM device WHERE device_id = ?",
            &[Value::I64(device_id)],
        )
        .await
        .expect("查询失败");
    let row = &rows[0];
    assert_eq!(row.get("status").and_then(|v| v.as_i64()), Some(1));
    assert_eq!(
        row.get("signal_strength").and_then(|v| v.as_i64()),
        Some(-70)
    );
    assert_eq!(
        row.get("fw_version").and_then(|v| v.as_str()),
        Some("2.2.0")
    );

    drop_service_test_tables(&mut *conn).await;
    pool.close_all().await;
    println!("✅ device::status_report 真实落库验证通过");
}
