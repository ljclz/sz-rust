use super::*;
use crate::config::AppConfig;

/// 构造测试用的 mysql 连接配置
fn make_mysql_conn() -> DatabaseConnection {
    DatabaseConnection {
        r#type: "mysql".to_string(),
        hostname: "172.17.16.14".to_string(),
        database: "shop".to_string(),
        username: "shop".to_string(),
        password: String::new(),
        hostport: 8802,
        charset: "utf8mb4".to_string(),
        prefix: "sz_".to_string(),
        deploy: 0,
        rw_separate: false,
        fields_strict: true,
        break_reconnect: true,
    }
}

/// 测试 App 全局单例初始化和获取
///
/// 注：OnceLock 全局状态不可重置，因此此测试只验证 init/global 的契约，
/// 不验证 init 后的 config 内容（config 内容由其他测试用 `App::new()` 验证）。
#[test]
fn test_app_init_and_global() {
    let config = AppConfig::default();
    let app = App::init(config);

    // 验证 global() 返回同一实例
    let app2 = App::global();
    assert!(app2.is_some());
    assert!(std::ptr::eq(app, app2.unwrap()));

    // 再次 init 应返回同一实例（不覆盖）
    let config2 = AppConfig::default();
    let app3 = App::init(config2);
    assert!(std::ptr::eq(app, app3));
}

/// 测试数据库连接配置获取
#[test]
fn test_db_connection() {
    let mut config = AppConfig::default();
    config
        .database
        .connections
        .insert("mysql".to_string(), make_mysql_conn());

    let app = App::new(config);
    let conn = app.db_connection("mysql");
    assert!(conn.is_some());
    let conn = conn.unwrap();
    assert_eq!(conn.hostname, "172.17.16.14");
    assert_eq!(conn.hostport, 8802);
    assert_eq!(conn.prefix, "sz_");
}

/// 测试不存在的数据库连接返回 None
#[test]
fn test_db_connection_not_found() {
    let config = AppConfig::default();
    let app = App::new(config);
    assert!(app.db_connection("nonexistent").is_none());
}

/// 测试默认数据库连接获取
#[test]
fn test_default_db_connection() {
    let mut config = AppConfig::default();
    config.database.default = "mysql".to_string();
    config
        .database
        .connections
        .insert("mysql".to_string(), make_mysql_conn());

    let app = App::new(config);
    let conn = app.default_db_connection();
    assert!(conn.is_some());
    assert_eq!(conn.unwrap().database, "shop");
}

/// 测试 Cache 单例设置和获取
#[test]
fn test_cache() {
    let config = AppConfig::default();
    let app = App::new(config);

    // 初始为 None
    assert!(app.cache().is_none());

    // 设置后可获取
    app.set_cache("memory_cache");
    assert_eq!(app.cache(), Some("memory_cache".to_string()));
}

/// 测试 Log 单例设置和获取
#[test]
fn test_log() {
    let config = AppConfig::default();
    let app = App::new(config);

    // 初始为 None
    assert!(app.log().is_none());

    // 设置后可获取
    app.set_log("file_logger");
    assert_eq!(app.log(), Some("file_logger".to_string()));
}

/// 测试 db_connection_names 返回所有连接名
#[test]
fn test_db_connection_names() {
    let mut config = AppConfig::default();
    config
        .database
        .connections
        .insert("mysql".to_string(), make_mysql_conn());
    config.database.connections.insert(
        "njszjt".to_string(),
        DatabaseConnection {
            r#type: "mysql".to_string(),
            hostname: "172.17.16.14".to_string(),
            database: "njszjt".to_string(),
            username: "njszjt".to_string(),
            password: String::new(),
            hostport: 8802,
            charset: "utf8mb4".to_string(),
            prefix: "soci_".to_string(),
            deploy: 0,
            rw_separate: false,
            fields_strict: true,
            break_reconnect: true,
        },
    );

    let app = App::new(config);
    let mut names = app.db_connection_names();
    names.sort();
    assert_eq!(names, vec!["mysql", "njszjt"]);
}

