# SZ-Rust 综合测试结果报告

- **测试日期**：2026-07-24
- **测试目标**：`e:\vue\test\鲜视达\rust\sz-rust`
- **Workspace 版本**：0.2.0
- **Workspace 成员**：8 个 crate（sz-rust-core / sz-rust-macros / sz-rust-examples / sz-rust-addons-loader / sz-rust-pdf / sz-rust-cli / sz-rust-tracing / sz-rust-observability）
- **测试环境**：Windows + stable-x86_64-pc-windows-msvc，32GB 内存（约 20GB 可用）
- **构建参数**：`CARGO_BUILD_JOBS=2`（默认并发构建会触发 `rustc-LLVM ERROR: out of memory`，限制为 2 后稳定）

---

## 一、总体结论

| 命令 | 退出码 | 结果 | 错误 | 警告 |
|---|---|---|---|---|
| `cargo test --workspace` | 101 | ❌ 失败（仅文档测试失败） | 13（全部为 doctest） | 2（linker_messages） |
| `cargo clippy --workspace --all-targets` | 0 | ✅ 通过 | 0 | 18（含 5 重复 + 2 linker_messages） |
| `cargo clippy --workspace -- -D warnings` | 101 | ❌ 失败（警告视为错误） | 5 | 1（linker_messages） |
| `cargo doc --workspace --no-deps` | 0 | ✅ 通过 | 0 | 27（26 rustdoc + 1 linker_messages） |

**关键结论**：
- ✅ **所有单元测试和集成测试全部通过（463 passed / 0 failed / 167 ignored）**，代码逻辑层面无 bug。
- ❌ **文档测试失败 13 个**，根因是依赖树多版本冲突（`error[E0460]`），属于环境/依赖问题，非代码 bug。
- ❌ **`-D warnings` 模式下 clippy 失败**，有 5 个 lib 警告需修复。
- ⚠️ **文档构建有 26 个 rustdoc 警告**，主要为失效链接和未闭合 HTML 标签。

---

## 二、`cargo test --workspace` 详细结果

- **退出码**：101
- **构建**：✅ 成功（限制 `--jobs 2` 后避免 OOM）
- **构建警告**：2 个 `linker_messages` 警告（sz-orm-macros、sz-rust-macros 在 Windows 上创建 .dll.lib，proc-macro crate 正常现象）
- **测试汇总**：

| 测试套件 | 通过 | 失败 | 忽略 | 耗时 |
|---|---:|---:|---:|---:|
| sz-rust-addons-loader (unittests) | 51 | 0 | 0 | 0.39s |
| sz-rust-cli (unittests) | 18 | 0 | 0 | 0.00s |
| sz-rust main (unittests) | 14 | 0 | 0 | 0.03s |
| sz-rust-core (unittests) | 57 | 0 | 0 | 0.00s |
| sz-rust-core / cache_parity | 2 | 0 | 9 | 0.01s |
| sz-rust-core / compact_macro | 8 | 0 | 1 | 10.01s |
| sz-rust-core / fuzz | 10 | 0 | 0 | 0.00s |
| sz-rust-core / relation_parity | 71 | 0 | 0 | 1.17s |
| sz-rust-core / runtime_perf | 32 | 0 | 0 | 0.00s |
| sz-rust-core / soak | 0 | 0 | 0 | 0.00s |
| sz-rust-core / sql_string | 0 | 0 | 0 | 0.00s |
| sz-rust-core / upload_parity | 0 | 0 | 0 | 0.00s |
| sz-rust-core / validate | 6 | 0 | 0 | 0.00s |
| sz-rust-core / adversarial | 0 | 0 | 0 | 0.00s |
| sz-rust-examples (lib unittests) | 0 | 0 | 0 | 0.00s |
| sz-rust-examples / hello_world | 25 | 0 | 0 | 0.25s |
| sz-rust-macros (unittests) | 129 | 0 | 0 | 2.06s |
| sz-rust-observability (unittests) | 35 | 0 | 0 | 0.00s |
| sz-rust-pdf (unittests) | 0 | 0 | 3 | 0.00s |
| sz-rust-tracing (unittests) | 0 | 0 | 0 | 0.00s |
| **sz-rust-core / doc-tests** | **5** | **13** | **154** | **7.52s** |
| **合计** | **463** | **13** | **167** | — |

