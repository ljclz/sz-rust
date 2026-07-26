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

#[ignore = "需真实 MySQL 9.6，手动运行: cargo test -- --ignored"]
#[tokio::test]
async fn test_mysql_pool_init_and_query() {
    let cfg = ensure_mysql_available().await
        .expect("MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)");

    let pool = db::init_pool(&cfg).await.expect("MySQL 连接池初始化失败");

    // 验证连接池配置：db.rs 中 max_size=20, min_idle=2
    assert_eq!(pool.config().max_size, 20, "MySQL max_size 应为 20");
    assert_eq!(pool.config().min_idle, 2, "MySQL min_idle 应为 2");

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
    let pg_cfg = pg_test_config();

    let pool = db::init_pg_pool(&pg_cfg).await
        .expect("PostgreSQL 不可达，请启动 PG 18 (127.0.0.1:5432, postgres/test123, sz_orm_test 数据库)");

    // 验证连接池配置：db.rs 中 max_size=10, min_idle=1
    assert_eq!(pool.config().max_size, 10, "PG max_size 应为 10");
    assert_eq!(pool.config().min_idle, 1, "PG min_idle 应为 1");

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
    let cfg = ensure_mysql_available().await
        .expect("MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)");

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
    conn.execute(create_sql)
        .await
        .expect("建表失败");

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
    let cfg = ensure_mysql_available().await
        .expect("MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)");

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
    let cfg = ensure_mysql_available().await
        .expect("MySQL 不可达，请启动 MySQL 9.6 (127.0.0.1:3306, root/test123, sz_orm_test 数据库)");

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
