# ADR-010：配置加载方式（serde + YAML + 环境变量覆盖 + 默认值）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-009（缓存策略）
> **相关代码**：`packages/sz-rust-core/src/config.rs`

## 背景

PHP 端配置通过 `config/*.php` 文件加载，每个配置文件返回一个数组：

```php
// config/app.php
return [
    'app_host' => '',
    'app_namespace' => 'app',
    'with_route' => true,
    'auto_multi_app' => true,
    'default_app' => 'index',
    'app_map' => [],
    'deny_app_list' => ['common'],
    'default_timezone' => 'Asia/Shanghai',
];
```

PHP 配置特点：
- 每个配置文件是独立的 PHP 文件，返回数组
- 环境变量通过 `.env` 文件覆盖（`think\facade\Env`）
- 配置项有隐式默认值（PHP 数组访问不存在的 key 返回 null）

sz-rust 需要决定如何对齐 PHP 配置加载，同时利用 Rust 的类型系统优势。

## 决策

采用 **serde + YAML + 环境变量覆盖 + 默认值** 策略：

### 1. YAML 格式（而非 PHP 文件）

```yaml
# config/app.yaml
app:
  app_host: ""
  app_namespace: "app"
  with_route: true
  auto_multi_app: true
  default_app: "index"
  default_timezone: "Asia/Shanghai"
  app_map: {}
  deny_app_list: ["common"]

database:
  mysql:
    host: "127.0.0.1"
    port: 3306
    database: "ljclz"
    username: "root"
    password: ""

cache:
  default_store: "memory"
  stores:
    memory:
      driver: "memory"
    redis:
      driver: "redis"
      host: "127.0.0.1"
      port: 6379
```

选择 YAML 而非 PHP 文件的原因：
- Rust 无法执行 PHP 文件
- YAML 是 Rust 生态中配置文件的事实标准（serde_yaml）
- YAML 比 JSON 更适合人类编写（支持注释、多行字符串）

### 2. 环境变量覆盖

| 格式 | 示例 | 说明 |
|------|------|------|
| `SZ_{SECTION}__{KEY}` | `SZ_APP__DEFAULT_APP=api` | 标准格式，双下划线分隔层级 |
| `SZ_DB_{CONN}_PASSWORD` | `SZ_DB_MYSQL_PASSWORD=xxx` | 数据库密码简写格式 |

环境变量覆盖 YAML 配置，优先级最高。

### 3. 默认值（serde `#[serde(default)]`）

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AppSection {
    #[serde(default)]
    pub app_host: String,
    #[serde(default = "default_true")]
    pub with_route: bool,
    #[serde(default = "default_default_app")]
    pub default_app: String,
    #[serde(default = "default_app_map")]
    pub app_map: HashMap<String, String>,
    #[serde(default = "default_deny_app_list")]
    pub deny_app_list: Vec<String>,
}
```

所有配置项都有默认值，即使 YAML 文件缺失或字段缺失也能正常加载。对齐 PHP 数组访问不存在的 key 返回 null 的行为（Rust 用默认值替代 null）。

### 4. 配置结构（5 个 section）

```rust
pub struct AppConfig {
    pub app: AppSection,           // 对齐 config/app.php
    pub database: DatabaseSection, // 对齐 config/database.php
    pub cache: CacheSection,       // 对齐 config/cache.php
    pub addons: AddonsSection,     // 对齐 config/addons.php
    pub log: LogSection,           // 对齐 config/log.php
}
```

### 5. 类型安全

```rust
// PHP: $config['app']['default_app'] → string|null（运行时错误）
// Rust: config.app.default_app → String（编译期类型检查）
```

Rust 的强类型系统确保配置项类型正确，避免 PHP 的运行时类型错误。

## 后果

### 正面后果

- **类型安全**：所有配置项都有明确的类型，编译期检查
- **默认值保证**：即使 YAML 文件缺失，配置也能正常加载
- **环境变量覆盖**：生产环境敏感配置（密码、密钥）通过环境变量注入，不写入 YAML
- **PHP 对齐**：5 个 section 对齐 PHP 的 5 个配置文件
- **serde 生态**：利用 serde 的反序列化能力，支持复杂嵌套结构

### 负面后果

- **YAML 与 PHP 配置不兼容**：迁移时需要手动将 PHP 配置文件转换为 YAML
- **环境变量格式复杂**：`SZ_{SECTION}__{KEY}` 格式需要双下划线分隔层级，容易写错
- **默认值分散**：每个字段的默认值通过 `#[serde(default = "fn")]` 定义，分散在结构体定义中
- **不支持 PHP 的 `config()` 函数**：PHP 端 `config('app.default_app')` 的点号访问语法在 Rust 端无直接等价物

## 注意事项

- **`.env` 文件**：PHP 端通过 `.env` 文件加载环境变量，Rust 端建议使用 `dotenvy` crate 加载 `.env` 文件
- **`#[serde(default)]` vs `#[serde(default = "fn")]`**：前者用类型的 `Default::default()`，后者用自定义函数。布尔值默认 `false`，需要 `default_true` 函数才能默认 `true`
- **环境变量优先级**：环境变量 > YAML 配置 > 默认值
- **敏感配置**：数据库密码、JWT 密钥等敏感配置不应写入 YAML 文件，应通过环境变量注入
- **配置热加载**：当前不支持配置热加载，配置变更需要重启服务。未来可考虑实现 `watch` 机制

## Bug 定位提示

如果生产 Bug 表现为"配置项值错误"或"环境变量未生效"：

1. **L1 决策层**：查阅本 ADR，确认配置是否通过 `AppConfig` 结构体加载，环境变量是否按 `SZ_{SECTION}__{KEY}` 格式设置
2. **L2 运行时层**：检查 tracing span `config.load` 中的 `path` 和 `overrides` 字段
3. **L3 指标层**：无专门指标（配置加载是一次性操作）
4. **L4 代码层**：
   - 值错误 Bug → 检查 `config.rs` 的 `#[serde(default)]` 或 `#[serde(default = "fn")]` 是否正确
   - 环境变量未生效 Bug → 检查环境变量格式是否为 `SZ_{SECTION}__{KEY}`，是否使用了双下划线
   - YAML 解析错误 Bug → 检查 YAML 文件格式，特别是缩进（YAML 对缩进敏感）
   - 类型不匹配 Bug → 检查配置项的 Rust 类型与 YAML 值类型是否匹配（如 `bool` vs `string`）
