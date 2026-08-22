//! 多租户支持 — 在 Model 层自动注入 tenant_id 过滤
//!
//! ## 设计目标
//!
//! 对齐 SaaS 多租户场景：同一张表通过 `tenant_id` 列区分不同租户的数据，
//! 业务代码无需手动追加 `WHERE tenant_id = ?`，由框架层自动注入。
//!
//! ## 核心组件
//!
//! | 组件 | 说明 |
//! |------|------|
//! | [`TenantContext`] | 全局租户上下文（thread-local + Arc），持有当前 tenant_id |
//! | [`TenantAware`] | 业务实体实现的 trait，声明 `tenant_id_field()` 和 `tenant_id()` |
//! | [`TenantRepository`] | Repository 装饰器，自动在查询/保存/删除时注入 tenant_id |
//! | [`tenant_middleware`] | axum 中间件，从请求 Header 提取 tenant_id |
//!
//! ## 快速开始
//!
//! ```rust,ignore
//! use sz_rust_core::multi_tenant::{TenantContext, TenantAware, TenantRepository};
//! use sz_rust_core::orm::repository::{Repository, InMemoryRepository, WhereCondition, WhereOp};
//!
//! // 1. 启动时设置当前租户（通常在 auth 中间件中完成）
//! TenantContext::set_current(1001);
//!
//! // 2. 用 TenantRepository 包装底层仓库
//! let inner = Arc::new(InMemoryRepository::<Order>::new());
//! let repo = TenantRepository::new(inner);
//!
//! // 3. 查询自动追加 tenant_id 条件
//! let orders = repo.find_by(&[WhereCondition::new("status", WhereOp::Eq, Value::I64(1))])?;
//! // 实际执行的过滤条件：status=1 AND tenant_id=1001
//! ```
//!
//! ## 安全保证
//!
//! - 查询：自动追加 `tenant_id = current` AND 条件，无法绕过
//! - 保存：若实体 tenant_id 与当前租户不一致，返回 `TenantError::TenantMismatch`
//! - 删除：仅允许删除当前租户的数据
//! - 无上下文时：返回 `TenantError::TenantNotSet`，拒绝操作

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::{error::Error, fmt};

use crate::orm::repository::{
    EntityAttributes, Repository, RepositoryError, RepositoryResult, WhereCondition, WhereOp,
};
use crate::orm::Value;

// ============================================================================
// TenantContext — 全局租户上下文
// ============================================================================

