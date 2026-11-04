# sz300 覆盖率提升第二阶段交付记录

> 日期：2026-09-04
> 阶段：sz300_cov_phase2
> 依据：spec.md（411 行）+ design.md（700 行）+ tasks.md（301 行，9 组 24 子任务）
> 基线：phase1 lib 视角覆盖率 85.85%

---

## 1. 完成状态汇总

| 组 | 任务 | 状态 | 说明 |
|----|------|------|------|
| 1 | lib 结构扩展（bootstrap.rs + builders.rs + jobs/） | ✅ 完成 | main.rs 启动主干抽取为 pub fn |
| 2 | 测试基础设施（db_fixture OnceCell 共享 + 60s 超时 + 鲁棒 schema） | ✅ 完成 | start_mysql_with_schema_shared() |
| 3 | 23 个 ignored 测试迁移至 shared 容器 | ✅ 完成 | order_expire(3) + success_path(6) + services_success(13) + common_smoke(1) |
| 4 | bootstrap 抽取函数验证（bootstrap_test.rs） | ✅ 编译通过 | 2 个 #[ignore] 用例，待 Docker 验证 |
| 5 | bin 端到端测试（bin_e2e_test.rs + process.rs） | ✅ 编译通过 | 3 个 #[ignore] 用例，待 Docker 验证 |
| 6 | lib 视角覆盖率度量 + 未覆盖清单 | ⏳ 待 CI | 需 Docker 运行 llvm-cov --include-ignored |
| 7 | 补充 lib 未覆盖路径测试 | ⏳ 待 CI | 依赖组 6 未覆盖清单 |
| 8 | CI 配置更新 | ✅ 完成 | COVERAGE_THRESHOLD 保持 85（附理由）+ bin 视角度量 step |
| 9 | 交付审查与文档同步 | ✅ 完成 | 本文档 |

---

## 2. 新增 / 修改文件清单

### 新增文件
| 文件 | 行数 | 说明 |
|------|------|------|
| `packages/sz-rust-sz300/src/bootstrap.rs` | 69 | 从 main.rs 抽取的 install_signal_handlers + init_job_queue_worker |
| `packages/sz-rust-sz300/src/builders.rs` | — | 从 main.rs 移出的 build_hybrid_retriever 等构建器 |
| `packages/sz-rust-sz300/src/jobs/` | — | 从 main.rs 移出的后台任务处理器 |
| `packages/sz-rust-sz300/tests/common/db_fixture.rs` | 158 | OnceCell 共享容器 + 60s 超时 + apply_schema_robust |
| `packages/sz-rust-sz300/tests/common/process.rs` | 48 | wait_for_port + spawn_server（子进程辅助） |
| `packages/sz-rust-sz300/tests/common/request.rs` | — | 测试请求辅助 |
| `packages/sz-rust-sz300/tests/common/seed.rs` | — | 测试数据种子 |
| `packages/sz-rust-sz300/tests/common/unique.rs` | — | unique_id() 数据隔离 |
| `packages/sz-rust-sz300/tests/bootstrap_test.rs` | 47 | bootstrap pub fn 验证（2 个 #[ignore]） |
| `packages/sz-rust-sz300/tests/bin_e2e_test.rs` | 76 | bin 端到端（3 个 #[ignore]） |
| `packages/sz-rust-sz300/tests/builders_test.rs` | 93 | builders 环境变量分支覆盖（4 个用例） |
| `packages/sz-rust-sz300/tests/common_smoke_test.rs` | — | OnceCell 共享冒烟验证 |
| `packages/sz-rust-sz300/tests/order_expire_handler_test.rs` | — | 3 个用例迁移至 shared 容器 |
| `packages/sz-rust-sz300/tests/success_path_test.rs` | — | 6 个用例迁移至 shared 容器 |
| `packages/sz-rust-sz300/tests/services_success_test.rs` | — | 13 个用例迁移至 shared 容器 |

### 修改文件
| 文件 | 说明 |
|------|------|
| `packages/sz-rust-sz300/src/lib.rs` | 追加 `pub mod bootstrap;` + `pub mod builders;` + `pub mod jobs;` |
| `packages/sz-rust-sz300/src/main.rs` | 删除内联逻辑，改为调用 bootstrap/builders pub fn |
| `packages/sz-rust-sz300/tests/common/mod.rs` | 追加 `pub mod process;` |
| `.github/workflows/coverage.yml` | COVERAGE_THRESHOLD 保持 85（附理由）+ 新增 bin 视角度量 step |

