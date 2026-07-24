//! 对抗性边界测试 — 针对 DML 序列、超大输入、并发碰撞、编码渗透
//!
//! 测试目标：验证 sz-rust-core 的核心模块在对抗性条件下的**行为正确性**和**鲁棒性**。
//! 与 fuzz 测试不同（fuzz 只验证"不 panic"），本测试验证"结果正确"和"边界安全"。
//!
//! ## 覆盖维度
//!
//! | 维度 | 测试用例数 | 覆盖模块 |
//! |------|-----------|---------|
//! | A. DML 操作序列 | 4 | config, env, i18n, mail |
//! | B. 超大输入攻击 | 6 | env, i18n, mail |
//! | C. 并发碰撞 | 3 | env, i18n, mail |
//! | D. 编码/特殊字符渗透 | 6 | env, i18n, mail |
//!
//! ## 安全约束
//!
//! - 不使用 `unsafe` 块
//! - 不使用 `todo!` / `unimplemented!` / `unreachable!`
//! - 所有临时文件在测试结束后清理

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::thread;

use chrono::Utc;
use sz_rust_core::cache::{Cache, MemoryCacheDriver};
use sz_rust_core::config::{DatabaseConnection, DatabaseSection};
use sz_rust_core::cookie::{CookieJar, CookieOptions};
use sz_rust_core::env::Env;
use sz_rust_core::i18n::I18n;
use sz_rust_core::mail::{MailAddress, MailAttachment, Mailer, MailMessage, MemoryMailer};
use sz_rust_core::session::{MemorySessionStore, Session, SessionStore};
use sz_rust_core::config::AppConfig;
use sz_rust_core::config::LogSection;
use sz_rust_core::log::LogFacade;
use sz_rust_core::container::App;
use sz_rust_core::event::{ClosureListener, EventDispatcher};
use sz_rust_core::hooks::{HookContext, HookEvent, HookRegistry};
use sz_rust_core::router::{parse_path, is_app_in_map, ParsedPath};
use sz_rust_core::upload::{HashAlgo, UploadErrCode};
use sz_rust_core::view::{SimpleTemplateEngine, View, ViewConfig, ViewData};
use sz_rust_core::validate::Validate;
use serde_json::Value;

// ============================================================================
// A. DML 操作序列对抗测试（检测状态污染）
// ============================================================================

/// A1: DatabaseSection 创建→修改→删除→重新创建 序列
///
/// 检测：多次修改后的状态一致性和字段隔离性
#[test]
fn adversarial_dml_database_section() {
    // 1. 创建 — 使用默认值
    let mut section = DatabaseSection::default();
    assert_eq!(section.default, "mysql");
    assert!(section.auto_timestamp);
    assert_eq!(section.datetime_format, "Y-m-d H:i:s");
    assert_eq!(section.connections.len(), 0);

    // 2. 修改 — 添加连接并更改默认值
    section.connections.insert(
        "test_db".to_string(),
        DatabaseConnection {
            r#type: "mysql".to_string(),
            hostname: "localhost".to_string(),
            database: "test_db".to_string(),
            username: "root".to_string(),
            password: "secret".to_string(),
            hostport: 3306,
            charset: "utf8mb4".to_string(),
            prefix: "sz_".to_string(),
            deploy: 0,
            rw_separate: false,
            fields_strict: true,
            break_reconnect: true,
        },
    );
    section.default = "test_db".to_string();
    section.auto_timestamp = false;
    section.datetime_format = "Y/m/d H:i:s".to_string();
    assert_eq!(section.connections.len(), 1);
    assert_eq!(section.default, "test_db");
    assert!(!section.auto_timestamp);
    assert_eq!(section.datetime_format, "Y/m/d H:i:s");

    // 验证连接的字段完整
    let conn = section.connections.get("test_db").unwrap();
    assert_eq!(conn.hostname, "localhost");
    assert_eq!(conn.hostport, 3306);
    assert_eq!(conn.prefix, "sz_");

    // 3. 删除连接
    section.connections.remove("test_db");
    section.default = "mysql".to_string();
    assert_eq!(section.connections.len(), 0);
    // 删除后 default 指向默认 "mysql"，但 mysql 连接不存在
    // 验证 default_connection 返回 None（AppConfig 的方法，需完整构造）
    let config = sz_rust_core::config::AppConfig {
        database: section.clone(),
        ..Default::default()
    };
    assert!(config.default_connection().is_none(),
        "删除所有连接后 default_connection() 应返回 None");

    // 4. 重新创建 — 恢复并添加两个连接
    section.connections.insert(
        "mysql".to_string(),
        DatabaseConnection {
            r#type: "mysql".to_string(),
            hostname: "db.example.com".to_string(),
            database: "production".to_string(),
            username: "admin".to_string(),
            password: "new_secret".to_string(),
            hostport: 3307,
            charset: "utf8mb4".to_string(),
            prefix: "prod_".to_string(),
            deploy: 0,
            rw_separate: true,
            fields_strict: true,
            break_reconnect: true,
        },
    );
    section.connections.insert(
        "readonly".to_string(),
        DatabaseConnection {
            r#type: "mysql".to_string(),
            hostname: "readonly.example.com".to_string(),
            database: "analytics".to_string(),
            username: "reader".to_string(),
            password: "readonly_pass".to_string(),
            hostport: 3308,
            charset: "utf8mb4".to_string(),
            prefix: "analytics_".to_string(),
            deploy: 1,
            rw_separate: false,
            fields_strict: false,
            break_reconnect: true,
        },
    );
    section.auto_timestamp = true;

    // 验证重新创建后的完整状态
    assert_eq!(section.connections.len(), 2);
    let mysql_conn = section.connections.get("mysql").unwrap();
    assert_eq!(mysql_conn.hostname, "db.example.com");
    assert_eq!(mysql_conn.hostport, 3307);
    assert_eq!(mysql_conn.database, "production");
    assert!(mysql_conn.rw_separate);

    let ro_conn = section.connections.get("readonly").unwrap();
    assert_eq!(ro_conn.hostname, "readonly.example.com");
    assert_eq!(ro_conn.hostport, 3308);
    assert_eq!(ro_conn.prefix, "analytics_");
    assert_eq!(ro_conn.deploy, 1);
    assert!(!ro_conn.fields_strict);

    assert!(section.auto_timestamp);
    assert_eq!(section.datetime_format, "Y/m/d H:i:s"); // 修改后的格式保留
}

/// A2: Env 设置→覆盖→删除→重新设置 序列
///
/// 检测：内部存储的状态一致性，不污染真实进程环境变量
#[test]
fn adversarial_dml_env() {
    let env = Env::new();

    // 1. 设置
    env.set("DB_HOST", "localhost");
    env.set("DB_PORT", "3306");
    env.set("DB_NAME", "test_db");
    assert_eq!(env.get("DB_HOST"), Some("localhost".to_string()));
    assert_eq!(env.get("DB_PORT"), Some("3306".to_string()));
    assert_eq!(env.get("DB_NAME"), Some("test_db".to_string()));

    // 2. 覆盖
    env.set("DB_HOST", "10.0.0.1");
    env.set("DB_PORT", "8802");
    assert_eq!(env.get("DB_HOST"), Some("10.0.0.1".to_string()));
    assert_eq!(env.get("DB_PORT"), Some("8802".to_string()));
    // 未被覆盖的键保持不变
    assert_eq!(env.get("DB_NAME"), Some("test_db".to_string()));

    // 3. 删除
    assert!(env.remove("DB_HOST"));
    assert!(!env.has("DB_HOST"));
    assert_eq!(env.get("DB_HOST"), None);
    // 删除不存在的键返回 false
    assert!(!env.remove("DB_HOST"));
    assert!(!env.remove("NON_EXISTENT"));

    // 4. 重新设置
    env.set("DB_HOST", "db.cluster.internal");
    env.set("DB_PASSWORD", "rotated_password");
    assert_eq!(env.get("DB_HOST"), Some("db.cluster.internal".to_string()));
    assert_eq!(env.get("DB_PASSWORD"), Some("rotated_password".to_string()));
    // 验证未被删除的键仍存在
    assert_eq!(env.get("DB_PORT"), Some("8802".to_string()));
    assert_eq!(env.get("DB_NAME"), Some("test_db".to_string()));

    // 验证最终快照
    let all = env.all();
    assert_eq!(all.len(), 4);
    assert_eq!(all.get("DB_HOST"), Some(&"db.cluster.internal".to_string()));
    assert_eq!(all.get("DB_PORT"), Some(&"8802".to_string()));
    assert_eq!(all.get("DB_NAME"), Some(&"test_db".to_string()));
    assert_eq!(all.get("DB_PASSWORD"), Some(&"rotated_password".to_string()));
}