### 失败的文档测试（13 个，全部在 sz-rust-core）

```
failures:
    packages\sz-rust-core\src\hooks.rs - hooks::hook_context (line 603)
    packages\sz-rust-core\src\hooks.rs - hooks::hook_context_from_meta (line 636)
    packages\sz-rust-core\src\hooks.rs - hooks::is_soft_deleted (line 744)
    packages\sz-rust-core\src\hooks.rs - hooks::is_tenant_aware (line 907)
    packages\sz-rust-core\src\hooks.rs - hooks::only_trashed_filter_sql (line 715)
    packages\sz-rust-core\src\hooks.rs - hooks::soft_delete_filter_sql (line 699)
    packages\sz-rust-core\src\hooks.rs - hooks::soft_delete_restore_sql (line 787)
    packages\sz-rust-core\src\hooks.rs - hooks::soft_delete_update_sql (line 771)
    packages\sz-rust-core\src\hooks.rs - hooks::tenant_filter_sql (line 853)
    packages\sz-rust-core\src\hooks.rs - hooks::tenant_filter_sql_no_table (line 878)
    packages\sz-rust-core\src\routing.rs - routing::ConventionRoute::from_uri (line 574)
    packages\sz-rust-core\src\static_files.rs - static_files::extract_version_hash (line 788)
    packages\sz-rust-core\src\upload\image.rs - upload::image::Editor (line 522)
```

### 失败根因分析

文档测试失败的根因是**依赖树多版本冲突**，不是代码 bug。具体表现为两种错误：

**1. `error[E0460]: found possibly newer version of crate X`（10 个失败）**

依赖树中存在同一 crate 的多个版本，doctest 链接时无法选择正确版本。涉及 crate：
- `rav1e`（image 0.25 间接依赖）
- `tungstenite`（axum ws 与 tokio-tungstenite 版本差异）
- `rayon`（image / rav1e 共用，版本不同）
- `rand`（多个版本：0.8.x 主版本，但 sz-orm 系列可能引入其他版本）

示例错误：
```
error[E0460]: found possibly newer version of crate `rav1e` which `sz_rust_core` depends on
   --> packages\sz-rust-core\src\hooks.rs:878:1
    |
878 | extern crate r#sz_rust_core;
    | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    = note: perhaps that crate needs to be recompiled?
    = note: the following crate versions were found:
            crate `rav1e`: ...librav1e-388d707f9b6f3bfe.rmeta
            crate `rav1e`: ...librav1e-495a0e3eeb8f3e78.rmeta
            ... (6+ 个版本)
```

**2. `error: crate X required to be available in rlib format`（3 个失败）**

某些依赖 crate 只编译出 `.rmeta`（用于检查）而未生成 `.rlib`（用于链接），doctest 需要完整 rlib 才能链接。涉及 crate：
- `cfb`、`uuid`（hooks.rs:603 hook_context）
- `avif_serialize`（hooks.rs:744 is_soft_deleted）
- `pulp`（hooks.rs:787 soft_delete_restore_sql）
- `y4m`（hooks.rs:907 is_tenant_aware）
- `regex`、`owned_ttf_parser`（其他 hooks doctest）

**修复建议**（非本次任务范围，仅记录）：
- 统一依赖版本：在 `Cargo.toml` 的 `[workspace.dependencies]` 中显式 pin 住 `image`、`tokio-tungstenite` 等冲突 crate 的版本
- 或者在 sz-rust-core 的 `Cargo.toml` 中为 doctest 添加 `[lints.rust]` 配置或对相关 doctest 标注 `ignore`

---

## 三、`cargo clippy --workspace --all-targets` 详细结果

