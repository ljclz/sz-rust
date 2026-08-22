//! T3.11 Data Scope 端到端集成测试 — 13 场景
//!
//! 对应 spec 5.3.1（12 条业务规则）+ 5.3.3（6 类异常场景）
//! 对应 tasks.md T3.11

use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use sz_rust_orm_facade::data_scope::cache::{DeptTreeCache, DeptTreeProvider};
use sz_rust_orm_facade::data_scope::context::DataScopeContext;
use sz_rust_orm_facade::data_scope::custom::{CustomConditionGenerator, CustomGeneratorRegistry};
use sz_rust_orm_facade::data_scope::error::DataScopeError;
use sz_rust_orm_facade::data_scope::evaluator::{DataScopeEvaluator, DefaultDataScopeEvaluator};
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::data_scope::rule::{DataScopeMode, DataScopeRule};
use sz_rust_orm_facade::repository::{WhereCondition, WhereOp};
use sz_rust_orm_facade::Value;

// ============================================================================
// Mock DeptTreeProvider — 部门树: 5→{6,7}, 6→{8}, 7→{}
// ============================================================================

struct MockDeptProvider {
    call_count: Arc<AtomicU32>,
}

impl MockDeptProvider {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }
    fn tracked() -> (Arc<Self>, Arc<AtomicU32>) {
        let count = Arc::new(AtomicU32::new(0));
        let provider = Arc::new(Self {
            call_count: count.clone(),
        });
        (provider, count)
    }
}

#[async_trait]
impl DeptTreeProvider for MockDeptProvider {
    async fn sub_depts(&self, dept_id: i64) -> Result<Vec<i64>, DataScopeError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        match dept_id {
            5 => Ok(vec![6, 7]),
            6 => Ok(vec![8]),
            _ => Ok(vec![]),
        }
    }
}

// ============================================================================
// Mock Circular DeptProvider — A→B→A 循环引用
// ============================================================================

struct CircularDeptProvider;

#[async_trait]
impl DeptTreeProvider for CircularDeptProvider {
    async fn sub_depts(&self, dept_id: i64) -> Result<Vec<i64>, DataScopeError> {
        match dept_id {
            1 => Ok(vec![2]),
            2 => Ok(vec![1]),
            _ => Ok(vec![]),
        }
    }
}

// ============================================================================
// Mock CustomConditionGenerator — WHERE region = 'CN'
// ============================================================================

struct RegionGenerator;

#[async_trait]
impl CustomConditionGenerator for RegionGenerator {
    fn name(&self) -> &str {
        "region_cn"
    }
    async fn generate(
        &self,
        _ctx: &DataScopeContext,
    ) -> Result<Vec<WhereCondition>, DataScopeError> {
        Ok(vec![WhereCondition::new(
            "region",
            WhereOp::Eq,
            Value::String("CN".into()),
        )])
    }
}

/// 返回空条件的生成器（模拟不安全自定义）
struct EmptyGenerator;

#[async_trait]
impl CustomConditionGenerator for EmptyGenerator {
    fn name(&self) -> &str {
        "empty_gen"
    }
    async fn generate(
        &self,
        _ctx: &DataScopeContext,
    ) -> Result<Vec<WhereCondition>, DataScopeError> {
        Ok(Vec::new())
    }
}

// ============================================================================
// Helper: 构建评估器
// ============================================================================

fn make_evaluator(
    dept_cache: Arc<DeptTreeCache>,
    custom_registry: Arc<CustomGeneratorRegistry>,
) -> DefaultDataScopeEvaluator {
    DefaultDataScopeEvaluator::new(
        dept_cache,
        custom_registry,
        Arc::new(DataScopeMetrics::new()),
    )
}

fn make_default_evaluator() -> DefaultDataScopeEvaluator {
    make_evaluator(
        Arc::new(DeptTreeCache::new(
            Arc::new(MockDeptProvider::new()),
            Duration::from_secs(300),
        )),
        Arc::new(CustomGeneratorRegistry::new()),
    )
}

fn make_evaluator_with_custom() -> DefaultDataScopeEvaluator {
    let mut registry = CustomGeneratorRegistry::new();
    registry.register(Arc::new(RegionGenerator));
    registry.register(Arc::new(EmptyGenerator));
    make_evaluator(
        Arc::new(DeptTreeCache::new(
            Arc::new(MockDeptProvider::new()),
            Duration::from_secs(300),
        )),
        Arc::new(registry),
    )
}

// ============================================================================
// 13 端到端测试场景
// ============================================================================

#[tokio::test]
async fn it_data_scope_all_mode() {
    let evaluator = make_default_evaluator();
    let ctx = DataScopeContext::new(10, 5, false);
    let rule = DataScopeRule::new("order", DataScopeMode::All);
    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert!(
        conditions.is_empty(),
        "ALL mode should return no conditions"
    );
}

#[tokio::test]
async fn it_data_scope_dept_mode() {
    let evaluator = make_default_evaluator();
    let ctx = DataScopeContext::new(10, 5, false);
    let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");
    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert_eq!(conditions.len(), 1, "DEPT mode should return 1 condition");
}