/// A3: I18n 加载→覆盖→回退 序列
///
/// 检测：多次加载的累加语义和语言回退链正确性
#[test]
fn adversarial_dml_i18n() {
    let i18n = I18n::with_default_lang("zh-cn");

    // 1. 首次加载
    i18n.load_from_json_str(
        r#"{"hello": "你好", "welcome": "欢迎", "bye": "再见"}"#,
        "zh-cn",
        "<test_a>",
    )
    .unwrap();
    i18n.load_from_json_str(
        r#"{"hello": "Hello", "welcome": "Welcome"}"#,
        "en-us",
        "<test_b>",
    )
    .unwrap();
    assert_eq!(i18n.get_simple("hello", Some("zh-cn")), Some("你好".to_string()));
    assert_eq!(i18n.get_simple("hello", Some("en-us")), Some("Hello".to_string()));
    assert_eq!(i18n.get_simple("bye", Some("zh-cn")), Some("再见".to_string()));
    // en-us 无 bye 键，但 get 会回退到 default_lang (zh-cn) → 找到
    assert_eq!(i18n.get_simple("bye", Some("en-us")), Some("再见".to_string()));

    // 2. 覆盖 — 重新加载部分键
    i18n.load_from_json_str(r#"{"hello": "你好世界", "new_key": "新键"}"#, "zh-cn", "<test_c>").unwrap();
    // 被覆盖的键更新
    assert_eq!(i18n.get_simple("hello", Some("zh-cn")), Some("你好世界".to_string()));
    // 未被覆盖的键保留
    assert_eq!(i18n.get_simple("welcome", Some("zh-cn")), Some("欢迎".to_string()));
    assert_eq!(i18n.get_simple("bye", Some("zh-cn")), Some("再见".to_string()));
    // 新增的键存在
    assert_eq!(i18n.get_simple("new_key", Some("zh-cn")), Some("新键".to_string()));

    // 3. 回退测试
    i18n.set_current_lang("en-us");
    // en-us 存在 hello → 返回 Hello
    assert_eq!(i18n.get_simple("hello", None), Some("Hello".to_string()));

    // 设置仅在 zh-cn 存在的键
    i18n.set("zh-cn", "only_cn", "仅中文");
    // en-us 当前语言无 only_cn → 回退到默认 zh-cn
    assert_eq!(i18n.get_simple("only_cn", None), Some("仅中文".to_string()));

    // 切换到未加载的语言
    i18n.set_current_lang("ja-jp");
    // ja-jp 无任何数据 → 回退到 default_lang (zh-cn)
    // zh-cn 的 hello 已被覆盖为 "你好世界"
    assert_eq!(i18n.get_simple("hello", None), Some("你好世界".to_string()));
    // only_cn 在 zh-cn 中存在
    assert_eq!(i18n.get_simple("only_cn", None), Some("仅中文".to_string()));

    // 完全不存在的键
    assert_eq!(i18n.get_simple("totally_nonexistent", None), None);
}

/// A4: MemoryMailer 发送→清空→发送→读取 序列
///
/// 检测：清空后内部 Vec 不被旧数据污染
#[test]
fn adversarial_dml_memory_mailer() {
    let mailer = MemoryMailer::new();

    // 1. 发送第一批
    let msg1 = MailMessage::new()
        .to("alice@example.com")
        .subject("First Batch")
        .text("This is the first message.");
    let msg2 = MailMessage::new()
        .to("bob@example.com")
        .subject("Second Batch")
        .html("<h1>Second message</h1>");
    mailer.send(msg1).unwrap();
    mailer.send(msg2).unwrap();
    assert_eq!(mailer.count(), 2);

    // 2. 清空
    mailer.clear();
    assert_eq!(mailer.count(), 0);
    assert!(mailer.last().is_none());
    assert!(mailer.all().is_empty());

    // 3. 发送第二批
    let msg3 = MailMessage::new()
        .to("charlie@example.com")
        .subject("Third Batch")
        .html("<h1>Body 3</h1>");
    let msg4 = MailMessage::new()
        .to("dave@example.com")
        .to("eve@example.com")
        .subject("Fourth Batch")
        .text("Body 4");
    mailer.send(msg3).unwrap();
    mailer.send(msg4).unwrap();
    assert_eq!(mailer.count(), 2);

    // 4. 读取验证
    let all = mailer.all();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].subject, "Third Batch");
    assert_eq!(all[0].to[0].email, "charlie@example.com");
    assert_eq!(all[0].html_body, Some("<h1>Body 3</h1>".to_string()));
    assert_eq!(all[1].subject, "Fourth Batch");
    assert_eq!(all[1].to.len(), 2);
    assert_eq!(all[1].to[0].email, "dave@example.com");
    assert_eq!(all[1].to[1].email, "eve@example.com");
    // 无 HTML 内容
    assert!(all[1].html_body.is_none());
    assert_eq!(all[1].text_body, Some("Body 4".to_string()));

    // 验证 last() 返回最后一条
    let last = mailer.last().unwrap();
    assert_eq!(last.subject, "Fourth Batch");
}

// ============================================================================
// B. 超大输入攻击测试
// ============================================================================