---

## 3. 验证证据

### 3.1 编译验证
```
cargo check --tests -p sz-rust-sz300
→ Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.50s
```

### 3.2 代码审查（禁止项检查）
| 检查项 | 命令 | 结果 |
|--------|------|------|
| 禁用 std::fs | `grep -rn "std::fs" tests/ src/` | 仅注释中出现（"禁止 std::fs"），无实际调用 ✅ |
| 禁 crate 级 #![allow(dead_code)] | `grep -rn "#!\[allow(dead_code)\]" src/` | 无命中 ✅ |
| 禁 SELECT * | `grep -rn "SELECT \*" src/` | 仅注释中出现（"禁 SELECT *"），无实际 SQL ✅ |

### 3.3 CI 配置验证
```yaml
# .github/workflows/coverage.yml:17-18
# phase2 lib 覆盖率待 CI 环境界定（本地 Docker 不可用无法度量 --include-ignored），暂保持 85
COVERAGE_THRESHOLD: 85

# 新增 bin 视角度量 step（组8.2）
- name: Run sz300 bin coverage
  run: |
    cargo llvm-cov -p sz-rust-sz300 --bin sz300-server \
      --summary-only -- --include-ignored --test-threads=1
```

---

## 4. 待 CI 环境验证项

> 本地 Docker 不可用，以下项需在 CI 环境（Ubuntu + Docker）中验证。

| 组 | 验证命令 | 预期 |
|----|----------|------|
| 3.4 | `cargo test -p sz-rust-sz300 --include-ignored --test-threads=1` | 23 passed; 0 failed |
| 4.1 | `cargo test -p sz-rust-sz300 --test bootstrap_test --include-ignored` | 2 passed |
| 5.2 | `cargo test -p sz-rust-sz300 --test bin_e2e_test --include-ignored` | 3 passed |
| 6.1 | `cargo llvm-cov -p sz-rust-sz300 --lib --summary-only -- --include-ignored` | lib 覆盖率数值 |
| 7.2 | `cargo llvm-cov -p sz-rust-sz300 --lib --fail-under-lines 90 -- --include-ignored` | 退出码 0（若达 90%） |

---

## 5. 防幻影交付三件套

| 完成项 | 交付物路径 | 验证命令真实输出 | 变更标识 |
|--------|-----------|-----------------|----------|
| 组1 lib 结构 | `src/bootstrap.rs` `src/builders.rs` `src/jobs/` | `cargo check --tests` Finished in 17.50s | 未提交（待用户确认） |
| 组2 测试基础设施 | `tests/common/db_fixture.rs` | 编译通过 | 未提交 |
| 组3 23 个迁移 | `tests/order_expire_handler_test.rs` 等 4 文件 | 编译通过，待 CI 运行 | 未提交 |
| 组4 bootstrap 验证 | `tests/bootstrap_test.rs` | 编译通过，2 个 #[ignore]] | 未提交 |
| 组5 bin 端到端 | `tests/bin_e2e_test.rs` `tests/common/process.rs` | 编译通过，3 个 #[ignore]] | 未提交 |
| 组8 CI 配置 | `.github/workflows/coverage.yml` | bin 视角度量 step 已添加 | 未提交 |

---

## 6. 数字可溯源性

| 数字 | 来源 |
|------|------|
| phase1 基线 85.85% | phase1 交付记录（llvm-cov --lib 实测） |
| 23 个 ignored 测试 | `grep -c 'ignore = "requires Docker"'` 实测（1+3+6+13=23） |
| COVERAGE_THRESHOLD=85 | `.github/workflows/coverage.yml:18` |
| 编译 17.50s | `cargo check --tests` 实测输出 |
| bootstrap.rs 69 行 | `Get-Content | Measure-Object -Line` 实测 |
| db_fixture.rs 158 行 | `Get-Content | Measure-Object -Line` 实测 |

---

## 7. 后续行动项

1. **CI 环境验证**：推送至 PR 触发 CI，验证组 3/4/5 测试全量通过
2. **组 6 覆盖率度量**：CI 中运行 llvm-cov --lib，记录实际 lib 覆盖率数值
3. **组 7 未覆盖补充**：根据组 6 清单编写 cov_supplement_test.rs，目标 ≥90%
4. **组 8 阈值校准**：若组 7 达 90%，更新 COVERAGE_THRESHOLD 为 90
5. **提交**：用户确认后提交所有变更