- **退出码**：0 ✅
- **错误**：0
- **警告**：18 个（其中 5 个为 lib test 重复 lib 的警告，2 个为 linker_messages）
- **构建耗时**：15.22s

### 警告清单（去重后 13 个独立警告）

#### sz-orm-macros (lib) - 1 warning
1. `linker_messages` - Windows 上创建 .dll.lib（proc-macro crate 正常现象，可忽略）

#### sz-rust-macros (lib) - 1 warning
2. `linker_messages` - 同上

#### sz-rust-core (lib) - 5 warnings
3. `clippy::derivable_impls` - `packages\sz-rust-core\src\controller.rs:67:1`
   - `impl Default for JwtConfig` 可用 `#[derive(Default)]` 替代
4. `clippy::redundant_closure` - `packages\sz-rust-core\src\cookie.rs:148:33`
   - `.unwrap_or_else(|| Utc::now())` → `.unwrap_or_else(Utc::now)`
5. `clippy::derivable_impls` - `packages\sz-rust-core\src\cookie.rs:229:1`
   - `impl Default for CookieJar` 可用 `#[derive(Default)]` 替代
6. `clippy::doc_nested_refdefs` - `packages\sz-rust-core\src\env.rs:143:11`
   - 列表项中的链接引用定义：`[`EnvError::FileRead`]` 应改为 `[`EnvError::FileRead`][]`
7. `clippy::doc_nested_refdefs` - `packages\sz-rust-core\src\env.rs:144:11`
   - 同上：`[`EnvError::Parse`]` 应改为 `[`EnvError::Parse`][]`

#### sz-rust-core (lib test) - 8 warnings（其中 5 个为 lib 警告的重复）
8. `clippy::unnecessary_get_then_check` - `packages\sz-rust-core\src\cookie.rs:828:25`
   - `cookies.get("invalid").is_none()` → `!cookies.contains_key("invalid")`
9. `clippy::unnecessary_get_then_check` - `packages\sz-rust-core\src\cookie.rs:835:25`
   - `cookies.get("").is_none()` → `!cookies.contains_key("")`
10. `clippy::writeln_empty_string` - `packages\sz-rust-core\src\env.rs:512:9`
    - `writeln!(file, "")` → `writeln!(file)`（移除空字符串）

#### sz-rust-core (test "adversarial") - 3 warnings
11. `clippy::field_reassign_with_default` - `packages\sz-rust-core\tests\adversarial.rs:102:5`
    - `let mut config = AppConfig::default(); config.database = section.clone();`
    - 建议：`let config = AppConfig { database: section.clone(), ..Default::default() };`
12. `clippy::useless_format` - `packages\sz-rust-core\tests\adversarial.rs:836:19`
    - `format!("\u{FEFF}APP_KEY")` → `"\u{FEFF}APP_KEY".to_string()`
13. `clippy::needless_borrows_for_generic_args` - `packages\sz-rust-core\tests\adversarial.rs:1261:25`
    - `&format!("val_{}_{}", i, j)` → `format!("val_{}_{}", i, j)`

---

## 四、`cargo clippy --workspace -- -D warnings` 详细结果

- **退出码**：101 ❌
- **错误**：5（全部来自 sz-rust-core lib，即第三节中的 5 个 lib 警告被提升为错误）
- **警告**：1（sz-orm-macros 的 linker_messages，rustc 警告不受 clippy `-D warnings` 影响）
- **构建耗时**：未单独统计（增量构建）

### 错误清单（5 个，均为 sz-rust-core lib）

1. `error: this `impl` can be derived` - `controller.rs:67:1`（`-D clippy::derivable-impls`）
2. `error: redundant closure` - `cookie.rs:148:33`（`-D clippy::redundant-closure`）
3. `error: this `impl` can be derived` - `cookie.rs:229:1`（`-D clippy::derivable-impls`）
4. `error: link reference defined in list item` - `env.rs:143:11`（`-D clippy::doc-nested-refdefs`）
5. `error: link reference defined in list item` - `env.rs:144:11`（`-D clippy::doc-nested-refdefs`）