/// B1a: env.rs 解析 — 100KB 单行（无等号 → Parse 错误）
#[test]
fn adversarial_huge_single_line_no_eq() {
    let temp_dir = std::env::temp_dir().join("sz_rust_adv_b1a");
    let _ = std::fs::create_dir_all(&temp_dir);
    let env_file = temp_dir.join(".env");

    // 100KB 单行，不含 '='
    let huge_line = "A".repeat(100 * 1024);
    std::fs::write(&env_file, &huge_line).unwrap();

    let env = Env::new();
    let result = env.load_from_file(&env_file);
    // 不含 '=' → 预期 Parse 错误
    assert!(result.is_err(), "100KB 无等号行应返回 Parse 错误");
    match result {
        Err(sz_rust_core::env::EnvError::Parse { line, .. }) => {
            assert_eq!(line, 1);
        }
        _ => panic!("期望 Parse 错误"),
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// B1a2: env.rs 解析 — 100KB 单行键值对
#[test]
fn adversarial_huge_single_line_kv() {
    let temp_dir = std::env::temp_dir().join("sz_rust_adv_b1a2");
    let _ = std::fs::create_dir_all(&temp_dir);
    let env_file = temp_dir.join(".env");

    // 100KB 单行键值对
    let huge_value = "V".repeat(100 * 1024 - 4); // 减去 "K=" 的长度
    let content = format!("K={}", huge_value);
    std::fs::write(&env_file, &content).unwrap();

    let env = Env::new();
    env.load_from_file(&env_file).unwrap();

    let val = env.get("K").unwrap();
    assert_eq!(val.len(), 100 * 1024 - 4);
    assert!(val.chars().all(|c| c == 'V'));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// B1b: env.rs 解析 — 10MB 总内容（5 万行有效键值对）
#[test]
fn adversarial_huge_total_ini() {
    let temp_dir = std::env::temp_dir().join("sz_rust_adv_b1b");
    let _ = std::fs::create_dir_all(&temp_dir);
    let env_file = temp_dir.join(".env");

    // 生成约 10MB 的有效 INI 内容（每行约 150 字节）
    let mut content = String::with_capacity(12 * 1024 * 1024);
    for i in 0..70000 {
        content.push_str(&format!("KEY_{} = {}\n", i, "V".repeat(140)));
    }
    // 确保至少 9MB
    assert!(content.len() >= 9 * 1024 * 1024, "生成内容应接近 10MB，实际 {} bytes", content.len());

    std::fs::write(&env_file, &content).unwrap();

    let env = Env::new();
    env.load_from_file(&env_file).unwrap();

    // 验证首尾和中间采样
    assert_eq!(env.get("KEY_0"), Some("V".repeat(140)));
    assert_eq!(env.get("KEY_69999"), Some("V".repeat(140)));
    assert_eq!(env.get("KEY_35000"), Some("V".repeat(140)));

    // 验证总数
    let all = env.all();
    assert_eq!(all.len(), 70000, "应存储 70000 个键值对");
    assert!(all.contains_key("KEY_0"));
    assert!(all.contains_key("KEY_69999"));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// B1c: env.rs 解析 — 嵌套 100 层 section
#[test]
fn adversarial_deeply_nested_sections() {
    let temp_dir = std::env::temp_dir().join("sz_rust_adv_b1c");
    let _ = std::fs::create_dir_all(&temp_dir);
    let env_file = temp_dir.join(".env");

    // 生成 100 层 section
    let mut content = String::new();
    for i in 0..100 {
        content.push_str(&format!("[section_{}]\n", i));
        content.push_str(&format!("level = {}\n", i));
    }

    std::fs::write(&env_file, &content).unwrap();

    let env = Env::new();
    env.load_from_file(&env_file).unwrap();

    // 验证每层 section 都正确解析
    for i in 0..100 {
        let key = format!("section_{}.level", i);
        let val = env.get(&key);
        assert!(val.is_some(), "第 {} 层 section 解析失败", i);
        assert_eq!(val.unwrap(), format!("{}", i));
    }

    // 验证 section 隔离性
    let all = env.all();
    assert_eq!(all.len(), 100);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// B2a: i18n.rs 插值 — 10,000 个不同占位符替换
#[test]
fn adversarial_i18n_many_placeholders() {
    let i18n = I18n::new();

    // 构造含 10,000 个不同占位符的模板
    let mut template = String::with_capacity(100000);
    let mut vars = HashMap::new();
    for i in 0..10000 {
        template.push_str(&format!(":{},", i));
        vars.insert(format!("{}", i), format!("val_{}", i));
    }
    // 去掉末尾逗号
    template.pop();

    i18n.set("en-us", "many_vars", &template);
    let result = i18n.get("many_vars", &vars, Some("en-us")).unwrap();

    // 验证每个占位符都被替换
    for i in 0..10000 {
        let expected = format!("val_{}", i);
        assert!(
            result.contains(&expected),
            "占位符 {} 应被替换为 {}", i, expected
        );
    }

    // 验证结果中不包含任何未替换的占位符
    assert!(!result.contains(":"));
    assert_eq!(result.matches("val_").count(), 10000);

    // 验证逗号分隔正确（无丢失）
    assert_eq!(result.matches(",").count(), 9999);
}

/// B2b: i18n.rs 插值 — 单变量被引用 1000 次
#[test]
fn adversarial_i18n_single_var_many_times() {
    let i18n = I18n::new();

    // 构造同一变量被引用 1000 次的模板
    let template = ":name,".repeat(1000);
    let mut vars = HashMap::new();
    vars.insert("name".to_string(), "Alice".to_string());

    i18n.set("en-us", "many_refs", &template);
    let result = i18n.get("many_refs", &vars, Some("en-us")).unwrap();

    // 验证：结果应包含 1000 个 "Alice"
    let count = result.matches("Alice").count();
    assert_eq!(count, 1000, "应替换 1000 次，实际替换 {} 次", count);

    // 验证长度：1000 个 "Alice" + 1000 个逗号（末尾也有逗号）
    assert_eq!(result.len(), 1000 * 6);
}

/// B3a: mail.rs MailMessage — 10,000 个收件人
#[test]
fn adversarial_mail_many_recipients() {
    let mut msg = MailMessage::new()
        .subject("Bulk Email")
        .text("This is a bulk email for testing.");

    for i in 0..10000 {
        msg = msg.to(format!("user{}@example.com", i));
    }

    assert_eq!(msg.to.len(), 10000);

    let mailer = MemoryMailer::new();
    mailer.send(msg).unwrap();

    let sent = mailer.last().unwrap();
    assert_eq!(sent.to.len(), 10000);
    // 验证首尾
    assert_eq!(sent.to[0].email, "user0@example.com");
    assert_eq!(sent.to[9999].email, "user9999@example.com");
    // 验证中间
    assert_eq!(sent.to[5000].email, "user5000@example.com");
}

/// B3b: mail.rs MailMessage — 100MB 附件
#[test]
fn adversarial_mail_huge_attachment() {
    // 创建 100MB 附件内容
    let huge_content = vec![0u8; 100 * 1024 * 1024];
    let attachment = MailAttachment::new("huge_file.bin", huge_content, "application/octet-stream");

    let msg = MailMessage::new()
        .to("user@example.com")
        .subject("Huge Attachment Test")
        .text("Please find the attached large file.")
        .attach(attachment);

    let mailer = MemoryMailer::new();
    mailer.send(msg).unwrap();

    let sent = mailer.last().unwrap();
    assert_eq!(sent.attachments.len(), 1);
    assert_eq!(sent.attachments[0].filename, "huge_file.bin");
    assert_eq!(sent.attachments[0].content.len(), 100 * 1024 * 1024);
    assert_eq!(sent.attachments[0].mime_type, "application/octet-stream");
    // 验证内容完整性
    assert_eq!(sent.attachments[0].content[0], 0u8);
    assert_eq!(sent.attachments[0].content[50 * 1024 * 1024], 0u8);
    assert_eq!(sent.attachments[0].content[100 * 1024 * 1024 - 1], 0u8);
}

// ============================================================================
// C. 并发碰撞测试
// ============================================================================

/// C1: Env 10 个并发线程同时 set/get/remove 同一个 key
///
/// 检测：并发写入后的内部 HashMap 不损坏，读写锁正常工作
#[test]
fn adversarial_concurrent_env() {
    let env = Arc::new(Env::new());
    let barrier = Arc::new(Barrier::new(11)); // 10 workers + 1 main
    let mut handles = vec![];

    // 预先设置一个值，供并发读取
    env.set("shared_key", "initial");

    for i in 0..10 {
        let env = Arc::clone(&env);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();

            for _ in 0..200 {
                env.set("shared_key", &format!("thread_{}", i));
                let _ = env.get("shared_key");
                let _ = env.has("shared_key");
            }
            // 偶数线程删除 key
            if i % 2 == 0 {
                env.remove("shared_key");
            }
        }));
    }

    barrier.wait();
    for _ in 0..200 {
        env.set("shared_key", "main_thread");
        let _ = env.get("shared_key");
        let _ = env.has("shared_key");
    }

    for handle in handles {
        handle.join().expect("线程应正常结束");
    }

    // 验证：并发操作后内部存储不损坏
    let all = env.all();
    // shared_key 可能被某些线程删除或设置，但不应导致 panic
    let _ = env.get("shared_key");

    // 验证读写锁仍正常工作
    env.set("post_concurrent", "still_works");
    assert_eq!(env.get("post_concurrent"), Some("still_works".to_string()));

    println!("并发测试后 env 存储条目数: {}", all.len());
}

/// C2: I18n 10 个并发线程同时 set/get/has 同一个语言
///
/// 检测：并发写入后语言数据完整，读写锁不损坏
#[test]
fn adversarial_concurrent_i18n() {
    let i18n = Arc::new(I18n::new());
    let barrier = Arc::new(Barrier::new(11));
    let mut handles = vec![];

    for i in 0..10 {
        let i18n = Arc::clone(&i18n);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for j in 0..200 {
                let key = format!("key_{}_{}", i, j);
                let value = format!("value_{}_{}", i, j);
                i18n.set("en-us", &key, &value);
                let _ = i18n.get_simple(&key, Some("en-us"));
                let _ = i18n.has(&key, Some("en-us"));
            }
        }));
    }

    barrier.wait();
    for j in 0..200 {
        i18n.set("en-us", &format!("main_key_{}", j), &format!("main_value_{}", j));
    }

    for handle in handles {
        handle.join().expect("线程应正常结束");
    }

    // 验证并发写入后读写锁未损坏
    let en_data = i18n.all_for_lang("en-us");
    // 10 线程 * 200 + 主线程 200 = 2200 个唯一键
    assert_eq!(en_data.len(), 2200, "应包含所有 2200 个唯一键");

    // 验证主线程写入的键完整
    for j in 0..200 {
        let key = format!("main_key_{}", j);
        assert_eq!(
            en_data.get(&key),
            Some(&format!("main_value_{}", j)),
            "主线程写入的键 {} 不完整", key
        );
    }

    // 验证读写锁仍能正常工作
    i18n.set("en-us", "post_concurrent", "works");
    assert_eq!(
        i18n.get_simple("post_concurrent", Some("en-us")),
        Some("works".to_string())
    );
}

/// C3: MemoryMailer 10 个并发线程同时 send
///
/// 检测：并发发送后邮件计数正确，内容完整
#[test]
fn adversarial_concurrent_memory_mailer() {
    let mailer = Arc::new(MemoryMailer::new());
    let barrier = Arc::new(Barrier::new(11));
    let mut handles = vec![];

    for i in 0..10 {
        let mailer = Arc::clone(&mailer);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for j in 0..50 {
                let msg = MailMessage::new()
                    .to(format!("user{}_{}@example.com", i, j))
                    .subject(format!("Subject {}_{}", i, j))
                    .text(format!("Body {}_{}", i, j));
                mailer.send(msg).unwrap();
            }
        }));
    }

    barrier.wait();
    for j in 0..50 {
        let msg = MailMessage::new()
            .to(format!("main_{}@example.com", j))
            .subject(format!("Main Subject {}", j))
            .text(format!("Main Body {}", j));
        mailer.send(msg).unwrap();
    }

    for handle in handles {
        handle.join().expect("线程应正常结束");
    }

    // 验证：并发发送后邮件总数正确
    // 10 线程 * 50 封 + 主线程 50 封 = 550 封
    assert_eq!(mailer.count(), 550, "应发送 550 封邮件");

    // 验证主线程每封邮件都可定位
    let all = mailer.all();
    let main_subjects: Vec<&str> = all.iter().filter_map(|m| {
        if m.subject.starts_with("Main Subject ") {
            Some(m.subject.as_str())
        } else {
            None
        }
    }).collect();
    assert_eq!(main_subjects.len(), 50, "主线程 50 封邮件应全部存在");

    // 验证 post_concurrent 发送仍正常
    let final_msg = MailMessage::new()
        .to("final@example.com")
        .subject("Final Check")
        .text("Final verification");
    mailer.send(final_msg).unwrap();
    assert_eq!(mailer.count(), 551);
}