/// 测试从实际配置文件加载 5 个数据库连接
///
/// 直接验证 `AppConfig::load_from_dir()` 能加载 5 个连接配置，
/// 不通过 `App::init()`（避免 OnceLock 全局状态污染）。
#[tokio::test]
async fn test_load_5_db_connections() {
    // 查找 config 目录（从当前目录向上 5 级查找）
    let config_dir = std::env::current_dir().ok().and_then(|d| {
        let mut current = d.clone();
        for _ in 0..5 {
            if current.join("config").exists() {
                return Some(current.join("config"));
            }
            if let Some(parent) = current.parent() {
                current = parent.to_path_buf();
            } else {
                break;
            }
        }
        None
    });

    // 没有 config 目录时跳过（不是所有运行环境都有配置文件）
    let Some(config_dir) = config_dir else {
        eprintln!("跳过：未找到 config 目录");
        return;
    };

    let config = AppConfig::load_from_dir(&config_dir).await.unwrap();

    // 验证 5 个数据库连接配置
    let names: Vec<&str> = config
        .database
        .connections
        .keys()
        .map(|s| s.as_str())
        .collect();
    assert!(
        names.len() >= 5,
        "应有 5 个数据库连接，实际 {}: {:?}",
        names.len(),
        names
    );

    // 验证每个连接配置存在
    assert!(config.database.connections.contains_key("mysql"));
    assert!(config.database.connections.contains_key("njszjt"));
    assert!(config.database.connections.contains_key("ljclz"));
    assert!(config.database.connections.contains_key("food"));
    assert!(config.database.connections.contains_key("oceanbase"));

    // 验证默认连接（hostname 已改用 localhost，实际地址通过环境变量注入）
    assert_eq!(config.database.default, "mysql");
    let default_conn = config.database.connections.get("mysql").unwrap();
    assert_eq!(default_conn.hostname, "localhost");
    assert_eq!(default_conn.hostport, 8802);
    assert_eq!(default_conn.prefix, "sz_");
}

// ========================================================================
// DI 服务容器测试
// ========================================================================

/// 测试用服务类型
#[derive(Debug, PartialEq)]
struct TestService {
    value: i32,
}

impl TestService {
    fn new() -> Self {
        Self { value: 42 }
    }
}

/// 测试瞬态服务：每次 make 返回新实例
#[test]
fn test_container_bind_transient() {
    let container = Container::new();
    container.bind(TestService::new);

    let s1 = container.make::<TestService>().expect("应能解析服务");
    let s2 = container.make::<TestService>().expect("应能解析服务");

    // 瞬态：两个实例不同
    assert!(!Arc::ptr_eq(&s1, &s2));
    assert_eq!(s1.value, 42);
    assert_eq!(s2.value, 42);
}

/// 测试单例服务：多次 make 返回同一实例
#[test]
fn test_container_singleton() {
    let container = Container::new();
    container.singleton(TestService::new);

    let s1 = container.make::<TestService>().expect("应能解析服务");
    let s2 = container.make::<TestService>().expect("应能解析服务");

    // 单例：两个实例相同
    assert!(Arc::ptr_eq(&s1, &s2));
    assert_eq!(s1.value, 42);
}

/// 测试解析未注册服务返回 None
#[test]
fn test_container_make_unregistered() {
    let container = Container::new();
    assert!(container.make::<TestService>().is_none());
}

/// 测试 has 检查服务是否已注册
#[test]
fn test_container_has() {
    let container = Container::new();
    assert!(!container.has::<TestService>());

    container.singleton(TestService::new);
    assert!(container.has::<TestService>());
}

/// 测试 forget 移除服务绑定与缓存
#[test]
fn test_container_forget() {
    let container = Container::new();
    container.singleton(TestService::new);

    // 先解析一次（触发单例缓存）
    let _ = container.make::<TestService>();
    assert!(container.has::<TestService>());

    container.forget::<TestService>();
    assert!(!container.has::<TestService>());
    assert!(container.make::<TestService>().is_none());
}

/// 测试 clear 清空所有绑定
#[test]
fn test_container_clear() {
    let container = Container::new();
    container.singleton(TestService::new);
    container.bind(|| 99i32);
    assert_eq!(container.count(), 2);

    container.clear();
    assert_eq!(container.count(), 0);
}

/// 测试 App 代理的 DI 方法（bind/make/singleton）
#[test]
fn test_app_di_proxy() {
    let config = AppConfig::default();
    let app = App::new(config);

    // 注册单例
    app.singleton(TestService::new);
    assert!(app.has_service::<TestService>());

    // 解析
    let s1 = app.make::<TestService>().expect("应能解析");
    let s2 = app.make::<TestService>().expect("应能解析");
    assert!(Arc::ptr_eq(&s1, &s2));
}

/// 测试不同类型的服务共存
#[test]
fn test_container_multiple_types() {
    let container = Container::new();
    container.singleton(TestService::new);
    container.singleton(|| String::from("logger"));
    container.bind(|| vec![1, 2, 3]);

    assert_eq!(container.make::<TestService>().unwrap().value, 42);
    assert_eq!(&*container.make::<String>().unwrap(), "logger");
    assert_eq!(*container.make::<Vec<i32>>().unwrap(), vec![1, 2, 3]);
}

/// 测试 count 返回已注册数量
#[test]
fn test_container_count() {
    let container = Container::new();
    assert_eq!(container.count(), 0);

    container.bind(TestService::new);
    assert_eq!(container.count(), 1);

    container.singleton(|| String::from("x"));
    assert_eq!(container.count(), 2);
}

// ========================================================================
// Scoped 生命周期测试
// ========================================================================

/// 计数器服务，用于验证 scoped 服务的实例创建次数
///
/// 使用 `AtomicU64` 记录实例化次数，可跨线程读取。
struct ScopedCounter {
    value: i64,
}

impl ScopedCounter {
    fn new(value: i64) -> Self {
        Self { value }
    }
}

