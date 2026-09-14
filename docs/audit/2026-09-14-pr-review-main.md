# PR 审查报告（2026-09-14，branch: main，range: HEAD~1..HEAD）

> 审查时点: `HEAD @ 51bc52e`（报告为时点快照；后续新提交不在本报告范围内）

## 状态机
- scanning → scanning; scanning → compile; compile → static; static → static; static → static; static → security; security → test; test → integration; integration → ai; ai → done; 最终状态: **done**
- 严重度阈值: medium（≥ 该级别阻塞）

## 问题清单（0 critical / 0 high / 0 medium / 2 low）

- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），sz300 单元测试移交企业版仓库流程，本次仅测 facade
- [low] `workspace` **gate-skipped**: sz-rust-sz300 不在 workspace members（1614e84 开源/企业版分离），jobs_integration_test 移交企业版仓库流程


## 补充信息

## 变更集
```
 ...211\247\350\241\214\346\212\245\345\221\212.md" |   8 +-
 docs/audit/coverage-exemption.md                   |   2 +-
 docs/audit/doc-debt.md                             |   2 +-
 .../sz-rust-addons-admin/tests/services_test.rs    | 411 +++++++++++++++++++++
 4 files changed, 417 insertions(+), 6 deletions(-)
```

## AI 评审（仅供参考：不进入问题计数，不参与阻塞判定）

## PR 评审报告

### 最重要的潜在问题

#### 1. **测试可靠性：SQL 字符串匹配路由的脆弱性**（可维护性/正确性）

`MockConnection::query` 使用 `sql.to_ascii_lowercase()` 后做子串匹配，存在严重缺陷：

- **大小写问题**：`to_ascii_lowercase()` 对非 ASCII 字符（如中文注释）无效，且 SQL 中若包含 `COUNT(*)` 与 `count(*)` 之外的变体（如 `COUNT( * )`）会漏匹配
- **子串冲突**：`"select id from users where username"` 与 `"select id from users where id"` 存在前缀重叠，若 SQL 为 `SELECT id FROM users WHERE username = ? AND id = ?` 会错误匹配前者
- **静默降级**：未匹配的 SQL 返回空 `vec![]`，测试可能因 mock 静默返回空结果而误判成功

```rust
// 建议：使用 SQL 解析或结构化匹配
impl MockConnection {
    fn query(sql: &str, behavior: &MockBehavior) -> QueryRows {
        // 使用正则或解析器提取关键操作类型
        let normalized = sql.trim().to_uppercase();
        if normalized.starts_with("SELECT") {
            if normalized.contains("COUNT(") {
                return vec![count_row(behavior.total_cnt)];
            }
            if normalized.contains("FROM USERS") {
                if normalized.contains("WHERE USERNAME") {
                    return if behavior.duplicate_username { vec![id_row()] } else { vec![] };
                }
                if normalized.contains("WHERE ID") {
                    return if behavior.user_exists { vec![id_row()] } else { vec![] };
                }
            }
        }
        panic!("Unexpected SQL: {}", sql); // 未匹配时 panic 而非静默返回
    }
}
```

#### 2. **并发安全：`Mutex<MockBehavior>` 的锁粒度与死锁风险**（并发）

`MockConnection::query` 中 `self.behavior.lock().unwrap().clone()` 在异步上下文中持有锁，但 `execute` 方法未加锁。若 service 层在 `execute` 和 `query` 之间修改行为，会导致测试结果不确定。更严重的是，若 `query` 内部再调用需要锁的方法（如嵌套查询），会死锁。

```rust
// 建议：使用原子类型或 RwLock，并避免在锁内做 IO
#[derive(Default, Clone)]
struct MockBehavior {
    duplicate_username: AtomicBool,
    user_exists: AtomicBool,
    total_cnt: AtomicU64,
    // ...
}

impl MockConnection {
    fn query(&self, sql: &str) -> QueryRows {
        // 直接读取原子值，无需锁
        if sql.contains("count(*)") {
            return vec![count_row(self.behavior.total_cnt.load(Ordering::SeqCst))];
        }
        // ...
    }
}
```

#### 3. **测试隔离性：共享 `MockBehavior` 导致测试间耦合**（可维护性）

所有测试共享同一个 `Arc<Mutex<MockBehavior>>`，若测试并行执行（cargo test 默认多线程），一个测试修改 `duplicate_username` 会影响其他测试。虽然当前测试可能串行，但这是隐患。

```rust
// 建议：每个测试创建独立的 MockBehavior，或使用 thread_local
#[tokio::test]
async fn test_create_user_duplicate() {
    let behavior = MockBehavior {
        duplicate_username: true,
        ..Default::default()
    };
    let conn = MockConnection::new(behavior);
    // 测试逻辑...
}
```

#### 4. **错误处理：`lock().unwrap()` 在测试中可能 panic**（健壮性）

若 `Mutex` 被污染（poisoned），`unwrap()` 会导致测试直接 panic，掩盖真实错误。测试代码应使用 `unwrap_or_else` 或 `expect` 提供上下文。

```rust
// 建议
let behavior = self.behavior.lock()
    .unwrap_or_else(|e| e.into_inner()) // 容忍 poisoned，继续使用数据
    .clone();
```

#### 5. **覆盖率数据可信度：mock 测试的覆盖率虚高**（安全/合规）

通过 SQL 路由的 mock 测试虽然能覆盖 service 层分支，但**未验证 SQL 语法正确性**。若 service 层 SQL 有语法错误（如拼写错误、表名错误），mock 测试会通过，但真实数据库会失败。这可能导致覆盖率报告虚高，掩盖真实缺陷。

```rust
// 建议：至少增加一个真实数据库的冒烟测试（如 SQLite 内存模式）
#[tokio::test]
async fn test_user_service_sqlite_smoke() {
    let pool = Pool::connect("sqlite::memory:").await.unwrap();
    // 执行 schema 初始化
    // 调用真实 service 方法
    // 断言结果
}
```

---

### 整体评分：**5/10**

**加分项**：
- 补测方向正确，针对覆盖率缺口（user_service 68.59%）有明确行动
- 文档更新清晰，豁免流程有审批人/日期/过期时间

**扣分项**：
- mock 实现过于脆弱（SQL 字符串匹配），测试可靠性存疑
- 并发安全设计不当（共享 Mutex + 原子操作缺失）
- 未验证 SQL 语法正确性，覆盖率数据可能虚高
- 测试代码 411 行但仅覆盖 17 个测试，效率偏低（每个测试约 24 行 mock 样板代码）


## 结论
✅ 通过（无 ≥ medium 级别问题）