// ============================================================================
// D. 编码/特殊字符渗透测试
// ============================================================================

/// D1a: env.rs 解析 — null 字节 embedded（键和值中）
#[test]
fn adversarial_encoding_env_null_byte() {
    let temp_dir = std::env::temp_dir().join("sz_rust_adv_d1a");
    let _ = std::fs::create_dir_all(&temp_dir);
    let env_file = temp_dir.join(".env");

    // null 字节在键中（Rust 字符串支持 \0）
    let content = b"KEY_WITH_NULL\x00_BYTE = value_without_null\nNORMAL = ok\n";
    std::fs::write(&env_file, content).unwrap();

    let env = Env::new();
    let result = env.load_from_file(&env_file);

    // 由于 Rust 字符串支持 null 字节，解析应成功
    assert!(result.is_ok(), "含 null 字节的键应能解析成功");

    // 含 null 字节的键应可检索
    let key_with_null = "KEY_WITH_NULL\x00_BYTE";
    let val = env.get(key_with_null);
    assert!(val.is_some(), "含 null 字节的键应可检索");
    if let Some(v) = val {
        assert_eq!(v, "value_without_null");
    }

    // 正常键不受影响
    assert_eq!(env.get("NORMAL"), Some("ok".to_string()));

    // null 字节在值中
    let env_file2 = temp_dir.join(".env2");
    let content2 = b"NORMAL_KEY = value_with_\x00_null";
    std::fs::write(&env_file2, content2).unwrap();

    let env2 = Env::new();
    env2.load_from_file(&env_file2).unwrap();
    let val2 = env2.get("NORMAL_KEY").unwrap();
    // null 字节应在值中保留
    assert!(val2.contains('\0'), "null 字节应在值中保留");
    assert_eq!(val2, "value_with_\0_null");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// D1b: env.rs 解析 — 无限递归 key（100KB 超长键名）
#[test]
fn adversarial_encoding_env_recursive_key() {
    let env = Env::new();

    // 100KB 超长键名
    let long_key = "K".repeat(100 * 1024);
    env.set(&long_key, "value_of_long_key");
    let val = env.get(&long_key);
    assert!(val.is_some(), "100KB 键名应能存储");
    assert_eq!(val.unwrap(), "value_of_long_key");

    // 验证其他键不受影响
    env.set("normal_key", "normal_value");
    assert_eq!(env.get("normal_key"), Some("normal_value".to_string()));

    // 通过 load_from_file 测试超长键名解析
    let temp_dir = std::env::temp_dir().join("sz_rust_adv_d1b");
    let _ = std::fs::create_dir_all(&temp_dir);
    let env_file = temp_dir.join(".env");

    // section 内超长键
    let mut content = String::new();
    content.push_str("[section]\n");
    let long_section_key = "K".repeat(50 * 1024);
    content.push_str(&format!("{} = deep_value\n", long_section_key));
    std::fs::write(&env_file, &content).unwrap();

    let env2 = Env::new();
    env2.load_from_file(&env_file).unwrap();
    let full_key = format!("section.{}", long_section_key);
    let val2 = env2.get(&full_key);
    assert!(val2.is_some(), "section 内超长键名应能存储");
    assert_eq!(val2.unwrap(), "deep_value");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// D1c: env.rs 解析 — BOM 头
#[test]
fn adversarial_encoding_env_bom() {
    let temp_dir = std::env::temp_dir().join("sz_rust_adv_d1c");
    let _ = std::fs::create_dir_all(&temp_dir);
    let env_file = temp_dir.join(".env");

    // UTF-8 BOM（\u{FEFF}）开头的 INI 内容
    let bom: String = "\u{FEFF}".to_string();
    let content = format!("{}APP_KEY = value_with_bom\nNORMAL = ok", bom);
    std::fs::write(&env_file, &content).unwrap();

    let env = Env::new();
    let result = env.load_from_file(&env_file);

    // BOM 开头的行：trim 不会移除 \u{FEFF}（它不是空白字符）
    // 第一行变为 "\u{FEFF}APP_KEY = value_with_bom"
    // 包含 '='，key 为 "\u{FEFF}APP_KEY"，value 为 "value_with_bom"
    // 解析应成功，但键包含 BOM 字符
    assert!(result.is_ok(), "BOM 开头的 INI 应能解析成功（BOM 作为键前缀保留）");

    // 验证 BOM 在键中保留
    let bom_key = "\u{FEFF}APP_KEY".to_string();
    let val = env.get(&bom_key);
    assert!(val.is_some(), "BOM 开头的键应可检索");
    assert_eq!(val.unwrap(), "value_with_bom");

    // 正常行不受影响
    assert_eq!(env.get("NORMAL"), Some("ok".to_string()));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

/// D2: i18n.rs — HTML 实体注入 + XSS payload 通过插值绕过
///
/// 检测：插值函数不应转义 HTML，但也不应 panic 或损坏数据
/// 注意：转义是模板层的职责，i18n 层只做文本替换，这是设计如此
#[test]
fn adversarial_encoding_i18n_xss_bypass() {
    let i18n = I18n::new();

    // 场景 1：HTML 实体在模板中
    i18n.set("en-us", "html_entity", "&lt;script&gt;alert('xss')&lt;/script&gt;");
    let result = i18n.get_simple("html_entity", Some("en-us")).unwrap();
    assert_eq!(result, "&lt;script&gt;alert('xss')&lt;/script&gt;");
    // 验证 HTML 实体未被二次转义
    assert!(result.contains("&lt;"));
    assert!(result.contains("&gt;"));

    // 场景 2：XSS payload 通过插值变量注入
    i18n.set("en-us", "user_greeting", "Hello, :name!");
    let mut vars = HashMap::new();
    vars.insert(
        "name".to_string(),
        "<script>alert('xss')</script>".to_string(),
    );
    let result2 = i18n.get("user_greeting", &vars, Some("en-us")).unwrap();
    // 插值不转义 HTML（设计如此），但不应 panic
    assert_eq!(
        result2,
        "Hello, <script>alert('xss')</script>!"
    );
    assert!(result2.contains("<script>"));

    // 场景 3：插值变量含占位符语法（试图绕过）
    vars.clear();
    vars.insert("name".to_string(), ":name".to_string()); // 自我引用
    i18n.set("en-us", "self_ref", ":name");
    let result3 = i18n.get("self_ref", &vars, Some("en-us")).unwrap();
    // 单次替换后结果仍是 ":name"（已替换为 ":name"），不会二次替换
    assert_eq!(result3, ":name");

    // 场景 4：变量值含 {name} 格式
    vars.clear();
    vars.insert("name".to_string(), "{name}".to_string());
    let result4 = i18n.get("self_ref", &vars, Some("en-us")).unwrap();
    // 替换后结果变为 "{name}"，不会二次替换
    assert_eq!(result4, "{name}");
}

/// D3: mail.rs MailAddress — RFC 5322 畸形地址 + 超长 local part
///
/// 检测：MailAddress 构造和格式化在面对畸形输入时不 panic
#[test]
fn adversarial_encoding_mail_malformed_addresses() {
    // 场景 1：RFC 5322 畸形地址（无 @）
    let addr = MailAddress::new("not-an-email");
    assert_eq!(addr.email, "not-an-email");
    assert_eq!(addr.to_rfc5322_string(), "not-an-email");

    // 场景 2：空字符串
    let addr2 = MailAddress::new("");
    assert_eq!(addr2.email, "");
    assert_eq!(addr2.to_rfc5322_string(), "");

    // 场景 3：只有 @
    let addr3 = MailAddress::new("@");
    assert_eq!(addr3.to_rfc5322_string(), "@");

    // 场景 4：多个 @
    let addr4 = MailAddress::new("user@@@example.com");
    assert_eq!(addr4.to_rfc5322_string(), "user@@@example.com");

    // 场景 5：含特殊字符的 local part
    let addr5 = MailAddress::new("user\\\"test\\\"@example.com");
    let rfc_str = addr5.to_rfc5322_string();
    assert!(rfc_str.contains("\\"));

    // 场景 6：超长 local part（100KB）
    let long_local = "a".repeat(100 * 1024);
    let long_email = format!("{}@example.com", long_local);
    let addr6 = MailAddress::new(&long_email);
    assert_eq!(addr6.email.len(), 100 * 1024 + 12); // local + @example.com
    let rfc6 = addr6.to_rfc5322_string();
    assert!(rfc6.starts_with("aaa"));
    assert!(rfc6.ends_with("@example.com"));

    // 场景 7：带引号显示名（含特殊字符）
    let addr7 = MailAddress::with_name("user@example.com", "John \"Quoted\" Doe");
    let rfc7 = addr7.to_rfc5322_string();
    // RFC 5322 中显示名内的引号应该被转义，但当前实现不做转义
    // 本测试只验证不 panic，且格式保持
    assert!(rfc7.starts_with("\""));
    assert!(rfc7.contains("\"John \"Quoted\" Doe\""));

    // 场景 8：MailMessage 使用畸形地址发送不 panic
    let mailer = MemoryMailer::new();
    let msg = MailMessage::new()
        .to(MailAddress::new("not-an-email"))
        .to(MailAddress::new(""))
        .to(MailAddress::new("@"))
        .to(MailAddress::new("a".repeat(50000) + "@example.com"))
        .subject("Malformed addresses")
        .text("Testing malformed email addresses");
    mailer.send(msg).unwrap();

    let sent = mailer.last().unwrap();
    assert_eq!(sent.to.len(), 4);
    assert_eq!(sent.to[0].email, "not-an-email");
    assert_eq!(sent.to[1].email, "");
    assert_eq!(sent.to[2].email, "@");
    assert_eq!(sent.to[3].email.len(), 50000 + 12);
}

// ============================================================================
// E. Session 对抗性测试
// ============================================================================

/// E1: Session DML 序列 — 创建→设置→获取→删除→清空→重新创建
#[test]
fn adversarial_session_dml_sequence() {
    let store = MemorySessionStore::new();
    let session = Session::new("session_dml", store);

    // 1. 初始状态：空 session
    assert!(!session.has("key1"));
    assert!(session.get("key1").is_none());
    assert!(session.all().is_empty());

    // 2. 设置多个值
    session.set("key1", Value::String("val1".into()));
    session.set("key2", Value::Number(42.into()));
    session.set("key3", Value::Bool(true));
    assert_eq!(session.get("key1"), Some(Value::String("val1".into())));
    assert!(session.has("key2"));
    assert!(session.has("key3"));
    assert_eq!(session.all().len(), 3);

    // 3. 覆盖值
    session.set("key1", Value::String("overwritten".into()));
    assert_eq!(
        session.get("key1"),
        Some(Value::String("overwritten".into()))
    );

    // 4. 删除单个键
    let deleted = session.delete("key3");
    assert_eq!(deleted, Some(Value::Bool(true)));
    assert!(!session.has("key3"));

    // 5. 清空
    session.clear();
    assert!(!session.has("key1"));
    assert!(!session.has("key2"));
    assert!(session.all().is_empty());

    // 6. 清空后重新设置
    session.set("new_key", Value::String("new_val".into()));
    assert_eq!(
        session.get("new_key"),
        Some(Value::String("new_val".into()))
    );
}

/// E2: Session 空/特殊 session_id 创建
#[test]
fn adversarial_session_edge_ids() {
    let store = MemorySessionStore::new();

    // 空字符串
    let s1 = Session::new("", store);
    assert!(s1.all().is_empty());
    s1.set("k", Value::String("v".into()));
    assert_eq!(s1.get("k"), Some(Value::String("v".into())));

    // 超长 session_id（10KB）
    let store2 = MemorySessionStore::new();
    let long_id = "A".repeat(10 * 1024);
    let s2 = Session::new(&long_id, store2);
    s2.set("key", Value::String("value".into()));
    assert_eq!(s2.get("key"), Some(Value::String("value".into())));

    // 特殊字符 session_id
    let store3 = MemorySessionStore::new();
    let s3 = Session::new("id with spaces\tand\nnewlines", store3);
    s3.set("x", Value::Number(1.into()));
    assert!(s3.has("x"));
}

/// E3: Session Flash DML 序列
#[test]
fn adversarial_session_flash_sequence() {
    let store = MemorySessionStore::new();
    let session = Session::new("flash_dml", store);

    // 设置 flash
    session.flash("notice", Value::String("Saved!".into()));
    session.flash("error", Value::String("Failed!".into()));
    assert_eq!(
        session.get_flash("notice"),
        Some(Value::String("Saved!".into()))
    );
    assert_eq!(
        session.get_flash("error"),
        Some(Value::String("Failed!".into()))
    );

    // 清空 flash
    session.clear_flash();
    assert!(session.get_flash("notice").is_none());
    assert!(session.get_flash("error").is_none());

    // flash 与普通 key 不应冲突
    session.set("notice", Value::String("normal".into()));
    assert!(session.get_flash("notice").is_none()); // flash 已清空
    assert_eq!(session.get("notice"), Some(Value::String("normal".into())));
}

/// E4: Session 并发安全
#[test]
fn adversarial_session_concurrent() {
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let session = Arc::new(Session::with_shared_store("shared", Arc::clone(&store)));
    let barrier = Arc::new(Barrier::new(6));
    let mut handles = vec![];

    for i in 0..5 {
        let session = Arc::clone(&session);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for j in 0..100 {
                session.set(
                    &format!("thread{}_{}", i, j),
                    Value::String(format!("val_{}_{}", i, j)),
                );
                let _ = session.get(&format!("thread{}_{}", i, j));
            }
        }));
    }

    barrier.wait();

    for handle in handles {
        handle.join().expect("线程应正常结束");
    }

    // 验证读写锁不损坏（共享同一 session，并发写入后 post_concurrent 应正常）
    session.set("post_concurrent", Value::String("works".into()));
    assert_eq!(
        session.get("post_concurrent"),
        Some(Value::String("works".into()))
    );
}