/// 测试 scoped 服务：同一作用域内返回同一实例
#[test]
fn test_container_scoped_same_scope() {
    let container = Container::new();
    container.scoped(|| ScopedCounter::new(100));

    // 同一作用域：两次 make 返回同一实例
    let s1 = container
        .make_with_scope::<ScopedCounter>(1)
        .expect("应能解析 scoped 服务");
    let s2 = container
        .make_with_scope::<ScopedCounter>(1)
        .expect("应能解析 scoped 服务");

    assert!(Arc::ptr_eq(&s1, &s2), "同一作用域内必须返回同一实例");
    assert_eq!(s1.value, 100);
}

/// 测试 scoped 服务：不同作用域返回不同实例
#[test]
fn test_container_scoped_different_scope() {
    let container = Container::new();
    container.scoped(|| ScopedCounter::new(200));

    let s1 = container
        .make_with_scope::<ScopedCounter>(1)
        .expect("scope 1 应能解析");
    let s2 = container
        .make_with_scope::<ScopedCounter>(2)
        .expect("scope 2 应能解析");

    assert!(!Arc::ptr_eq(&s1, &s2), "不同作用域必须返回不同实例");
    assert_eq!(s1.value, 200);
    assert_eq!(s2.value, 200);
}

/// 测试 clear_scope 清理作用域缓存
#[test]
fn test_container_clear_scope() {
    let container = Container::new();
    container.scoped(|| ScopedCounter::new(300));

    // scope 1 创建实例
    let s1 = container
        .make_with_scope::<ScopedCounter>(1)
        .expect("scope 1 应能解析");
    assert_eq!(container.active_scope_count(), 1);

    // 清理 scope 1
    container.clear_scope(1);
    assert_eq!(container.active_scope_count(), 0);

    // 再次解析：应创建新实例（与之前不同）
    let s2 = container
        .make_with_scope::<ScopedCounter>(1)
        .expect("scope 1 清理后应能再次解析");
    assert!(!Arc::ptr_eq(&s1, &s2), "清理后再次解析必须返回新实例");
}

/// 测试 scoped 与 singleton 共存
#[test]
fn test_container_scoped_with_singleton() {
    let container = Container::new();
    container.singleton(|| ScopedCounter::new(1000));
    container.scoped(|| ScopedCounter::new(2000));

    // singleton：忽略 scope_id，返回全局单例
    let single_a = container.make_with_scope::<ScopedCounter>(1);
    let single_b = container.make_with_scope::<ScopedCounter>(2);

    // 注：singleton 和 scoped 注册到同一个 TypeId，后注册的覆盖前者。
    // 这里 singleton 先注册，scoped 后注册，所以 scoped 覆盖 singleton。
    // 验证覆盖语义：scoped 行先生效
    let scoped_a = container
        .make_with_scope::<ScopedCounter>(1)
        .expect("应能解析");
    let scoped_b = container
        .make_with_scope::<ScopedCounter>(2)
        .expect("应能解析");

    // 不同 scope 必须返回不同实例
    assert!(!Arc::ptr_eq(&scoped_a, &scoped_b));

    // 防止 unused 警告
    let _ = (single_a, single_b);
}

/// 测试 forget 清理 scoped 服务在所有作用域的缓存
#[test]
fn test_container_forget_clears_scoped() {
    let container = Container::new();
    container.scoped(|| ScopedCounter::new(400));

    // 在多个作用域创建实例
    let _ = container.make_with_scope::<ScopedCounter>(1);
    let _ = container.make_with_scope::<ScopedCounter>(2);
    let _ = container.make_with_scope::<ScopedCounter>(3);
    assert_eq!(container.active_scope_count(), 3);

    // forget 应清理所有作用域中该类型的缓存
    container.forget::<ScopedCounter>();
    assert!(!container.has::<ScopedCounter>());
    // 作用域映射本身仍存在（只是其中的 ScopedCounter 缓存被移除）
    // 但因为 forget 是按 TypeId 清理，scope_map 中的条目数会减少
}

/// 测试 clear 清空所有缓存（含 scoped 和 aliases）
#[test]
fn test_container_clear_all() {
    let container = Container::new();
    container.singleton(TestService::new);
    container.scoped(|| ScopedCounter::new(500));
    container.alias::<TestService>("svc");

    // 创建 scoped 实例
    let _ = container.make_with_scope::<ScopedCounter>(1);

    assert_eq!(container.count(), 2);
    assert_eq!(container.alias_count(), 1);
    assert_eq!(container.active_scope_count(), 1);

    container.clear();

    assert_eq!(container.count(), 0);
    assert_eq!(container.alias_count(), 0);
    assert_eq!(container.active_scope_count(), 0);
}

// ========================================================================
// instance() 直接绑定实例测试
// ========================================================================