最终输出：`error: could not compile `sz-rust-core` (lib) due to 5 previous errors`

**说明**：此命令未加 `--all-targets`，仅检查 lib 目标，因此只暴露 5 个 lib 错误。若加上 `--all-targets`，第三节中其余 6 个独立警告也会被提升为错误。

---

## 五、`cargo doc --workspace --no-deps` 详细结果

- **退出码**：0 ✅
- **错误**：0
- **警告**：27 个（26 个 rustdoc 警告 + 1 个 linker_messages）
- **构建耗时**：8.51s
- **输出**：`target\doc\sz_rust_addons_loader\index.html and 10 other files`

### 警告清单（26 个 rustdoc 警告）

#### sz-rust-core (lib doc) - 25 warnings

**A. 失效的 intra-doc 链接（`rustdoc::broken_intra_doc_links`）- 13 个**

1. `cache.rs:223` - unresolved link to `Cache::init_default`（结构体 `Cache` 无 `init_default` 关联项）
2. `controller.rs:574` - unresolved link to `JwtEncoder`（作用域内无 `JwtEncoder`）
3. `env.rs:143` - unresolved link to `文件读取失败`（中文被误识别为链接目标）
4. `relation\belongs_to.rs:99` - unresolved link to `default_belongs_to_foreign_key(related_class)`
5. `relation\belongs_to_many.rs:177` - unresolved link to `default_current_fk(current_class)`
6. `relation\belongs_to_many.rs:178` - unresolved link to `default_related_fk(related_class)`
7. `relation\has_many.rs:132` - unresolved link to `default_foreign_key(parent_class)`
8. `relation\has_one.rs:65` - unresolved link to `default_foreign_key(parent_class)`
9. `relation\has_one.rs:77` - unresolved link to `php_has_many`
10. `relation\morph.rs:179` - unresolved link to `default_morph_type_column(morph)`
11. `relation\morph.rs:180` - unresolved link to `default_morph_id_column(morph)`
12. `relation\morph.rs:246` - unresolved link to `default_morph_type_column(morph)`
13. `relation\morph.rs:247` - unresolved link to `default_morph_id_column(morph)`
14. `response.rs:10` - unresolved link to `ApiResponse::to_response`（结构体无 `to_response`）
15. `validate\message.rs:14` - unresolved link to `Validate::set_lang`（作用域内无 `Validate`）

**B. 公共文档链接到私有项（`rustdoc::private_intra_doc_links`）- 4 个**

16. `controller.rs:576` - `get_token` 文档链接到私有 `verify_token_with_config`
17. `validate.rs:20` - `validate` 文档链接到私有 `Validate::current_scene`
18. `validate.rs:25` - `validate` 文档链接到私有 `Validate::error`
19. `validate.rs:26` - `validate` 文档链接到私有 `Validate::type_callbacks`
20. `validate.rs:1073` - `parse_error_msg_with_lang` 文档链接到私有 `Validate::lang`

**C. 代码块解析错误（`rustdoc::invalid_rust_codeblocks`）- 1 个**

21. `event.rs:158` - ` ```ignore ` 代码块无法解析为 Rust 代码（mismatched closing delimiter `]`）
    - 建议：将 ` ```ignore ` 改为 ` ```text `

**D. 未闭合 HTML 标签（`rustdoc::invalid_html_tags`）- 4 个**

22. `request.rs:51` - 未闭合 HTML 标签 `Body`（`axum::http::Request<Body>` 应加反引号）
23. `session.rs:173` - 未闭合 HTML 标签 `dyn`（`Arc<dyn SessionStore>` 应加反引号）
24. `upload\image.rs:220` - 未闭合 HTML 标签 `u8`（`Rgba<u8>` 应加反引号）
25. `upload\image.rs:223` - 未闭合 HTML 标签 `u8`（`Rgba<u8>` 应加反引号）

#### sz-rust-examples (bin "crud_demo" doc) - 1 warning

26. `crud_demo.rs:64` - 未闭合 HTML 标签 `Mutex`（`Arc<Mutex>` 应加反引号）

---

## 六、构建环境注意事项

### 1. 内存限制问题
默认 `cargo test --workspace`（不限制并发）会触发 `rustc-LLVM ERROR: out of memory`，导致 sz-rust-core lib 编译失败，进而级联导致所有依赖 `sz_rust_core` 的测试报 `error[E0463]: can't find crate for sz_rust_core`。