// ============================================================================
// F. Cookie 对抗性测试
// ============================================================================

/// F1: CookieJar 构建与 set 链式边界
#[test]
fn adversarial_cookie_jar_chain_operations() {
    use sz_rust_core::cookie::CookieEntry;

    // 空 CookieJar
    let jar = CookieJar::new();
    assert!(!jar.has("nonexistent"));
    assert!(jar.get("any").is_none());

    // set 链式调用
    let jar = CookieJar::new()
        .set("k1", "v1", CookieOptions::default())
        .set("k2", "v2", CookieOptions::default());

    let cookies = jar.get_response_cookies();
    assert_eq!(cookies.len(), 2);

    // forever
    let jar = CookieJar::new()
        .forever("forever_key", "forever_val", CookieOptions::default());
    let cookies = jar.get_response_cookies();
    let forever_entry = cookies.iter().find(|c| c.name == "forever_key").unwrap();
    // forever 的 expire 应该至少为 315360000（10 年）
    assert!(forever_entry.expire >= 315360000);

    // CookieEntry to_header_string 不 panic
    let entry = CookieEntry {
        name: "test".to_string(),
        value: "val".to_string(),
        expire: 0,
        options: CookieOptions::default(),
    };
    let header = entry.to_header_string();
    assert!(header.contains("test=val"));
    assert!(!header.contains("Expires")); // expire=0 不输出 Expires

    // delete 操作
    let jar = CookieJar::new().set("k", "v", CookieOptions::default());
    let jar = jar.delete("k", CookieOptions::default());
    let cookies = jar.get_response_cookies();
    let delete_entry = cookies.iter().find(|c| c.name == "k").unwrap();
    // delete 设置 expire = now - 3600，是一个过去的正时间戳
    assert!(delete_entry.expire < Utc::now().timestamp());
}

/// F2: CookieOptions 构造边界
#[test]
fn adversarial_cookie_options_boundaries() {
    // 默认值
    let opts = CookieOptions::default();
    assert_eq!(opts.expire, 0);
    assert_eq!(opts.path, "/");
    assert_eq!(opts.domain, "");

    // 负 expire
    let neg = CookieOptions::with_expire(-1);
    assert_eq!(neg.expire, -1);

    // max expire
    let max_exp = CookieOptions::with_expire(86400 * 365 * 100); // 100 年
    assert_eq!(max_exp.expire, 86400 * 365 * 100);
}

// ============================================================================
// G. Cache 对抗性测试
// ============================================================================