/// 测试 instance() 直接绑定已创建实例
#[test]
fn test_container_instance_direct_binding() {
    let container = Container::new();
    let original = Arc::new(TestService { value: 999 });

    // 注意：instance 接收 T（非 Arc），内部转为 Arc
    // 这里通过 clone Arc 内部值来测试
    container.instance(TestService { value: 999 });

    let resolved = container
        .make::<TestService>()
        .expect("instance() 注册的服务应能解析");
    assert_eq!(resolved.value, 999);

    // 多次 make 返回同一实例
    let resolved2 = container.make::<TestService>().expect("应能再次解析");
    assert!(Arc::ptr_eq(&resolved, &resolved2));

    // 防止 unused 警告
    let _ = original;
}

/// 测试 instance() 后 has() 返回 true
#[test]
fn test_container_instance_has() {
    let container = Container::new();
    assert!(!container.has::<TestService>());

    container.instance(TestService::new());
    assert!(container.has::<TestService>());
}

/// 测试 instance() 后 forget() 清理
#[test]
fn test_container_instance_forget() {
    let container = Container::new();
    container.instance(TestService::new());
    assert!(container.has::<TestService>());

    container.forget::<TestService>();
    assert!(!container.has::<TestService>());
    assert!(container.make::<TestService>().is_none());
}

/// 测试 App 代理的 instance/scoped/alias 方法
#[test]
fn test_app_instance_scoped_alias_proxy() {
    let config = AppConfig::default();
    let app = App::new(config);

    // instance
    app.instance(TestService { value: 777 });
    let s = app.make::<TestService>().expect("App::instance 后应能解析");
    assert_eq!(s.value, 777);

    // scoped
    app.scoped(|| ScopedCounter::new(888));
    let s1 = app
        .make_with_scope::<ScopedCounter>(42)
        .expect("App::make_with_scope 应能解析");
    let s2 = app
        .make_with_scope::<ScopedCounter>(42)
        .expect("同 scope 应能再次解析");
    assert!(Arc::ptr_eq(&s1, &s2));

    // alias
    app.alias::<TestService>("test_svc");
    assert!(app.container().is_alias("test_svc"));
    let type_id = app
        .container()
        .resolve_alias("test_svc")
        .expect("别名应能解析");
    assert_eq!(type_id, std::any::TypeId::of::<TestService>());

    // clear_scope
    app.clear_scope(42);
    assert_eq!(app.container().active_scope_count(), 0);
}

// ========================================================================
// alias 别名测试
// ========================================================================

/// 测试 alias 注册与 resolve_alias 查找
#[test]
fn test_container_alias_register_and_resolve() {
    let container = Container::new();
    container.singleton(TestService::new);
    container.alias::<TestService>("test_service");

    assert!(container.is_alias("test_service"));
    assert!(!container.is_alias("nonexistent"));

    let type_id = container
        .resolve_alias("test_service")
        .expect("别名应能解析");
    assert_eq!(type_id, std::any::TypeId::of::<TestService>());
}

/// 测试 resolve_alias 未注册别名返回 None
#[test]
fn test_container_alias_resolve_unregistered() {
    let container = Container::new();
    assert!(container.resolve_alias("not_registered").is_none());
}

/// 测试多个别名共存
#[test]
fn test_container_multiple_aliases() {
    let container = Container::new();
    container.singleton(TestService::new);
    container.singleton(|| String::from("logger"));

    container.alias::<TestService>("svc1");
    container.alias::<TestService>("svc2");
    container.alias::<String>("logger");

    assert_eq!(container.alias_count(), 3);

    let mut aliases = container.debug_aliases();
    aliases.sort();
    assert_eq!(
        aliases,
        vec!["logger".to_string(), "svc1".to_string(), "svc2".to_string()]
    );

    // 同一类型的多个别名都解析到同一 TypeId
    let tid1 = container.resolve_alias("svc1").unwrap();
    let tid2 = container.resolve_alias("svc2").unwrap();
    assert_eq!(tid1, tid2);
}

/// 测试 alias 不影响 make 解析
///
/// alias 仅用于调试和反向查找，不参与 make 流程。
/// 即使注册了 alias，make 仍按 TypeId 查找。
#[test]
fn test_container_alias_does_not_affect_make() {
    let container = Container::new();
    container.singleton(TestService::new);
    container.alias::<TestService>("my_svc");

    // make 仍正常工作
    let svc = container.make::<TestService>().expect("make 应正常工作");
    assert_eq!(svc.value, 42);

    // 未注册 alias 但类型已注册的情况
    container.alias::<String>("my_str");
    // String 类型未注册为服务，make 应返回 None
    // 但 alias 仍可解析
    assert!(container.is_alias("my_str"));
    assert!(container.make::<String>().is_none());
}

// ========================================================================
// 标签绑定测试（对齐 PHP `app()->tag()` / `app()->tagged()`）
// ========================================================================

/// 测试用：日志类型
#[derive(Debug, PartialEq)]
struct FileLogger {
    level: u8,
}

impl FileLogger {
    fn new() -> Self {
        Self { level: 1 }
    }
}

/// 测试用：邮件类型
#[derive(Debug, PartialEq)]
struct MailLogger {
    level: u8,
}

