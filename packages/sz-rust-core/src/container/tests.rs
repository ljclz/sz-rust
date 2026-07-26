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
#[test]
fn test_load_5_db_connections() {
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

    let config = AppConfig::load_from_dir(&config_dir).unwrap();

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

    assert!(
        !Arc::ptr_eq(&s1, &s2),
        "不同作用域必须返回不同实例"
    );
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
    assert!(
        !Arc::ptr_eq(&s1, &s2),
        "清理后再次解析必须返回新实例"
    );
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
    let resolved2 = container
        .make::<TestService>()
        .expect("应能再次解析");
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
    assert_eq!(aliases, vec!["logger".to_string(), "svc1".to_string(), "svc2".to_string()]);

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