/// G1: Cache DML 序列 — set→get→inc→dec→delete→clear→remember
#[test]
fn adversarial_cache_dml_sequence() {
    let driver = MemoryCacheDriver::new();
    let cache = Cache::new();
    cache.register_default(driver);

    // 1. set + get
    cache.set("k1", "value1", None).unwrap();
    assert_eq!(cache.get::<String>("k1").unwrap(), Some("value1".to_string()));

    // 2. 覆盖
    cache.set("k1", "overwritten", None).unwrap();
    assert_eq!(
        cache.get::<String>("k1").unwrap(),
        Some("overwritten".to_string())
    );

    // 3. inc / dec
    assert_eq!(cache.inc("counter", 1).unwrap(), 1);
    assert_eq!(cache.inc("counter", 5).unwrap(), 6);
    assert_eq!(cache.dec("counter", 2).unwrap(), 4);

    // 4. has
    assert!(cache.has("k1").unwrap());
    assert!(cache.has("counter").unwrap());
    assert!(!cache.has("nonexistent").unwrap());

    // 5. delete
    cache.delete("k1").unwrap();
    assert!(!cache.has("k1").unwrap());

    // 6. pull (delete + return)
    cache.set("pull_me", "pulled", None).unwrap();
    let pulled = cache.pull::<String>("pull_me").unwrap();
    assert_eq!(pulled, Some("pulled".to_string()));
    assert!(!cache.has("pull_me").unwrap());

    // 7. clear
    cache.set("after_clear", "gone", None).unwrap();
    cache.clear().unwrap();
    assert!(!cache.has("after_clear").unwrap());

    // 8. remember
    let computed = cache
        .remember("computed", None, || "lazy_value".to_string())
        .unwrap();
    assert_eq!(computed, "lazy_value".to_string());
    // 再次 remember 应从缓存取
    let cached: String = cache
        .remember("computed", None, || panic!("不应再次调用"))
        .unwrap();
    assert_eq!(cached, "lazy_value".to_string());
}

/// G2: Cache 超大 value
#[test]
fn adversarial_cache_huge_value() {
    let driver = MemoryCacheDriver::new();
    let cache = Cache::new();
    cache.register_default(driver);

    // 10MB value
    let huge: String = "X".repeat(10 * 1024 * 1024);
    cache.set("huge", &huge, None).unwrap();
    let retrieved = cache.get::<String>("huge").unwrap();
    assert_eq!(retrieved, Some(huge.clone()));
    assert_eq!(retrieved.unwrap().len(), 10 * 1024 * 1024);
}

/// G3: Cache 并发碰撞
#[test]
fn adversarial_cache_concurrent() {
    let cache = Arc::new({
        let c = Cache::new();
        c.register_default(MemoryCacheDriver::new());
        c
    });
    let barrier = Arc::new(Barrier::new(6));
    let mut handles = vec![];

    for i in 0..5 {
        let cache = Arc::clone(&cache);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for j in 0..100 {
                cache
                    .set(
                        &format!("key_{}_{}", i, j),
                        format!("val_{}_{}", i, j),
                        None,
                    )
                    .unwrap();
                let _ = cache.get::<String>(&format!("key_{}_{}", i, j));
            }
        }));
    }

    barrier.wait();

    for handle in handles {
        handle.join().expect("线程应正常结束");
    }

    // 验证 post_concurrent 正常
    cache.set("final", "works", None).unwrap();
    assert_eq!(
        cache.get::<String>("final").unwrap(),
        Some("works".to_string())
    );
}

// ============================================================================
// H. Validate 对抗性测试
// ============================================================================

/// H1: Validate Rule::from_string 边界
#[test]
fn adversarial_validate_rule_parse_boundaries() {
    use sz_rust_core::validate::Rule;

    // 空字符串 → Rule::Simple("") → to_list 返回 [("", "")]
    let r = Rule::from_string("");
    assert_eq!(r.to_list().len(), 1);

    // 纯管道符 → Rule::Multiple([Simple(""), Simple("")]) → to_list 返回 [("", ""), ("", "")]
    let r = Rule::from_string("|");
    assert_eq!(r.to_list().len(), 2);

    // 纯冒号 → Rule::WithArgs("", "") → to_list 返回 [("", "")]
    let r = Rule::from_string(":");
    let list = r.to_list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].0, "");
    assert!(list[0].1.is_empty());

    // 管道符边界
    let r = Rule::from_string("require||email");
    let list = r.to_list();
    assert_eq!(list.len(), 3); // split("|") 产生 ["require", "", "email"]，空字符串不被过滤
    assert_eq!(list[0].0, "require");
    assert_eq!(list[1].0, "");

    // 含冒号的规则分割
    let r = Rule::from_string("in:1,2,3|length:1,10");
    let list = r.to_list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].0, "in");
    assert_eq!(list[0].1, "1,2,3");
    assert_eq!(list[1].0, "length");
    assert_eq!(list[1].1, "1,10");

    // 只有冒号没有参数的规则
    let r = Rule::from_string("max:|min:");
    let list = r.to_list();
    assert_eq!(list.len(), 2);
    assert!(list[0].1.is_empty());
    assert!(list[1].1.is_empty());
}

/// H2: Validate require/must 边界值
#[test]
fn adversarial_validate_require_boundaries() {
    use sz_rust_core::validate::Validate;
    use serde_json::json;

    // require("0") → 应该为 true（PHP 特殊行为）
    assert!(Validate::require(&json!("0"), ""));

    // require(0) → 数字 0 在 empty() 中为 true，且不是字符串 "0"，所以 require 返回 false
    assert!(!Validate::require(&json!(0), ""));

    // require(false) → false（布尔 false 在 empty() 中为 true）
    assert!(!Validate::require(&json!(false), ""));

    // require(null) → false
    assert!(!Validate::require(&Value::Null, ""));

    // require("") → false
    assert!(!Validate::require(&json!(""), ""));

    // require(空数组) → false
    assert!(!Validate::require(&json!([]), ""));

    // must 与 require 实现相同（!is_empty_value || string "0"）
    assert!(!Validate::must(&Value::Null, ""));
    assert!(!Validate::must(&json!(""), ""));
}

/// H3: Validate check DML 序列 — 多规则链式验证
#[test]
fn adversarial_validate_check_dml_sequence() {
    use serde_json::json;

    // 1. 首次验证 — 全字段通过
    // 注意：min/max 规则检查的是值的字符串长度（PHP mb_strlen 对齐）
    let mut v = Validate::new()
        .rule("name", "require|length:2,10")
        .rule("age", "require|integer|min:2|max:3");
    let data = json!({"name": "Alice", "age": 100}); // "100" 长度 3，在 [2,3] 内
    assert!(v.check(&data).is_ok());

    // 2. 验证失败 — age 太小（字符串长度 1 < 2）
    let data2 = json!({"name": "Bob", "age": 5}); // "5" 长度 1，小于 min:2
    assert!(v.check(&data2).is_err());

    // 3. 移除 age 规则后验证
    let mut v2 = Validate::new()
        .rule("name", "require|length:2,10")
        .rule("age", "require|integer|min:2|max:3")
        .remove("age", None);
    assert!(v2.check(&data2).is_ok());

    // 4. 只验证部分字段
    let mut v3 = Validate::new()
        .rule("name", "require|length:2,10")
        .rule("age", "require|integer|min:2|max:3")
        .only(vec!["name".to_string()]);
    assert!(v3.check(&data2).is_ok()); // age 被 only 排除，name 合法

    // 5. 批量模式 — 收集所有错误
    let mut v4 = Validate::new()
        .rule("name", "require|min:5")
        .rule("age", "require|integer|max:10")
        .batch(true);
    let data3 = json!({"name": "A", "age": "abc"}); // name 长度不够且 age 不是整数
    let result = v4.check(&data3);
    assert!(result.is_err());
    use sz_rust_core::validate::ValidateError;
    match result.unwrap_err() {
        ValidateError::Batch(errors) => {
            assert!(errors.len() >= 2, "批量模式应收集多个错误");
        }
        _ => panic!("批量模式应返回 Batch 错误"),
    }

    // 6. 场景切换后重置 only/remove/append
    let mut v5 = Validate::new()
        .rule("name", "require|length:2,10")
        .rule("age", "require|integer|min:18")
        .register_scene("edit", vec!["name".to_string()])
        .scene("edit"); // 激活场景
    let data4 = json!({"name": "Alice", "age": 15});
    assert!(v5.check(&data4).is_ok()); // scene edit 只验证 name
}

// ============================================================================
// I. 跨模块 DML 序列对抗（状态污染检测）
// ============================================================================