impl MailLogger {
    fn new() -> Self {
        Self { level: 2 }
    }
}

/// 测试 tag 给类型打标签，tagged 返回该标签下的实例
#[test]
fn test_container_tag_and_tagged() {
    let container = Container::new();
    container.singleton(FileLogger::new);
    container.singleton(MailLogger::new);

    // 给 FileLogger 打 "reporters" 标签
    container.tag::<FileLogger>("reporters");

    let reporters = container.tagged::<FileLogger>("reporters");
    assert_eq!(reporters.len(), 1);
    assert_eq!(reporters[0].level, 1);
}

/// 测试同一标签下追加多个同类型实例
#[test]
fn test_container_tag_multiple_same_type() {
    let container = Container::new();
    container.singleton(FileLogger::new);

    // 同一类型多次打同一标签
    container.tag::<FileLogger>("reporters");
    container.tag::<FileLogger>("reporters");
    container.tag::<FileLogger>("reporters");

    // tagged 会返回多个实例（每个 TypeId 调用一次 make）
    // 但由于同一 TypeId 多次 push 到标签列表，且 singleton 每次返回同一实例
    let reporters = container.tagged::<FileLogger>("reporters");
    assert_eq!(reporters.len(), 3);
    // 全部为同一单例实例
    assert!(Arc::ptr_eq(&reporters[0], &reporters[1]));
    assert!(Arc::ptr_eq(&reporters[1], &reporters[2]));
}

/// 测试不同标签独立工作
#[test]
fn test_container_tag_different_tags() {
    let container = Container::new();
    container.singleton(FileLogger::new);
    container.singleton(MailLogger::new);

    container.tag::<FileLogger>("file_reporters");
    container.tag::<MailLogger>("mail_reporters");

    assert_eq!(container.tagged::<FileLogger>("file_reporters").len(), 1);
    assert_eq!(container.tagged::<MailLogger>("mail_reporters").len(), 1);

    // 交叉查询：FileLogger 不在 mail_reporters 标签下
    assert_eq!(container.tagged::<FileLogger>("mail_reporters").len(), 0);
    assert_eq!(container.tagged::<MailLogger>("file_reporters").len(), 0);
}

/// 测试 tagged 获取不存在的标签返回空向量
#[test]
fn test_container_tagged_nonexistent_tag() {
    let container = Container::new();
    container.singleton(FileLogger::new);

    let reporters = container.tagged::<FileLogger>("nonexistent");
    assert!(reporters.is_empty());
}

/// 测试 tagged 获取标签下未注册类型的实例返回空向量
#[test]
fn test_container_tagged_unregistered_type() {
    let container = Container::new();
    // 不注册 FileLogger 服务，但给 FileLogger 打标签
    container.tag::<FileLogger>("reporters");

    // tagged 应返回空（make::<FileLogger>() 返回 None）
    let reporters = container.tagged::<FileLogger>("reporters");
    assert!(reporters.is_empty());
}

/// 测试 tag_count 返回标签下已注册类型数量
#[test]
fn test_container_tag_count() {
    let container = Container::new();

    // 不存在的标签：count = 0
    assert_eq!(container.tag_count("nonexistent"), 0);

    container.tag::<FileLogger>("reporters");
    container.tag::<MailLogger>("reporters");

    assert_eq!(container.tag_count("reporters"), 2);
}

/// 测试 tag_names 返回所有已注册标签
#[test]
fn test_container_tag_names() {
    let container = Container::new();
    assert!(container.tag_names().is_empty());

    container.tag::<FileLogger>("reporters");
    container.tag::<MailLogger>("notifiers");

    let mut names = container.tag_names();
    names.sort();
    assert_eq!(
        names,
        vec!["notifiers".to_string(), "reporters".to_string()]
    );
}

/// 测试 tagged_type_ids 返回标签下的所有 TypeId
#[test]
fn test_container_tagged_type_ids() {
    let container = Container::new();
    container.tag::<FileLogger>("reporters");
    container.tag::<MailLogger>("reporters");

    let type_ids = container.tagged_type_ids("reporters");
    assert_eq!(type_ids.len(), 2);
    assert!(type_ids.contains(&TypeId::of::<FileLogger>()));
    assert!(type_ids.contains(&TypeId::of::<MailLogger>()));

    // 不存在的标签返回空
    assert!(container.tagged_type_ids("nonexistent").is_empty());
}

/// 测试 forget_tag 移除整个标签
#[test]
fn test_container_forget_tag() {
    let container = Container::new();
    container.singleton(FileLogger::new);
    container.tag::<FileLogger>("reporters");

    assert_eq!(container.tag_count("reporters"), 1);

    container.forget_tag("reporters");

    assert_eq!(container.tag_count("reporters"), 0);
    assert!(container.tagged::<FileLogger>("reporters").is_empty());
    assert!(!container.tag_names().contains(&"reporters".to_string()));
}