thread_local! {
    static TENANT_ID: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

/// 全局租户上下文 — 持有当前请求的 tenant_id
///
/// ## 线程安全
///
/// 内部使用线程局部存储（`thread_local!`），每个线程持有独立的租户上下文。
/// 在 axum 中间件（运行于请求线程）中设置后，同线程的业务代码通过 [`Self::current()`] 读取。
/// 多线程并发场景下各线程互不干扰，适合测试并行执行。
pub struct TenantContext;

impl TenantContext {
    /// 设置当前租户 ID
    pub fn set_current(tenant_id: i64) {
        TENANT_ID.with(|cell| cell.set(Some(tenant_id)));
    }

    /// 清除当前租户 ID（请求结束后调用）
    pub fn clear() {
        TENANT_ID.with(|cell| cell.set(None));
    }

    /// 获取当前租户 ID
    ///
    /// 未设置时返回 `None`。业务代码应在此情况下拒绝数据操作。
    pub fn current() -> Option<i64> {
        TENANT_ID.with(|cell| cell.get())
    }

    /// 获取当前租户 ID，未设置时返回错误
    pub fn require_current() -> Result<i64, TenantError> {
        Self::current().ok_or(TenantError::TenantNotSet)
    }

    /// 判断是否已设置租户上下文
    pub fn is_set() -> bool {
        Self::current().is_some()
    }

    /// 创建 [`TenantGuard`] — 在 await 前捕获当前租户 ID
    ///
    /// ## 为什么需要 Guard
    ///
    /// `TenantContext` 基于 `thread_local!` 存储。在 tokio 异步运行时中，
    /// `.await` 可能导致任务切换到不同线程，使 thread_local 值静默改变。
    ///
    /// 在业务代码需要跨 await 使用租户 ID 时，应先在 await **前**调用本方法
    /// 创建 `TenantGuard`，之后通过 `guard.tenant_id()` 访问（而非 `TenantContext::current()`）。
    ///
    /// ## 使用示例
    ///
    /// ```rust,ignore
    /// async fn handle_request() -> Result<(), TenantError> {
    ///     // 在第一个 await 之前捕获租户 ID
    ///     let guard = TenantContext::guard()?;
    ///
    ///     // 以下操作可能跨越 await，但 guard 持有正确的 tenant_id
    ///     let data = fetch_from_db(guard.tenant_id()).await?;
    ///     process(data, guard.tenant_id()).await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    ///
    /// ## 安全保证
    ///
    /// - `guard.tenant_id()` 返回创建时捕获的值，不受线程切换影响
    /// - `guard.assert_current()` 可验证当前 thread_local 是否与捕获值一致
    ///   （用于检测中间件是否正确设置了上下文）
    pub fn guard() -> Result<TenantGuard, TenantError> {
        Self::require_current().map(TenantGuard::new)
    }
}

// ============================================================================
// TenantGuard — await 安全的租户 ID 持有者
// ============================================================================

/// 在 await 前捕获的租户 ID 持有者
///
/// ## 设计目的
///
/// [`TenantContext`] 基于 thread_local，在 tokio `.await` 后可能切换到不同线程，
/// 导致 `TenantContext::current()` 返回不同值（或 None）。
///
/// `TenantGuard` 在 await **前**捕获 tenant_id，将其作为普通字段持有，
/// 之后通过 `guard.tenant_id()` 访问，不受线程切换影响。
///
/// ## 使用规范
///
/// 1. 在中间件设置 `TenantContext::set_current()` 后，**第一个 await 前**调用
///    `TenantContext::guard()?` 创建 guard
/// 2. 跨 await 的业务逻辑使用 `guard.tenant_id()` 而非 `TenantContext::current()`
/// 3. 如需验证当前线程上下文仍有效，调用 `guard.assert_current()`
///
/// ## 限制
///
/// - Guard 本身是 `Copy`，可自由跨 await 传递
/// - Guard 不自动验证 thread_local 一致性 — 需显式调用 `assert_current()`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantGuard {
    tenant_id: i64,
}

impl TenantGuard {
    /// 创建新的 Guard（捕获当前租户 ID）
    fn new(tenant_id: i64) -> Self {
        Self { tenant_id }
    }

    /// 获取捕获的租户 ID
    ///
    /// 此值在 Guard 生命周期内不变，不受线程切换影响。
    pub fn tenant_id(self) -> i64 {
        self.tenant_id
    }

    /// 验证当前 thread_local 上下文与捕获值一致
    ///
    /// 返回 `Err(TenantError::TenantMismatch)` 如果：
    /// - 当前线程的 `TenantContext` 未设置
    /// - 当前线程的 `TenantContext` 与捕获值不同
    ///
    /// ## 使用场景
    ///
    /// - 在关键操作前验证上下文完整性
    /// - 调试时检测中间件是否正确设置了租户上下文
    pub fn assert_current(&self) -> Result<(), TenantError> {
        match TenantContext::current() {
            Some(current) if current == self.tenant_id => Ok(()),
            Some(current) => Err(TenantError::TenantMismatch {
                entity_tenant: self.tenant_id,
                current_tenant: current,
            }),
            None => Err(TenantError::TenantNotSet),
        }
    }
}

// ============================================================================
// TenantAware — 租户感知实体 trait
// ============================================================================

/// 租户感知实体 trait
///
/// 业务实体实现此 trait 后，[`TenantRepository`] 可自动注入 tenant_id 过滤。
pub trait TenantAware: Clone + Send + Sync + 'static {
    /// 租户 ID 字段名（默认 `"tenant_id"`）
    fn tenant_id_field() -> &'static str {
        "tenant_id"
    }

    /// 获取当前实体的租户 ID
    fn tenant_id(&self) -> i64;

    /// 设置实体的租户 ID（保存时自动注入）
    fn set_tenant_id(&mut self, tenant_id: i64);
}

// ============================================================================
// TenantError — 多租户错误类型
// ============================================================================

/// 多租户操作错误
#[derive(Debug, Clone, PartialEq)]
pub enum TenantError {
    /// 未设置租户上下文（调用方未先设置 TenantContext）
    TenantNotSet,
    /// 实体 tenant_id 与当前租户不匹配（防止跨租户写入）
    TenantMismatch {
        /// 实体携带的 tenant_id
        entity_tenant: i64,
        /// 当前上下文的 tenant_id
        current_tenant: i64,
    },
}