/// I1: 同一 Arc<MemorySessionStore> 共享给多个 Session 实例
#[test]
fn adversarial_cross_module_session_shared_store() {
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());

    // 两个 Session 共享同一 store
    let s1 = Session::with_shared_store("shared", Arc::clone(&store));
    let s2 = Session::with_shared_store("shared", Arc::clone(&store));

    s1.set("k1", Value::String("v1".into()));
    // s2 应能看到 s1 写入的数据
    assert_eq!(s2.get("k1"), Some(Value::String("v1".into())));

    s2.set("k2", Value::String("v2".into()));
    assert_eq!(s1.get("k2"), Some(Value::String("v2".into())));

    s1.delete("k1");
    assert!(!s2.has("k1"));

    // 不同 session_id 不应互相影响
    let store2 = MemorySessionStore::new();
    let s3 = Session::new("different_id", store2);
    assert!(!s3.has("k2"));
    assert!(!s3.has("k1"));
}

/// I2: Cache + Env 混合操作不互相污染
#[test]
fn adversarial_cross_module_cache_env() {
    let driver = MemoryCacheDriver::new();
    let cache = Cache::new();
    cache.register_default(driver);
    let env = Env::new();

    // 混合操作
    cache.set("shared_key", "cache_val", None).unwrap();
    env.set("shared_key", "env_val");

    // 不应互相影响
    assert_eq!(
        cache.get::<String>("shared_key").unwrap(),
        Some("cache_val".to_string())
    );
    assert_eq!(env.get("shared_key"), Some("env_val".to_string()));

    // 删除一个不影响另一个
    cache.delete("shared_key").unwrap();
    assert_eq!(env.get("shared_key"), Some("env_val".to_string()));
}

// ============================================================================
// J. Event 对抗性测试
// ============================================================================

/// J1: EventDispatcher DML 序列 — 注册→别名→触发→移除→重新注册
#[test]
fn adversarial_event_dml_sequence() {
    let dispatcher = EventDispatcher::new();
    dispatcher.listen("user.login", Arc::new(ClosureListener::new(|params| {
        Ok(params.get("uid").cloned().unwrap_or(Value::Null))
    })), false);
    assert!(dispatcher.has_listener("user.login"));
    assert_eq!(dispatcher.listener_count("user.login"), 1);
    let results = dispatcher.trigger("user.login", &serde_json::json!({"uid": 42}), false).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], serde_json::json!(42));
    dispatcher.bind(vec![("login".to_string(), "user.login".to_string())]);
    assert!(dispatcher.has_listener("login"));
    dispatcher.remove("user.login");
    assert!(!dispatcher.has_listener("user.login"));
    dispatcher.listen("user.login", Arc::new(ClosureListener::new(|params| {
        Ok(params.get("uid").cloned().unwrap_or(Value::Null))
    })), false);
    let results2 = dispatcher.trigger("user.login", &serde_json::json!({"uid": 99}), false).unwrap();
    assert_eq!(results2.len(), 1);
    assert_eq!(results2[0], serde_json::json!(99));
}

/// J2: EventDispatcher 边界值 — 空事件名、超长事件名、first 插入顺序
#[test]
fn adversarial_event_boundaries() {
    let dispatcher = EventDispatcher::new();
    dispatcher.listen("", Arc::new(ClosureListener::new(|_| Ok(Value::Null))), false);
    assert!(dispatcher.has_listener(""));
    dispatcher.trigger("", &Value::Null, false).unwrap();
    let long_name = "A".repeat(10 * 1024);
    dispatcher.listen(&long_name, Arc::new(ClosureListener::new(|_| Ok(Value::Null))), false);
    assert!(dispatcher.has_listener(&long_name));
    dispatcher.trigger(&long_name, &Value::Null, false).unwrap();
    let d2 = EventDispatcher::new();
    d2.listen("order", Arc::new(ClosureListener::new(|_| Ok(Value::String("first".into())))), false);
    d2.listen("order", Arc::new(ClosureListener::new(|_| Ok(Value::String("second".into())))), false);
    d2.listen("order", Arc::new(ClosureListener::new(|_| Ok(Value::String("zero".into())))), true);
    let results = d2.trigger("order", &Value::Null, false).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0], Value::String("zero".into()));
    assert_eq!(results[1], Value::String("first".into()));
    assert_eq!(results[2], Value::String("second".into()));
}

// ============================================================================
// K. Container 对抗性测试
// ============================================================================

/// K1: App DML 序列 — 创建→set_cache/set_log→覆盖→重新设置
#[test]
fn adversarial_container_dml_sequence() {
    let config = AppConfig::default();
    let app = App::new(config);
    assert!(app.cache().is_none());
    assert!(app.log().is_none());
    assert!(app.db_connection_names().is_empty());
    app.set_cache("redis");
    assert_eq!(app.cache(), Some("redis".to_string()));
    app.set_log("file_logger");
    assert_eq!(app.log(), Some("file_logger".to_string()));
    app.set_cache("memcached");
    assert_eq!(app.cache(), Some("memcached".to_string()));
    app.set_log("console_logger");
    assert_eq!(app.log(), Some("console_logger".to_string()));
    let cfg = app.config();
    assert_eq!(cfg.database.default, "mysql");
}

/// K2: App 边界值 — cache/log 超长值、特殊字符
#[test]
fn adversarial_container_boundaries() {
    let config = AppConfig::default();
    let app = App::new(config);
    let long_cache = "C".repeat(100 * 1024);
    app.set_cache(&long_cache);
    assert_eq!(app.cache(), Some(long_cache.clone()));
    assert_eq!(app.cache().unwrap().len(), 100 * 1024);
    let long_log = "L".repeat(100 * 1024);
    app.set_log(&long_log);
    assert_eq!(app.log(), Some(long_log.clone()));
    assert_eq!(app.log().unwrap().len(), 100 * 1024);
    app.set_cache("cache with spaces\tand\nnewlines");
    assert!(app.cache().unwrap().contains('\t'));
    app.set_cache("");
    assert_eq!(app.cache(), Some("".to_string()));
}

// ============================================================================
// L. Hooks 对抗性测试
// ============================================================================

/// L1: HookRegistry DML 序列 — 注册→dispatch→移除→重新注册
#[test]
fn adversarial_hooks_dml_sequence() {
    let registry = HookRegistry::new();
    let executed = Arc::new(std::sync::Mutex::new(false));
    {
        let executed = Arc::clone(&executed);
        registry.register(HookEvent::BeforeInsert, Arc::new(move |_ctx| {
            let mut guard = executed.lock().unwrap();
            *guard = true;
            Ok(())
        }));
    }
    let ctx = HookContext::new();
    registry.dispatch(HookEvent::BeforeInsert, &ctx).unwrap();
    assert!(*executed.lock().unwrap());
    registry.clear(HookEvent::BeforeInsert);
    *executed.lock().unwrap() = false;
    registry.dispatch(HookEvent::BeforeInsert, &ctx).unwrap();
    assert!(!*executed.lock().unwrap(), "移除后钩子不应再执行");
    {
        let executed = Arc::clone(&executed);
        registry.register(HookEvent::BeforeInsert, Arc::new(move |_ctx| {
            let mut guard = executed.lock().unwrap();
            *guard = true;
            Ok(())
        }));
    }
    registry.dispatch(HookEvent::BeforeInsert, &ctx).unwrap();
    assert!(*executed.lock().unwrap(), "重新注册后钩子应再次执行");
}

/// L2: HookEvent 字符串映射边界
#[test]
fn adversarial_hooks_event_name_mapping() {
    use sz_rust_core::hooks::{ALL_EVENTS, PHP_NATIVE_EVENTS, EXTENDED_EVENTS, event_name, event_from_name};
    assert_eq!(ALL_EVENTS.len(), 16);
    for event in ALL_EVENTS.iter() {
        let name = event_name(*event);
        assert!(!name.is_empty());
        let roundtrip = event_from_name(name);
        assert_eq!(roundtrip, Some(*event), "双射失败: {:?} -> {} -> {:?}", event, name, roundtrip);
    }
    assert_eq!(event_from_name(""), None);
    assert_eq!(event_from_name("BEFORE_INSERT"), None);
    assert_eq!(event_from_name("before-insert"), None);
    let long_str = "X".repeat(100 * 1024);
    assert_eq!(event_from_name(&long_str), None);
    assert_eq!(PHP_NATIVE_EVENTS.len(), 12);
    assert_eq!(EXTENDED_EVENTS.len(), 4);
    for native in PHP_NATIVE_EVENTS.iter() {
        assert!(!EXTENDED_EVENTS.contains(native), "原生事件 {:?} 不应在扩展事件列表中", native);
    }
}

// ============================================================================
// M. Router 对抗性测试
// ============================================================================