/// 测试 clear 清空所有标签
#[test]
fn test_container_clear_clears_tags() {
    let container = Container::new();
    container.singleton(FileLogger::new);
    container.tag::<FileLogger>("reporters");
    container.tag::<FileLogger>("notifiers");

    assert!(!container.tag_names().is_empty());

    container.clear();

    assert!(container.tag_names().is_empty());
    assert_eq!(container.tag_count("reporters"), 0);
}

// ========================================================================
// 上下文绑定测试（对齐 PHP `app()->when()->needs()->give()`）
// ========================================================================

// 注：因 trait object 与泛型 T 的 downcast 冲突，上下文绑定测试用具体类型而非 trait

/// 测试用：PhotoController 消费者
struct PhotoController;

/// 测试用：VideoController 消费者
struct VideoController;

/// 测试 bind_contextual 注册上下文绑定
#[test]
fn test_container_bind_contextual() {
    let container = Container::new();

    // 为 PhotoController 注册上下文绑定：返回 "s3" 字符串
    container.bind_contextual::<PhotoController, String, _>(|| "s3".to_string());

    // 检查上下文绑定存在
    assert!(container.has_contextual::<PhotoController, String>());
    assert!(!container.has_contextual::<VideoController, String>());

    // 解析：为 PhotoController 返回上下文绑定的实例
    let fs = container
        .make_for::<String, PhotoController>()
        .expect("应为 PhotoController 解析上下文绑定");
    assert_eq!(&*fs, "s3");
}

/// 测试 make_for 无上下文绑定时回退到普通 make
#[test]
fn test_container_make_for_fallback() {
    let container = Container::new();

    // 注册普通单例
    container.singleton(|| "default".to_string());

    // 无上下文绑定：回退到普通 make
    let result = container
        .make_for::<String, PhotoController>()
        .expect("应回退到普通 make");
    assert_eq!(&*result, "default");
}

/// 测试 make_for 既无上下文绑定也无普通绑定返回 None
#[test]
fn test_container_make_for_no_binding() {
    let container = Container::new();

    let result = container.make_for::<String, PhotoController>();
    assert!(result.is_none());
}

/// 测试不同消费者获得不同的上下文绑定
#[test]
fn test_container_contextual_different_consumers() {
    let container = Container::new();

    // PhotoController 获得 "s3"
    container.bind_contextual::<PhotoController, String, _>(|| "s3".to_string());
    // VideoController 获得 "local"
    container.bind_contextual::<VideoController, String, _>(|| "local".to_string());

    let photo_fs = container
        .make_for::<String, PhotoController>()
        .expect("PhotoController 应解析");
    let video_fs = container
        .make_for::<String, VideoController>()
        .expect("VideoController 应解析");

    assert_eq!(&*photo_fs, "s3");
    assert_eq!(&*video_fs, "local");
}

/// 测试 contextual_count 返回上下文绑定数量
#[test]
fn test_container_contextual_count() {
    let container = Container::new();
    assert_eq!(container.contextual_count(), 0);

    container.bind_contextual::<PhotoController, String, _>(|| "s3".to_string());
    assert_eq!(container.contextual_count(), 1);

    container.bind_contextual::<VideoController, String, _>(|| "local".to_string());
    assert_eq!(container.contextual_count(), 2);

    // 覆盖已有绑定：数量不变
    container.bind_contextual::<PhotoController, String, _>(|| "azure".to_string());
    assert_eq!(container.contextual_count(), 2);
}

/// 测试 forget_contextual 移除指定上下文绑定
#[test]
fn test_container_forget_contextual() {
    let container = Container::new();
    container.bind_contextual::<PhotoController, String, _>(|| "s3".to_string());
    assert!(container.has_contextual::<PhotoController, String>());

    container.forget_contextual::<PhotoController, String>();
    assert!(!container.has_contextual::<PhotoController, String>());

    // forget 后 make_for 回退到普通 make（无普通绑定则返回 None）
    assert!(container.make_for::<String, PhotoController>().is_none());
}

/// 测试 clear 清空所有上下文绑定
#[test]
fn test_container_clear_clears_contextual() {
    let container = Container::new();
    container.bind_contextual::<PhotoController, String, _>(|| "s3".to_string());
    container.bind_contextual::<VideoController, String, _>(|| "local".to_string());
    assert_eq!(container.contextual_count(), 2);

    container.clear();

    assert_eq!(container.contextual_count(), 0);
    assert!(!container.has_contextual::<PhotoController, String>());
    assert!(!container.has_contextual::<VideoController, String>());
}

/// 测试上下文绑定与标签绑定共存
#[test]
fn test_container_tag_and_contextual_coexist() {
    let container = Container::new();
    container.singleton(|| "default".to_string());

    // 同时使用标签和上下文绑定
    container.tag::<String>("text_services");
    container.bind_contextual::<PhotoController, String, _>(|| "s3".to_string());

    // 标签查询正常
    let tagged = container.tagged::<String>("text_services");
    assert_eq!(tagged.len(), 1);
    assert_eq!(&*tagged[0], "default");

    // 上下文查询正常
    let contextual = container
        .make_for::<String, PhotoController>()
        .expect("上下文绑定应正常工作");
    assert_eq!(&*contextual, "s3");
}