#[tokio::test]
async fn it_data_scope_dept_and_sub_mode() {
    let evaluator = make_default_evaluator();
    let ctx = DataScopeContext::new(10, 5, false);
    let rule = DataScopeRule::new("order", DataScopeMode::DeptAndSub).with_dept_field("dept_id");
    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert_eq!(
        conditions.len(),
        1,
        "DEPT_AND_SUB should return 1 IN condition"
    );
}

#[tokio::test]
async fn it_data_scope_self_mode() {
    let evaluator = make_default_evaluator();
    let ctx = DataScopeContext::new(10, 5, false);
    let rule = DataScopeRule::new("order", DataScopeMode::Self_).with_creator_field("creator_id");
    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert_eq!(conditions.len(), 1, "SELF mode should return 1 condition");
}

#[tokio::test]
async fn it_data_scope_custom_mode() {
    let evaluator = make_evaluator_with_custom();
    let ctx = DataScopeContext::new(10, 5, false);
    let rule =
        DataScopeRule::new("order", DataScopeMode::Custom).with_custom_generator("region_cn");
    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert_eq!(
        conditions.len(),
        1,
        "CUSTOM mode should return generator conditions"
    );
}

#[tokio::test]
async fn it_data_scope_super_bypass() {
    let evaluator = make_default_evaluator();
    let ctx = DataScopeContext::new(1, 5, true);
    let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");
    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert!(
        conditions.is_empty(),
        "Super admin should bypass data scope"
    );
}

#[tokio::test]
async fn it_data_scope_missing_context() {
    let evaluator = make_default_evaluator();
    let ctx = DataScopeContext::default();
    let rule = DataScopeRule::new("order", DataScopeMode::Dept);
    let result = evaluator.evaluate(&ctx, &rule).await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().error_code(), "DATA_SCOPE_INVALID_RULE");
}

#[tokio::test]
async fn it_data_scope_dept_tree_cache_hit() {
    let (provider, call_count) = MockDeptProvider::tracked();
    let cache = Arc::new(DeptTreeCache::new(provider, Duration::from_secs(300)));
    let evaluator = make_evaluator(cache, Arc::new(CustomGeneratorRegistry::new()));

    let ctx = DataScopeContext::new(10, 5, false);
    let rule = DataScopeRule::new("order", DataScopeMode::DeptAndSub).with_dept_field("dept_id");

    let _ = evaluator.evaluate(&ctx, &rule).await.unwrap();
    let calls_after_first = call_count.load(Ordering::SeqCst);

    let _ = evaluator.evaluate(&ctx, &rule).await.unwrap();
    let calls_after_second = call_count.load(Ordering::SeqCst);

    assert_eq!(
        calls_after_second, calls_after_first,
        "2nd query should hit cache"
    );
}

#[tokio::test]
async fn it_data_scope_no_rule_backward_compat() {
    let evaluator = make_default_evaluator();
    let ctx = DataScopeContext::new(10, 5, false);
    let rule = DataScopeRule::new("order", DataScopeMode::All);
    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert!(
        conditions.is_empty(),
        "No restriction = empty conditions (backward compat)"
    );
}

#[tokio::test]
async fn it_data_scope_rls_complementary() {
    let evaluator = make_default_evaluator();
    let ctx = DataScopeContext::new(10, 5, false);
    let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");

    let data_scope_conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    let rls_condition = WhereCondition::new("tenant_id", WhereOp::Eq, Value::I64(1));

    let mut all_conditions = data_scope_conditions.clone();
    all_conditions.push(rls_condition);

    assert_eq!(all_conditions.len(), 2, "Data scope + RLS = intersection");
    assert_eq!(
        data_scope_conditions.len(),
        1,
        "Data scope contributes 1 condition"
    );
}

#[tokio::test]
async fn it_data_scope_unsafe_custom_rejected() {
    let evaluator = make_evaluator_with_custom();
    let ctx = DataScopeContext::new(10, 5, false);
    let rule =
        DataScopeRule::new("order", DataScopeMode::Custom).with_custom_generator("empty_gen");
    let result = evaluator.evaluate(&ctx, &rule).await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().error_code(), "DATA_SCOPE_UNSAFE_CUSTOM");
}

#[tokio::test]
async fn it_data_scope_dept_tree_circular_ref() {
    let cache = Arc::new(DeptTreeCache::new(
        Arc::new(CircularDeptProvider),
        Duration::from_secs(300),
    ));
    let evaluator = make_evaluator(cache, Arc::new(CustomGeneratorRegistry::new()));

    let ctx = DataScopeContext::new(10, 1, false);
    let rule = DataScopeRule::new("order", DataScopeMode::DeptAndSub).with_dept_field("dept_id");

    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert_eq!(
        conditions.len(),
        1,
        "Circular ref should still produce 1 IN condition"
    );
}

#[tokio::test]
async fn it_data_scope_guard_compat() {
    let ctx = DataScopeContext::new(42, 5, false);
    assert_eq!(ctx.user_id, 42);
    assert_eq!(ctx.dept_id, 5);
    assert!(!ctx.is_super);

    let super_ctx = DataScopeContext::new(1, 0, true);
    assert!(super_ctx.is_super);

    let evaluator = make_default_evaluator();
    let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");
    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert_eq!(conditions.len(), 1);

    let bypassed = evaluator.evaluate(&super_ctx, &rule).await.unwrap();
    assert!(bypassed.is_empty());
}