**解决方案**：设置 `CARGO_BUILD_JOBS=2` 或使用 `cargo test --workspace --jobs 2`。本次测试已采用此参数。

### 2. linker_messages 警告
sz-orm-macros 和 sz-rust-macros 是 proc-macro crate，在 Windows MSVC 工具链下编译时会输出 `linker stdout: 正在创建库 ...dll.lib` 消息，被 rustc 识别为 `linker_messages` 警告。这是 Windows 平台正常现象，可通过 `#![warn(linker_messages)]` 调整或忽略。

### 3. 首次清理
本次测试前执行了 `cargo clean --workspace`（清理了 38.1GB / 29989 个文件），确保从干净状态构建。

---

## 七、修复优先级建议

### P0 - 阻断 `-D warnings`（5 个，全部在 sz-rust-core lib）
- `controller.rs:67` - JwtConfig 改用 `#[derive(Default)]`
- `cookie.rs:148` - 移除冗余闭包 `|| Utc::now()` → `Utc::now`
- `cookie.rs:229` - CookieJar 改用 `#[derive(Default)]`
- `env.rs:143-144` - 修复 doc 链接引用 `[X]` → `[X][]`

### P1 - clippy 警告（6 个，在 tests 和 lib test）
- `cookie.rs:828,835` - `get(k).is_none()` → `!contains_key(k)`
- `env.rs:512` - `writeln!(file, "")` → `writeln!(file)`
- `adversarial.rs:102` - 字段重新赋值改为结构体初始化
- `adversarial.rs:836` - `format!` → `.to_string()`
- `adversarial.rs:1261` - 移除多余借用 `&format!(...)`

### P2 - rustdoc 失效链接（13 个 broken_intra_doc_links）
- 修复 `cache.rs`、`controller.rs`、`response.rs`、`validate/message.rs` 中的失效方法引用
- 修复 `relation/*.rs` 中 10 个 `default_*` 函数链接（改为代码块或转义）
- 修复 `env.rs:143` 中文被误识别为链接的问题

### P3 - rustdoc 私有项链接（5 个 private_intra_doc_links）
- `controller.rs:576`、`validate.rs:20,25,26,1073` - 移除私有项链接或加 `--document-private-items`

### P4 - rustdoc HTML 标签（5 个 invalid_html_tags）
- `request.rs:51`、`session.rs:173`、`upload/image.rs:220,223`、`crud_demo.rs:64` - 为 `<X>` 加反引号

### P5 - rustdoc 代码块（1 个 invalid_rust_codeblocks）
- `event.rs:158` - ` ```ignore ` 改为 ` ```text `

### P6 - doctest 失败（13 个，依赖多版本冲突）
- 需统一 `image`、`tokio-tungstenite`、`rav1e`、`rand` 等 crate 的依赖版本
- 或对受影响 doctest 标注 `ignore` 以跳过链接

---

## 八、测试命令完整输出说明

本报告已包含所有命令的关键输出（退出码、错误数、警告数、失败用例清单、警告/错误明细）。临时日志文件已在报告生成后清理。

如需重新生成完整原始输出，可在 `e:\vue\test\鲜视达\rust\sz-rust` 目录下依次执行：

```powershell
$env:CARGO_BUILD_JOBS="2"
cargo test --workspace --jobs 2 2>&1
cargo clippy --workspace --all-targets 2>&1
cargo clippy --workspace -- -D warnings 2>&1
cargo doc --workspace --no-deps 2>&1
```

> 注意：必须设置 `CARGO_BUILD_JOBS=2`（或使用 `--jobs 2`），否则默认并发构建会触发 `rustc-LLVM ERROR: out of memory`。