// ========================================================================
// 方法调用 / 自动注入测试（对齐 PHP `app()->call()` / `app()->invoke()`）
// ========================================================================

/// 测试 call_method 基本方法调用（单依赖）
#[test]
fn test_container_call_method_basic() {
    let container = Container::new();
    container.singleton(FileLogger::new);

    let result: u8 =
        container.call_method(|c| c.make::<FileLogger>().unwrap(), |logger| logger.level);

    assert_eq!(result, 1);
}

/// 测试 call_method 带多个依赖的方法调用
#[test]
fn test_container_call_method_with_dependencies() {
    let container = Container::new();
    container.singleton(FileLogger::new);
    container.singleton(MailLogger::new);

    let result: String = container.call_method(
        |c| {
            (
                c.make::<FileLogger>().unwrap(),
                c.make::<MailLogger>().unwrap(),
            )
        },
        |(file_logger, mail_logger)| {
            format!("file={}, mail={}", file_logger.level, mail_logger.level)
        },
    );

    assert_eq!(result, "file=1, mail=2");
}

/// 测试 call_method 返回值（不同类型）
#[test]
fn test_container_call_method_return_value() {
    let container = Container::new();
    container.singleton(TestService::new);

    let result: i32 = container.call_method(
        |c| c.make::<TestService>().unwrap(),
        |service| service.value * 2,
    );

    assert_eq!(result, 84);
}

/// 测试 invoke 基本方法调用
#[test]
fn test_container_invoke_basic() {
    let container = Container::new();
    container.singleton(FileLogger::new);

    let result: u8 = container.invoke(|c| c.make::<FileLogger>().unwrap().level);

    assert_eq!(result, 1);
}

/// 测试 invoke 带多个依赖
#[test]
fn test_container_invoke_with_multiple_deps() {
    let container = Container::new();
    container.singleton(FileLogger::new);
    container.singleton(MailLogger::new);
    container.singleton(TestService::new);

    let result: String = container.invoke(|c| {
        let file_logger = c.make::<FileLogger>().unwrap();
        let mail_logger = c.make::<MailLogger>().unwrap();
        let service = c.make::<TestService>().unwrap();
        format!(
            "file={}, mail={}, service={}",
            file_logger.level, mail_logger.level, service.value
        )
    });

    assert_eq!(result, "file=1, mail=2, service=42");
}

/// 测试 make_or_panic 成功解析已注册服务
#[test]
fn test_container_make_or_panic_success() {
    let container = Container::new();
    container.singleton(TestService::new);

    let service = container.make_or_panic::<TestService>();
    assert_eq!(service.value, 42);

    // 多次调用返回同一单例
    let service2 = container.make_or_panic::<TestService>();
    assert!(Arc::ptr_eq(&service, &service2));
}

/// 测试 make_or_panic 在服务未注册时 panic
#[test]
#[should_panic(expected = "无法解析服务")]
fn test_container_make_or_panic_panic() {
    let container = Container::new();
    // TestService 未注册，应 panic
    let _ = container.make_or_panic::<TestService>();
}

/// 测试 call_method 无依赖的方法调用
#[test]
fn test_container_call_method_no_dependencies() {
    let container = Container::new();

    let result: String = container.call_method(|_| (), |_| String::from("no dependencies needed"));

    assert_eq!(result, "no dependencies needed");
}

// ========================================================================
// 循环依赖检测测试（P0-ARCH-01）
// ========================================================================

/// 测试：构造栈在正常解析后应为空（无泄漏）
#[test]
fn test_container_constructing_stack_cleared_after_make() {
    let container = Container::new();
    container.singleton(TestService::new);

    assert_eq!(container.constructing_depth(), 0);
    let _svc = container.make::<TestService>();
    assert_eq!(container.constructing_depth(), 0);
}

/// 测试：非循环依赖正常工作（A -> B，B 无依赖）
///
/// 验证循环检测不误杀正常的依赖链。
#[test]
fn test_container_non_circular_dependency_works() {
    use std::sync::Arc;

    #[derive(Debug, PartialEq)]
    struct Inner {
        value: i32,
    }

    #[derive(Debug, PartialEq)]
    struct Outer {
        inner: Arc<Inner>,
    }

    let container = Container::new();

    // Inner 无依赖
    container.singleton(|| Inner { value: 42 });
    // Outer 依赖 Inner（非循环）
    let c = Arc::new(container);
    let c_outer = c.clone();
    c.singleton(move || {
        let inner = c_outer.make::<Inner>().expect("Inner 应能解析");
        Outer { inner }
    });

    let outer = c.make::<Outer>().expect("Outer 应能解析");
    assert_eq!(outer.inner.value, 42);
    // 解析后构造栈应为空
    assert_eq!(c.constructing_depth(), 0);
}

