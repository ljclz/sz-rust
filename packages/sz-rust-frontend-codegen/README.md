# sz-rust-frontend-codegen

> SZ-Rust 前端代码生成器 — 根据 ORM 模型自动生成 Vue/React 组件、路由、权限、API 客户端

## 功能概述

- **ORM 模型解析**：通过 syn AST 解析 Rust 源码，识别 `#[derive(Model)]` 结构体，提取字段、关系、验证规则元信息
- **Vue 3 组件生成**：列表页、详情页、新建表单、编辑表单（4 页面），支持 Element Plus / Ant Design Vue
- **React 18 组件生成**：函数组件 + Hooks 风格，antd 组件库
- **路由生成**：Vue Router / React Router v6 路由定义，支持懒加载、嵌套路由、meta 权限
- **权限生成**：路由守卫、v-permission 指令、usePermission 组合式函数、权限码常量
- **API 客户端生成**：根据 OpenAPI spec 生成请求函数与 TypeScript 类型定义
- **自定义模板**：Tera 模板引擎，支持自定义模板覆盖内置模板
- **CLI 集成**：`sz-rust make:frontend` 子命令

## 安装与依赖

在 `Cargo.toml` 中添加：

```toml
[dependencies]
sz-rust-frontend-codegen = { workspace = true }
```

## CLI 用法

```bash
# 生成 Vue 组件（默认 Element Plus）
sz-rust make:frontend --model User --model-dir src/model/ --output ./frontend/

# 生成 React 组件
sz-rust make:frontend --model User --framework react --output ./frontend/

# 使用 Ant Design Vue
sz-rust make:frontend --model User --ui ant_design_vue --output ./frontend/

# 生成测试骨架
sz-rust make:frontend --model User --with-tests --output ./frontend/

# 强制覆盖已存在文件
sz-rust make:frontend --model User --override overwrite --output ./frontend/
```

### CLI 参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `--model` | `Vec<String>` | — | 要生成的模型名（可多次指定） |
| `--model-dir` | `String` | `src/model/` | 模型目录 |
| `--framework` | `String` | `vue` | 前端框架（vue / react） |
| `--ui` | `String` | `element_plus` | UI 组件库（element_plus / ant_design_vue） |
| `--output` | `String` | `./frontend/` | 输出目录 |
| `--template-dir` | `Option<String>` | — | 自定义模板目录 |
| `--override` | `String` | `skip` | 覆盖策略（skip / overwrite / merge） |
| `--with-tests` | `bool` | `false` | 生成测试骨架 |
| `--with-interceptors` | `bool` | `false` | 生成请求拦截器 |
| `--lazy-load` | `bool` | `true` | 懒加载路由 |
| `--force` | `bool` | `false` | 强制覆盖非空输出目录 |

## 库 API 用法

```rust
use sz_rust_frontend_codegen::{
    CodegenService, GenerationConfig, Framework, UiLibrary, OverrideStrategy,
};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = GenerationConfig {
        models: vec!["User".to_string(), "Order".to_string()],
        model_dir: PathBuf::from("src/model/"),
        framework: Framework::Vue,
        ui_library: UiLibrary::ElementPlus,
        output_dir: PathBuf::from("./frontend/"),
        override_strategy: OverrideStrategy::Skip,
        with_tests: true,
        ..Default::default()
    };

    let service = CodegenService::new();
    let report = service.generate(config).await?;
    println!("{}", report.format_cli());
    Ok(())
}
```

## 内置模板清单

| 模板路径 | 说明 |
|---------|------|
| `vue/list.vue.tera` | Vue 列表页 |
| `vue/show.vue.tera` | Vue 详情页 |
| `vue/form_create.vue.tera` | Vue 新建表单 |
| `vue/form_edit.vue.tera` | Vue 编辑表单 |
| `vue/list.spec.ts.tera` | Vue 列表页测试骨架 |
| `vue/show.spec.ts.tera` | Vue 详情页测试骨架 |
| `vue/form_create.spec.ts.tera` | Vue 新建表单测试骨架 |
| `vue/form_edit.spec.ts.tera` | Vue 编辑表单测试骨架 |
| `react/list.tsx.tera` | React 列表页 |
| `react/show.tsx.tera` | React 详情页 |
| `react/form_create.tsx.tera` | React 新建表单 |
| `react/form_edit.tsx.tera` | React 编辑表单 |
| `router/routes.ts.tera` | Vue Router 路由 |
| `router/routes.tsx.tera` | React Router 路由 |
| `router/guard.ts.tera` | 路由守卫 |
| `composables/usePermission.ts.tera` | 权限组合式函数 |
| `directives/permission.ts.tera` | v-permission 指令 |
| `constants/permissions.ts.tera` | 权限码常量 |
| `api/module.ts.tera` | API 请求函数 |
| `types/module.ts.tera` | TypeScript 类型定义 |
| `utils/request.ts.tera` | HTTP 客户端封装 |