/// M1: parse_path 边界值
#[test]
fn adversarial_router_parse_path_boundaries() {
    let p = parse_path("/");
    assert_eq!(p, ParsedPath::new("index", "Index", "index"));
    let p = parse_path("");
    assert_eq!(p, ParsedPath::new("index", "Index", "index"));
    let long_segment = "a".repeat(50 * 1024);
    let long_path = format!("/{}", long_segment);
    let p = parse_path(&long_path);
    assert_eq!(p.app, "index");
    assert_eq!(p.controller.len(), 50 * 1024);
    assert_eq!(p.action, "index");
    let p = parse_path("/user/login?id=1&name=test");
    assert_eq!(p, ParsedPath::new("index", "User", "login"));
    let p = parse_path("/common/some/action");
    assert_eq!(p, ParsedPath::new("index", "Common", "some"));
    let p = parse_path("/a/b/c/d/e/f");
    assert_eq!(p, ParsedPath::new("index", "A", "b"));
}

/// M2: is_app_in_map 边界测试
#[test]
fn adversarial_router_is_app_in_map_boundaries() {
    for app in ["oapc", "admin", "api", "farm", "oapi", "cashier", "scene"] {
        assert!(is_app_in_map(app), "{} 应在 app_map 中", app);
    }
    assert!(!is_app_in_map("common"));
    assert!(!is_app_in_map(""));
    assert!(!is_app_in_map("unknown"));
    assert!(!is_app_in_map("OAPC"));
    let long = "X".repeat(100 * 1024);
    assert!(!is_app_in_map(&long));
}

// ============================================================================
// N. Upload 对抗性测试
// ============================================================================

/// N1: UploadErrCode 错误码映射边界
#[test]
fn adversarial_upload_err_code_boundaries() {
    assert_eq!(UploadErrCode::from_i32(0), UploadErrCode::Ok);
    assert_eq!(UploadErrCode::from_i32(1), UploadErrCode::IniSize);
    assert_eq!(UploadErrCode::from_i32(2), UploadErrCode::FormSize);
    assert_eq!(UploadErrCode::from_i32(3), UploadErrCode::Partial);
    assert_eq!(UploadErrCode::from_i32(4), UploadErrCode::NoFile);
    assert_eq!(UploadErrCode::from_i32(6), UploadErrCode::NoTmpDir);
    assert_eq!(UploadErrCode::from_i32(7), UploadErrCode::CantWrite);
    assert_eq!(UploadErrCode::from_i32(-1), UploadErrCode::Ok);
    assert_eq!(UploadErrCode::from_i32(999), UploadErrCode::Ok);
    assert_eq!(UploadErrCode::from_i32(i32::MAX), UploadErrCode::Ok);
    assert!(UploadErrCode::IniSize.error_message().contains("exceeds"));
    assert!(UploadErrCode::NoFile.error_message().contains("no file"));
    assert!(UploadErrCode::CantWrite.error_message().contains("write error"));
}

/// N2: HashAlgo 映射边界
#[test]
fn adversarial_upload_hash_algo_boundaries() {
    assert_eq!(HashAlgo::parse_algo("md5"), Some(HashAlgo::Md5));
    assert_eq!(HashAlgo::parse_algo("sha1"), Some(HashAlgo::Sha1));
    assert_eq!(HashAlgo::parse_algo("sha256"), Some(HashAlgo::Sha256));
    assert_eq!(HashAlgo::parse_algo("sha512"), Some(HashAlgo::Sha512));
    assert_eq!(HashAlgo::Md5.as_str(), "md5");
    assert_eq!(HashAlgo::Sha1.as_str(), "sha1");
    assert_eq!(HashAlgo::Sha256.as_str(), "sha256");
    assert_eq!(HashAlgo::Sha512.as_str(), "sha512");
    assert_eq!(HashAlgo::parse_algo(""), None);
    assert_eq!(HashAlgo::parse_algo("MD5"), None);
    let long = "X".repeat(100 * 1024);
    assert_eq!(HashAlgo::parse_algo(&long), None);
}

// ============================================================================
// O. View 对抗性测试
// ============================================================================

/// O1: View DML 序列 — assign→display→覆盖→clear_vars→重新赋值
#[test]
fn adversarial_view_dml_sequence() {
    let engine = SimpleTemplateEngine::new(ViewConfig::default());
    let view = View::new(Box::new(engine));
    assert!(!view.has_var("name"));
    view.assign("name", Value::String("World".into()));
    assert!(view.has_var("name"));
    assert_eq!(view.get_var("name"), Some(Value::String("World".into())));
    let result = view.display("Hello {$name}!", None).unwrap();
    assert_eq!(result, "Hello World!");
    view.assign("name", Value::String("Rust".into()));
    let result2 = view.display("Hello {$name}!", None).unwrap();
    assert_eq!(result2, "Hello Rust!");
    let mut vars = ViewData::new();
    vars.insert("first".to_string(), Value::String("Alice".into()));
    vars.insert("last".to_string(), Value::String("Bob".into()));
    view.assign_many(vars);
    assert!(view.has_var("first"));
    view.clear_vars();
    assert!(!view.has_var("name"));
    view.assign("new_key", Value::String("new_val".into()));
    assert_eq!(view.get_var("new_key"), Some(Value::String("new_val".into())));
}

/// O2: View 边界值 — 空模板、超大输入、特殊字符
#[test]
fn adversarial_view_boundaries() {
    let engine = SimpleTemplateEngine::new(ViewConfig::default());
    let view = View::new(Box::new(engine));
    let result = view.display("", None).unwrap();
    assert_eq!(result, "");
    let result = view.display("plain text", None).unwrap();
    assert_eq!(result, "plain text");
    let long_val = "V".repeat(100 * 1024);
    view.assign("long", Value::String(long_val.clone()));
    let result = view.display("{$long}", None).unwrap();
    assert_eq!(result.len(), 100 * 1024);
    assert_eq!(result, long_val);
    view.assign("special", Value::String("<script>alert('xss')</script>".into()));
    let result = view.display("{$special}", None).unwrap();
    assert!(!result.contains("<script>"));
    assert!(result.contains("&lt;script&gt;"));
    view.assign("user", serde_json::json!({"name": "Alice", "age": 30}));
    let result = view.display("{$user.name} is {$user.age}", None).unwrap();
    assert_eq!(result, "Alice is 30");
    let result = view.display("{$nonexistent}", None).unwrap();
    assert_eq!(result, "");
}

// ============================================================================
// P. Log 对抗性测试
// ============================================================================

/// P1: parse_level 和 LogFacade 基础操作边界
#[test]
fn adversarial_log_parse_level_boundaries() {
    let mut default_channels = HashMap::new();
    default_channels.insert(
        "default".to_string(),
        sz_rust_core::config::LogChannel {
            r#type: "file".to_string(),
            path: "logs".to_string(),
            level: "info".to_string(),
            max_files: 30,
            format: "%{message}".to_string(),
        },
    );
    let section = LogSection {
        default: "default".to_string(),
        channels: default_channels,
    };
    let facade = LogFacade::new(&section);
    assert_eq!(facade.default_channel(), "default");
    let names = facade.channel_names();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0], "default");
    facade.info("test info message");
    facade.warn("test warn message");
    facade.error("test error message");
    assert!(facade.logger().entries().len() >= 3);
    assert!(facade.channel("nonexistent").is_none());
}

/// P2: LogFacade 多通道操作边界
#[test]
fn adversarial_log_facade_channels() {
    let mut channels = HashMap::new();
    channels.insert(
        "file".to_string(),
        sz_rust_core::config::LogChannel {
            r#type: "file".to_string(),
            path: "runtime/logs".to_string(),
            level: "info".to_string(),
            max_files: 30,
            format: "[%{time}] %{message}".to_string(),
        },
    );
    channels.insert(
        "console".to_string(),
        sz_rust_core::config::LogChannel {
            r#type: "console".to_string(),
            path: String::new(),
            level: "debug".to_string(),
            max_files: 0,
            format: "%{message}".to_string(),
        },
    );
    let section = LogSection {
        default: "file".to_string(),
        channels,
    };
    let facade = LogFacade::new(&section);
    assert_eq!(facade.default_channel(), "file");
    let names = facade.channel_names();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"file".to_string()));
    assert!(names.contains(&"console".to_string()));
    assert!(facade.channel("nonexistent").is_none());
    let mut channels2 = HashMap::new();
    channels2.insert(
        "channel with spaces".to_string(),
        sz_rust_core::config::LogChannel {
            r#type: "file".to_string(),
            path: "logs".to_string(),
            level: "debug".to_string(),
            max_files: 10,
            format: "%{message}".to_string(),
        },
    );
    let section2 = LogSection {
        default: "channel with spaces".to_string(),
        channels: channels2,
    };
    let facade2 = LogFacade::new(&section2);
    assert!(facade2.channel("channel with spaces").is_some());
}