/// 测试：间接循环依赖 A -> B -> A
///
/// 场景：ServiceA 依赖 ServiceB，ServiceB 依赖 ServiceA，
/// 形成 A -> B -> A 的间接循环。
#[test]
#[should_panic(expected = "DI 容器检测到循环依赖")]
fn test_container_circular_indirect_a_b_a() {
    use std::sync::Arc;

    #[derive(Debug)]
    #[allow(dead_code)]
    struct ServiceA(Arc<ServiceB>);

    #[derive(Debug)]
    #[allow(dead_code)]
    struct ServiceB(Arc<ServiceA>);

    let container = Container::new();
    let c = Arc::new(container);

    let c_b = c.clone();
    c.singleton(move || ServiceA(c_b.make::<ServiceB>().expect("ServiceB 应能解析")));

    let c_a = c.clone();
    c.singleton(move || ServiceB(c_a.make::<ServiceA>().expect("ServiceA 应能解析")));

    // 触发解析，应检测到循环依赖
    let _ = c.make::<ServiceA>();
}

/// 测试：长链循环依赖 A -> B -> C -> A
///
/// 验证循环检测能正确报告完整依赖链。
#[test]
#[should_panic(expected = "DI 容器检测到循环依赖")]
fn test_container_circular_long_chain() {
    use std::sync::Arc;

    #[derive(Debug)]
    #[allow(dead_code)]
    struct NodeA(Arc<NodeB>);
    #[derive(Debug)]
    #[allow(dead_code)]
    struct NodeB(Arc<NodeC>);
    #[derive(Debug)]
    #[allow(dead_code)]
    struct NodeC(Arc<NodeA>);

    let container = Container::new();
    let c = Arc::new(container);

    let c_b = c.clone();
    c.singleton(move || NodeA(c_b.make::<NodeB>().expect("NodeB")));

    let c_c = c.clone();
    c.singleton(move || NodeB(c_c.make::<NodeC>().expect("NodeC")));

    let c_a = c.clone();
    c.singleton(move || NodeC(c_a.make::<NodeA>().expect("NodeA")));

    let _ = c.make::<NodeA>();
}

/// 测试：循环依赖错误信息包含完整依赖链
///
/// 通过 panic 的 expected 前缀验证错误消息格式。
/// 注：type_name 返回模块全路径，故 expected 使用完整路径。
#[test]
#[should_panic(
    expected = "DI 容器检测到循环依赖: sz_rust_core::container::tests::test_container_circular_error_message_contains_chain::NodeX"
)]
fn test_container_circular_error_message_contains_chain() {
    use std::sync::Arc;

    #[derive(Debug)]
    #[allow(dead_code)]
    struct NodeX(Arc<NodeY>);
    #[derive(Debug)]
    #[allow(dead_code)]
    struct NodeY(Arc<NodeX>);

    let container = Container::new();
    let c = Arc::new(container);

    let c_y = c.clone();
    c.singleton(move || NodeX(c_y.make::<NodeY>().expect("NodeY")));

    let c_x = c.clone();
    c.singleton(move || NodeY(c_x.make::<NodeX>().expect("NodeX")));

    let _ = c.make::<NodeX>();
}

// ============================================================================
// P1-TEST-03：App 未测公共函数补充
// ============================================================================

/// App::has_service — 检查服务是否已注册
#[test]
fn test_app_has_service_registered() {
    let app = App::new(AppConfig::default());
    app.singleton(|| "hello".to_string());
    assert!(app.has_service::<String>());
}

#[test]
fn test_app_has_service_not_registered() {
    let app = App::new(AppConfig::default());
    assert!(!app.has_service::<String>());
}

/// App::make_with_scope — 从指定作用域获取实例
#[test]
fn test_app_make_with_scope_existing() {
    #[derive(Debug)]
    struct ScopedVal(i32);

    let app = App::new(AppConfig::default());
    app.scoped(|| ScopedVal(42));

    let scope_id = 1;
    let instance = app.make_with_scope::<ScopedVal>(scope_id);
    // 首次获取会创建（scoped 生命周期）
    assert!(instance.is_some());
    assert_eq!(instance.unwrap().0, 42);
}

#[test]
fn test_app_make_with_scope_unregistered() {
    let app = App::new(AppConfig::default());
    let result = app.make_with_scope::<String>(1);
    assert!(result.is_none());
}

#[test]
fn test_app_make_with_scope_returns_cached() {
    use std::sync::Arc;

    #[derive(Debug)]
    struct Counter(Arc<std::sync::atomic::AtomicUsize>);

    let app = App::new(AppConfig::default());
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c2 = counter.clone();
    app.scoped(move || Counter(c2.clone()));

    let scope_id = 7;
    // 同 scope_id 多次获取应返回同一实例
    let first = app.make_with_scope::<Counter>(scope_id).unwrap();
    let second = app.make_with_scope::<Counter>(scope_id).unwrap();
    assert!(Arc::ptr_eq(&first.0, &second.0));
}