## Tera 自定义过滤器

| 过滤器 | 说明 | 示例 |
|--------|------|------|
| `rust_to_ts_type` | Rust 类型转 TypeScript 类型 | `i32` → `number` |
| `snake_to_pascal` | snake_case 转 PascalCase | `user_name` → `UserName` |
| `pascal_to_kebab` | PascalCase 转 kebab-case | `UserName` → `user-name` |
| `snake_to_camel` | snake_case 转 camelCase | `user_name` → `userName` |
| `is_sensitive` | 判断是否敏感字段 | `password` → `true` |
| `pluralize` | 复数化 | `order` → `orders` |
| `singularize` | 单数化 | `orders` → `order` |
| `capitalize` | 首字母大写 | `user` → `User` |

## 生成产物目录结构

```
frontend/
├── src/
│   ├── views/
│   │   └── user/
│   │       ├── Index.vue      # 列表页
│   │       ├── Show.vue       # 详情页
│   │       ├── Create.vue     # 新建表单
│   │       └── Edit.vue       # 编辑表单
│   ├── router/
│   │   ├── routes.ts          # 路由定义
│   │   └── guard.ts           # 路由守卫
│   ├── composables/
│   │   └── usePermission.ts   # 权限组合式函数
│   ├── directives/
│   │   └── permission.ts      # v-permission 指令
│   ├── constants/
│   │   └── permissions.ts     # 权限码常量
│   ├── api/
│   │   └── user.ts            # API 请求函数
│   ├── types/
│   │   └── user.ts            # TypeScript 类型定义
│   └── utils/
│       └── request.ts         # HTTP 客户端封装
```

## 配置文件 `.codegen.toml`

```toml
models = ["User", "Order"]
model_dir = "src/model/"
framework = "vue"
ui_library = "element_plus"
output_dir = "./frontend/"
override_strategy = "skip"
with_tests = false
with_interceptors = false
lazy_load = true
force = false
```

## 错误码列表

| 错误码 | 说明 |
|--------|------|
| `FE_CODEGEN_MODEL_DIR_NOT_FOUND` | 模型目录不存在 |
| `FE_CODEGEN_MODEL_PARSE_ERROR` | 模型解析错误 |
| `FE_CODEGEN_MODEL_MISSING` | 未指定任何模型 |
| `FE_CODEGEN_TEMPLATE_DIR_NOT_FOUND` | 模板目录不存在 |
| `FE_CODEGEN_TEMPLATE_MISSING` | 模板缺失 |
| `FE_CODEGEN_TEMPLATE_SYNTAX_ERROR` | 模板语法错误 |
| `FE_CODEGEN_TEMPLATE_RENDER_ERROR` | 模板渲染错误 |
| `FE_CODEGEN_TEMPLATE_PATH_TRAVERSAL` | 路径穿越攻击 |
| `FE_CODEGEN_TEMPLATE_INHERITANCE_CYCLE` | 模板继承循环 |
| `FE_CODEGEN_FILTER_UNKNOWN` | 未知过滤器 |
| `FE_CODEGEN_UI_LIBRARY_UNSUPPORTED` | 不支持的 UI 库 |
| `FE_CODEGEN_FRAMEWORK_CONFLICT`<` | 框架冲突 |
| `FE_CODEGEN_FILE_WRITE_ERROR` | 文件写入错误 |
| `FE_CODEGEN_OUTPUT_DIR_NOT_EMPTY` | 输出目录非空 |
| `FE_CODEGEN_CONFIG_PARSE_ERROR` | 配置解析错误 |
| `FE_CODEGEN_IO_ERROR` | IO 错误 |
| `FE_CODEGEN_GENERIC` | 通用错误 |

## 测试

```bash
cargo test -p sz-rust-frontend-codegen
```

71 个测试（52 单元 + 19 集成）全部通过。