impl fmt::Display for TenantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TenantError::TenantNotSet => {
                write!(f, "未设置租户上下文，请先调用 TenantContext::set_current()")
            }
            TenantError::TenantMismatch {
                entity_tenant,
                current_tenant,
            } => {
                write!(
                    f,
                    "租户不匹配：实体 tenant_id={}，当前租户={}",
                    entity_tenant, current_tenant
                )
            }
        }
    }
}

impl Error for TenantError {}

impl From<TenantError> for RepositoryError {
    fn from(err: TenantError) -> Self {
        RepositoryError::Other(err.to_string())
    }
}

// ============================================================================
// TenantRepository — 租户感知 Repository 装饰器
// ============================================================================

/// 租户感知 Repository 装饰器
///
/// 包装任意 `Repository<E>`，在查询/保存/删除时自动注入 tenant_id 条件。
///
/// ## 自动注入行为
///
/// | 操作 | 注入逻辑 |
/// |------|---------|
/// | `find_by` | 追加 `AND tenant_id = current` |
/// | `find_one_by` | 追加 `AND tenant_id = current` |
/// | `save` | 校验/填充 entity.tenant_id = current |
/// | `delete` | 先按主键查找，校验 tenant_id 后删除 |
/// | `delete_by` | 追加 `AND tenant_id = current` |
/// | `count_by` | 追加 `AND tenant_id = current` |
pub struct TenantRepository<E, R> {
    inner: Arc<R>,
    _marker: std::marker::PhantomData<E>,
}

impl<E: TenantAware, R> TenantRepository<E, R> {
    /// 用底层 Repository 创建 TenantRepository
    pub fn new(inner: Arc<R>) -> Self {
        Self {
            inner,
            _marker: std::marker::PhantomData,
        }
    }

    /// 构建 tenant_id 过滤条件
    fn tenant_condition() -> Result<WhereCondition, TenantError> {
        let tid = TenantContext::require_current()?;
        Ok(WhereCondition::new(
            E::tenant_id_field(),
            WhereOp::Eq,
            Value::I64(tid),
        ))
    }

    /// 在已有条件列表末尾追加 tenant_id 条件
    fn with_tenant_filter(
        conditions: &[WhereCondition],
    ) -> Result<Vec<WhereCondition>, TenantError> {
        let mut all = conditions.to_vec();
        all.push(Self::tenant_condition()?);
        Ok(all)
    }

    /// 校验实体 tenant_id 与当前租户一致；若实体 tenant_id=0 则自动注入
    fn validate_tenant(&self, entity: &mut E) -> Result<(), TenantError> {
        let current = TenantContext::require_current()?;
        let entity_tid = entity.tenant_id();
        if entity_tid == 0 {
            entity.set_tenant_id(current);
            Ok(())
        } else if entity_tid == current {
            Ok(())
        } else {
            Err(TenantError::TenantMismatch {
                entity_tenant: entity_tid,
                current_tenant: current,
            })
        }
    }
}

impl<E, R> Repository<E> for TenantRepository<E, R>
where
    E: TenantAware + EntityAttributes,
    R: Repository<E>,
{
    type Key = R::Key;

    fn key_of(&self, entity: &E) -> Self::Key {
        self.inner.key_of(entity)
    }

    fn find_by_id(&self, key: &Self::Key) -> RepositoryResult<Option<E>> {
        let entity = self.inner.find_by_id(key)?;
        match entity {
            Some(e) => {
                let current = match TenantContext::current() {
                    Some(t) => t,
                    None => return Err(TenantError::TenantNotSet.into()),
                };
                if e.tenant_id() == current {
                    Ok(Some(e))
                } else {
                    // 数据存在但不属于当前租户 → 视为不存在（安全隐藏）
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    fn find_all(&self) -> RepositoryResult<Vec<E>> {
        let cond = Self::tenant_condition()?;
        self.inner.find_by(&[cond])
    }

    fn find_by(&self, conditions: &[WhereCondition]) -> RepositoryResult<Vec<E>> {
        let all = Self::with_tenant_filter(conditions)?;
        self.inner.find_by(&all)
    }

    fn find_one_by(&self, conditions: &[WhereCondition]) -> RepositoryResult<Option<E>> {
        let all = Self::with_tenant_filter(conditions)?;
        self.inner.find_one_by(&all)
    }

    fn save(&self, mut entity: E) -> RepositoryResult<E> {
        self.validate_tenant(&mut entity)?;
        self.inner.save(entity)
    }

    fn save_many(&self, mut entities: Vec<E>) -> RepositoryResult<Vec<E>> {
        for e in &mut entities {
            self.validate_tenant(e)?;
        }
        self.inner.save_many(entities)
    }

    fn delete(&self, key: &Self::Key) -> RepositoryResult<usize> {
        match self.find_by_id(key)? {
            Some(_) => self.inner.delete(key),
            None => Ok(0),
        }
    }

    fn delete_by(&self, conditions: &[WhereCondition]) -> RepositoryResult<usize> {
        let all = Self::with_tenant_filter(conditions)?;
        self.inner.delete_by(&all)
    }

    fn count(&self) -> RepositoryResult<u64> {
        let cond = Self::tenant_condition()?;
        self.inner.count_by(&[cond])
    }

    fn count_by(&self, conditions: &[WhereCondition]) -> RepositoryResult<u64> {
        let all = Self::with_tenant_filter(conditions)?;
        self.inner.count_by(&all)
    }
}

// ============================================================================
// 中间件 — 从请求中提取 tenant_id
// ============================================================================

use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

/// axum 中间件：从 `X-Tenant-Id` Header 提取租户 ID 并设置到 TenantContext
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::multi_tenant::tenant_middleware;
///
/// let app = Router::new()
///     .route("/api/orders", get(list_orders))
///     .layer(tower::middleware::from_fn(tenant_middleware));
/// ```
pub async fn tenant_middleware(
    req: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    let tenant_id_str = req
        .headers()
        .get("X-Tenant-Id")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "Missing X-Tenant-Id header".to_string(),
            )
        })?;

    let tenant_id: i64 = tenant_id_str.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "X-Tenant-Id must be a valid integer".to_string(),
        )
    })?;

    TenantContext::set_current(tenant_id);

    let response = next.run(req).await;
    TenantContext::clear();

    Ok(response)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orm::repository::InMemoryRepository;

    // ---- 测试用实体 ----

    #[derive(Clone, Debug, PartialEq)]
    struct TenantOrder {
        id: i64,
        tenant_id: i64,
        order_no: String,
    }

    impl EntityAttributes for TenantOrder {
        fn get_attribute(&self, field: &str) -> Option<Value> {
            match field {
                "id" => Some(Value::I64(self.id)),
                "tenant_id" => Some(Value::I64(self.tenant_id)),
                "order_no" => Some(Value::String(self.order_no.clone())),
                _ => None,
            }
        }
    }

    impl TenantAware for TenantOrder {
        fn tenant_id_field() -> &'static str {
            "tenant_id"
        }
        fn tenant_id(&self) -> i64 {
            self.tenant_id
        }
        fn set_tenant_id(&mut self, tid: i64) {
            self.tenant_id = tid;
        }
    }

    fn make_order(id: i64, tenant_id: i64, no: &str) -> TenantOrder {
        TenantOrder {
            id,
            tenant_id,
            order_no: no.to_string(),
        }
    }

    fn repo() -> TenantRepository<TenantOrder, InMemoryRepository<TenantOrder>> {
        TenantRepository::new(Arc::new(InMemoryRepository::new()))
    }

    // ---- TenantContext ----

    #[test]
    fn test_tenant_context_set_and_get() {
        TenantContext::clear();
        assert!(!TenantContext::is_set());
        TenantContext::set_current(1001);
        assert!(TenantContext::is_set());
        assert_eq!(TenantContext::current(), Some(1001));
        assert_eq!(TenantContext::require_current(), Ok(1001));
        TenantContext::clear();
    }

    #[test]
    fn test_tenant_context_require_current_fails_when_unset() {
        TenantContext::clear();
        assert!(matches!(
            TenantContext::require_current(),
            Err(TenantError::TenantNotSet)
        ));
    }

    // ---- TenantRepository: find_by 自动过滤 ----

    #[test]
    fn test_find_by_auto_filters_tenant() {
        TenantContext::clear();
        let r = repo();

        // 预置数据：两个租户的订单
        r.inner.save(make_order(1, 1001, "ORD-001")).unwrap();
        r.inner.save(make_order(2, 1001, "ORD-002")).unwrap();
        r.inner.save(make_order(3, 2002, "ORD-003")).unwrap();

        // 未设置租户上下文 → find_by 应返回错误
        TenantContext::clear();
        assert!(r.find_by(&[]).is_err());

        // 设置租户 1001 → 只能看到 2 条
        TenantContext::set_current(1001);
        let orders = r.find_by(&[]).unwrap();
        assert_eq!(orders.len(), 2);
        assert!(orders.iter().all(|o| o.tenant_id == 1001));

        // 设置租户 2002 → 只能看到 1 条
        TenantContext::set_current(2002);
        let orders = r.find_by(&[]).unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].order_no, "ORD-003");
    }

    #[test]
    fn test_find_by_with_additional_conditions() {
        TenantContext::clear();
        let r = repo();
        r.inner.save(make_order(1, 1001, "ORD-001")).unwrap();
        r.inner.save(make_order(2, 1001, "ORD-002")).unwrap();
        r.inner.save(make_order(3, 1001, "ORD-003")).unwrap();

        TenantContext::set_current(1001);
        let orders = r
            .find_by(&[WhereCondition::new("id", WhereOp::Ge, Value::I64(2))])
            .unwrap();
        assert_eq!(orders.len(), 2);
    }

    // ---- TenantRepository: find_by_id 租户校验 ----

    #[test]
    fn test_find_by_id_hides_other_tenant_data() {
        TenantContext::clear();
        let r = repo();
        r.inner.save(make_order(42, 2002, "ORD-042")).unwrap();

        TenantContext::set_current(1001);
        let result = r.find_by_id(&Value::I64(42)).unwrap();
        assert!(result.is_none(), "跨租户数据应被隐藏");
    }

    #[test]
    fn test_find_by_id_returns_own_data() {
        TenantContext::clear();
        let r = repo();
        r.inner.save(make_order(42, 1001, "ORD-042")).unwrap();

        TenantContext::set_current(1001);
        let result = r.find_by_id(&Value::I64(42)).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().order_no, "ORD-042");
    }

    // ---- TenantRepository: save 租户校验 ----

    #[test]
    fn test_save_auto_injects_tenant_when_zero() {
        TenantContext::clear();
        let r = repo();
        TenantContext::set_current(1001);

        let order = make_order(0, 0, "ORD-NEW");
        let saved = r.save(order).unwrap();
        assert_eq!(saved.tenant_id, 1001, "tenant_id 应自动注入为当前租户");
    }

    #[test]
    fn test_save_rejects_cross_tenant_write() {
        TenantContext::clear();
        let r = repo();
        TenantContext::set_current(1001);

        let order = make_order(0, 2002, "ORD-BAD");
        let result = r.save(order);
        assert!(matches!(result, Err(RepositoryError::Other(_))));
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("租户不匹配"),
            "错误信息应包含租户不匹配: {}",
            err_msg
        );
    }

    #[test]
    fn test_save_many_all_must_match_tenant() {
        TenantContext::clear();
        let r = repo();
        TenantContext::set_current(1001);

        let orders = vec![make_order(0, 0, "ORD-A"), make_order(0, 2002, "ORD-B")];
        let result = r.save_many(orders);
        assert!(result.is_err(), "批量保存中存在跨租户数据应整体失败");
    }

    // ---- TenantRepository: delete 租户校验 ----

    #[test]
    fn test_delete_only_deletes_own_tenant() {
        TenantContext::clear();
        let r = repo();
        r.inner.save(make_order(1, 2002, "ORD-001")).unwrap();

        TenantContext::set_current(1001);
        let count = r.delete(&Value::I64(1)).unwrap();
        assert_eq!(count, 0, "跨租户删除应返回 0");

        TenantContext::set_current(2002);
        let found = r.find_by_id(&Value::I64(1)).unwrap();
        assert!(found.is_some());
    }

    // ---- TenantRepository: delete_by 自动过滤 ----

    #[test]
    fn test_delete_by_auto_filters_tenant() {
        TenantContext::clear();
        let r = repo();
        r.inner.save(make_order(1, 1001, "ORD-001")).unwrap();
        r.inner.save(make_order(2, 2002, "ORD-002")).unwrap();

        TenantContext::set_current(1001);
        let count = r.delete_by(&[]).unwrap();
        assert_eq!(count, 1);

        TenantContext::set_current(2002);
        let remaining = r.find_by(&[]).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, 2);
    }

    // ---- TenantRepository: count / count_by 自动过滤 ----

    #[test]
    fn test_count_by_auto_filters_tenant() {
        TenantContext::clear();
        let r = repo();
        r.inner.save(make_order(1, 1001, "ORD-001")).unwrap();
        r.inner.save(make_order(2, 1001, "ORD-002")).unwrap();
        r.inner.save(make_order(3, 2002, "ORD-003")).unwrap();

        TenantContext::set_current(1001);
        let count = r.count_by(&[]).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_count_auto_filters_tenant() {
        TenantContext::clear();
        let r = repo();
        r.inner.save(make_order(1, 1001, "ORD-001")).unwrap();
        r.inner.save(make_order(2, 2002, "ORD-002")).unwrap();

        TenantContext::set_current(1001);
        let count = r.count().unwrap();
        assert_eq!(count, 1);
    }

    // ---- TenantError Display ----

    #[test]
    fn test_tenant_error_display() {
        let e = TenantError::TenantNotSet;
        assert!(e.to_string().contains("未设置租户上下文"));

        let e = TenantError::TenantMismatch {
            entity_tenant: 2002,
            current_tenant: 1001,
        };
        let msg = e.to_string();
        assert!(msg.contains("2002"));
        assert!(msg.contains("1001"));
        assert!(msg.contains("租户不匹配"));
    }

    // ---- TenantGuard — await 安全捕获 ----

    #[test]
    fn test_tenant_guard_captures_current_tenant() {
        TenantContext::clear();
        TenantContext::set_current(1001);

        let guard = TenantContext::guard().expect("应成功创建 guard");
        assert_eq!(guard.tenant_id(), 1001);

        // 即使 thread_local 被改变，guard 仍持有原始值
        TenantContext::set_current(2002);
        assert_eq!(guard.tenant_id(), 1001, "guard 值不应随 thread_local 改变");

        TenantContext::clear();
    }

    #[test]
    fn test_tenant_guard_fails_when_unset() {
        TenantContext::clear();
        let result = TenantContext::guard();
        assert!(matches!(result, Err(TenantError::TenantNotSet)));
    }

    #[test]
    fn test_tenant_guard_assert_current_matches() {
        TenantContext::clear();
        TenantContext::set_current(1001);

        let guard = TenantContext::guard().unwrap();
        assert!(guard.assert_current().is_ok());

        TenantContext::clear();
    }

    #[test]
    fn test_tenant_guard_assert_current_mismatch() {
        TenantContext::clear();
        TenantContext::set_current(1001);

        let guard = TenantContext::guard().unwrap();

        // 改变 thread_local → assert_current 应检测到不匹配
        TenantContext::set_current(2002);
        let result = guard.assert_current();
        assert!(matches!(result, Err(TenantError::TenantMismatch { .. })));

        TenantContext::clear();
    }

    #[test]
    fn test_tenant_guard_assert_current_after_clear() {
        TenantContext::clear();
        TenantContext::set_current(1001);

        let guard = TenantContext::guard().unwrap();

        // 清除 thread_local → assert_current 应返回 TenantNotSet
        TenantContext::clear();
        let result = guard.assert_current();
        assert!(matches!(result, Err(TenantError::TenantNotSet)));
    }

    #[test]
    fn test_tenant_guard_is_copy() {
        TenantContext::clear();
        TenantContext::set_current(1001);

        let guard = TenantContext::guard().unwrap();
        let guard_copy = guard; // Copy semantics
        assert_eq!(guard.tenant_id(), 1001);
        assert_eq!(guard_copy.tenant_id(), 1001);

        TenantContext::clear();
    }

    // ---- find_all 自动过滤 ----

    #[test]
    fn test_find_all_auto_filters_tenant() {
        TenantContext::clear();
        let r = repo();
        r.inner.save(make_order(1, 1001, "ORD-001")).unwrap();
        r.inner.save(make_order(2, 2002, "ORD-002")).unwrap();

        TenantContext::set_current(1001);
        let all = r.find_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].tenant_id, 1001);
    }

    // ---- 无上下文时 save 应失败 ----

    #[test]
    fn test_save_fails_without_tenant_context() {
        TenantContext::clear();
        let r = repo();
        let order = make_order(0, 0, "ORD-NEW");
        let result = r.save(order);
        assert!(matches!(
            result.unwrap_err().to_string().as_str(),
            s if s.contains("未设置租户上下文")
        ));
    }
}
