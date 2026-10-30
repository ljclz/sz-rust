// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! HotReloadManager — 配置热重载核心管理器
//!
//! 持有规则/策略/自定义生成器注册表，提供 CRUD 接口，并在每次变更时
//! 自增世代号、广播 ChangeEvent、记录审计日志、更新指标。
//! 单条 upsert 走 DashMap 单键原子操作，保证并发安全。

use super::audit::{AuditEvent, AuditLogger};
use super::generation::PolicyGeneration;
use super::notifier::{ChangeEvent, ChangeNotifier};
use crate::data_scope::custom::CustomGeneratorRegistry;
use crate::data_scope::error::DataScopeError;
use crate::data_scope::field_scope::policy::FieldScopePolicy;
use crate::data_scope::field_scope::registry::FieldScopePolicyRegistry;
use crate::data_scope::field_scope::visibility::FieldVisibility;
use crate::data_scope::metrics::DataScopeMetrics;
use crate::data_scope::registry::{DataScopeRuleRegistry, UpsertAction};
use crate::data_scope::rule::{DataScopeMode, DataScopeRule};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 规则变更结果
#[derive(Debug, Serialize)]
pub struct RuleChangeResult {
    pub rule_id: String,
    pub generation: u64,
    pub status: &'static str,
}

/// 策略变更结果
#[derive(Debug, Serialize)]
pub struct PolicyChangeResult {
    pub policy_id: String,
    pub generation: u64,
    pub status: &'static str,
}

/// 规则补丁（字段全 Option，None 表示不更新）
#[derive(Debug, Deserialize, Default)]
pub struct RulePatch {
    pub dept_field: Option<Option<String>>,
    pub creator_field: Option<Option<String>>,
    pub custom_generator: Option<Option<String>>,
    pub priority: Option<u32>,
    pub enabled: Option<bool>,
}

/// 策略补丁
#[derive(Debug, Deserialize, Default)]
pub struct PolicyPatch {
    pub field_rules: Option<Vec<(String, FieldVisibility)>>,
    pub default_visibility: Option<FieldVisibility>,
    pub enabled: Option<bool>,
}

/// 配置热重载管理器
pub struct HotReloadManager {
    rule_registry: Arc<DataScopeRuleRegistry>,
    policy_registry: Arc<FieldScopePolicyRegistry>,
    custom_registry: Arc<CustomGeneratorRegistry>,
    generation: PolicyGeneration,
    notifier: ChangeNotifier,
    metrics: Arc<DataScopeMetrics>,
    audit: Arc<dyn AuditLogger>,
}

impl HotReloadManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rule_registry: Arc<DataScopeRuleRegistry>,
        policy_registry: Arc<FieldScopePolicyRegistry>,
        custom_registry: Arc<CustomGeneratorRegistry>,
        generation: PolicyGeneration,
        notifier: ChangeNotifier,
        metrics: Arc<DataScopeMetrics>,
        audit: Arc<dyn AuditLogger>,
    ) -> Self {
        Self {
            rule_registry,
            policy_registry,
            custom_registry,
            generation,
            notifier,
            metrics,
            audit,
        }
    }

    /// 当前世代号
    pub fn generation(&self) -> u64 {
        self.generation.current()
    }

    /// 当前规则总数
    pub fn rule_count(&self) -> usize {
        self.rule_registry.count_rules()
    }

    /// 当前策略总数
    pub fn policy_count(&self) -> usize {
        self.policy_registry.count_policies()
    }

    /// 订阅变更事件
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ChangeEvent> {
        self.notifier.subscribe()
    }

    /// 校验规则字段完整性
    fn validate_rule(&self, rule: &DataScopeRule) -> Result<(), DataScopeError> {
        match rule.mode {
            DataScopeMode::Dept | DataScopeMode::DeptAndSub => {
                if rule.dept_field.is_none() {
                    return Err(DataScopeError::RuleFieldMissing(format!(
                        "mode={:?} requires dept_field",
                        rule.mode
                    )));
                }
            }
            DataScopeMode::Self_ => {
                if rule.creator_field.is_none() {
                    return Err(DataScopeError::RuleFieldMissing(
                        "mode=Self_ requires creator_field".into(),
                    ));
                }
            }
            DataScopeMode::Custom => match &rule.custom_generator {
                None => {
                    return Err(DataScopeError::RuleFieldMissing(
                        "mode=Custom requires custom_generator".into(),
                    ));
                }
                Some(name) => {
                    if !self.custom_registry.contains(name) {
                        return Err(DataScopeError::CustomGeneratorNotFound(name.clone()));
                    }
                }
            },
            DataScopeMode::All => {}
        }
        Ok(())
    }

    /// 创建或覆盖规则（upsert 语义）
    pub async fn create_rule(
        &self,
        rule: DataScopeRule,
    ) -> Result<RuleChangeResult, DataScopeError> {
        self.validate_rule(&rule)?;
        let rule_id = rule.rule_id.clone();
        let target_table = rule.target_table.clone();
        let action = self.rule_registry.upsert_rule(rule);
        let gen = self.generation.bump();
        let status = match action {
            UpsertAction::Created => "created",
            UpsertAction::Updated => "updated",
        };
        let event = ChangeEvent::RuleCreated {
            generation: gen,
            target_table: target_table.clone(),
            timestamp: Utc::now(),
        };
        self.notifier.broadcast(event);
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "create_rule".into(),
                target_type: "rule".into(),
                target_id: rule_id.clone(),
                before: None,
                after: None,
                timestamp: Utc::now(),
            })
            .await;
        self.metrics.set_rule_total(self.rule_count() as u64);
        Ok(RuleChangeResult {
            rule_id,
            generation: gen,
            status,
        })
    }

    /// 更新规则（合并 patch）
    pub async fn update_rule(
        &self,
        rule_id: &str,
        patch: RulePatch,
    ) -> Result<RuleChangeResult, DataScopeError> {
        let existing = self
            .rule_registry
            .get_rule_by_id(rule_id)
            .ok_or_else(|| DataScopeError::RuleNotFound(rule_id.to_string()))?;
        let mut merged = existing.clone();
        if let Some(v) = patch.dept_field {
            merged.dept_field = v;
        }
        if let Some(v) = patch.creator_field {
            merged.creator_field = v;
        }
        if let Some(v) = patch.custom_generator {
            merged.custom_generator = v;
        }
        if let Some(v) = patch.priority {
            merged.priority = v;
        }
        if let Some(v) = patch.enabled {
            merged.enabled = v;
        }
        merged.updated_at = Some(Utc::now());
        self.validate_rule(&merged)?;
        let target_table = merged.target_table.clone();
        self.rule_registry.upsert_rule(merged);
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::RuleUpdated {
            generation: gen,
            target_table: target_table.clone(),
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "update_rule".into(),
                target_type: "rule".into(),
                target_id: rule_id.to_string(),
                before: Some(format!("{:?}", existing)),
                after: None,
                timestamp: Utc::now(),
            })
            .await;
        self.metrics.set_rule_total(self.rule_count() as u64);
        Ok(RuleChangeResult {
            rule_id: rule_id.to_string(),
            generation: gen,
            status: "updated",
        })
    }

    /// 删除规则
    pub async fn delete_rule(&self, rule_id: &str) -> Result<RuleChangeResult, DataScopeError> {
        let removed = self
            .rule_registry
            .delete_rule_by_id(rule_id)
            .ok_or_else(|| DataScopeError::RuleNotFound(rule_id.to_string()))?;
        let target_table = removed.target_table.clone();
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::RuleDeleted {
            generation: gen,
            target_table: target_table.clone(),
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "delete_rule".into(),
                target_type: "rule".into(),
                target_id: rule_id.to_string(),
                before: Some(format!("{:?}", removed)),
                after: None,
                timestamp: Utc::now(),
            })
            .await;
        self.metrics.set_rule_total(self.rule_count() as u64);
        Ok(RuleChangeResult {
            rule_id: rule_id.to_string(),
            generation: gen,
            status: "deleted",
        })
    }

    /// 校验策略字段名不重复
    fn validate_policy(policy: &FieldScopePolicy) -> Result<(), DataScopeError> {
        let mut seen = std::collections::HashSet::new();
        for (field, _) in &policy.field_rules {
            if !seen.insert(field.clone()) {
                return Err(DataScopeError::InvalidRule(format!(
                    "duplicate field '{}' in policy {}:{}",
                    field, policy.table_name, policy.role_name
                )));
            }
        }
        Ok(())
    }

    /// 创建或覆盖策略
    pub async fn create_policy(
        &self,
        policy: FieldScopePolicy,
    ) -> Result<PolicyChangeResult, DataScopeError> {
        Self::validate_policy(&policy)?;
        let policy_id = policy.policy_id();
        let target_table = policy.table_name.clone();
        let target_role = policy.role_name.clone();
        self.policy_registry.register(policy);
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::PolicyCreated {
            generation: gen,
            target_table,
            target_role,
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "create_policy".into(),
                target_type: "policy".into(),
                target_id: policy_id.clone(),
                before: None,
                after: None,
                timestamp: Utc::now(),
            })
            .await;
        self.metrics.set_policy_total(self.policy_count() as u64);
        Ok(PolicyChangeResult {
            policy_id,
            generation: gen,
            status: "created",
        })
    }

    /// 更新策略
    pub async fn update_policy(
        &self,
        table: &str,
        role: &str,
        patch: PolicyPatch,
    ) -> Result<PolicyChangeResult, DataScopeError> {
        let existing = self
            .policy_registry
            .get_policy(table, role)
            .ok_or_else(|| DataScopeError::PolicyNotFound(format!("{}:{}", table, role)))?;
        let mut merged = existing.clone();
        if let Some(v) = patch.field_rules {
            merged.field_rules = v;
        }
        if let Some(v) = patch.default_visibility {
            merged.default_visibility = v;
        }
        if let Some(v) = patch.enabled {
            merged.enabled = v;
        }
        merged.updated_at = Some(Utc::now());
        Self::validate_policy(&merged)?;
        let policy_id = merged.policy_id();
        self.policy_registry.register(merged);
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::PolicyUpdated {
            generation: gen,
            target_table: table.to_string(),
            target_role: role.to_string(),
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "update_policy".into(),
                target_type: "policy".into(),
                target_id: policy_id.clone(),
                before: Some(format!("{:?}", existing)),
                after: None,
                timestamp: Utc::now(),
            })
            .await;
        self.metrics.set_policy_total(self.policy_count() as u64);
        Ok(PolicyChangeResult {
            policy_id,
            generation: gen,
            status: "updated",
        })
    }

    /// 删除策略
    pub async fn delete_policy(
        &self,
        table: &str,
        role: &str,
    ) -> Result<PolicyChangeResult, DataScopeError> {
        let existing = self
            .policy_registry
            .get_policy(table, role)
            .ok_or_else(|| DataScopeError::PolicyNotFound(format!("{}:{}", table, role)))?;
        self.policy_registry.invalidate(table, role);
        let gen = self.generation.bump();
        self.notifier.broadcast(ChangeEvent::PolicyDeleted {
            generation: gen,
            target_table: table.to_string(),
            target_role: role.to_string(),
            timestamp: Utc::now(),
        });
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: "delete_policy".into(),
                target_type: "policy".into(),
                target_id: existing.policy_id(),
                before: Some(format!("{:?}", existing)),
                after: None,
                timestamp: Utc::now(),
            })
            .await;
        self.metrics.set_policy_total(self.policy_count() as u64);
        Ok(PolicyChangeResult {
            policy_id: format!("{}:{}", table, role),
            generation: gen,
            status: "deleted",
        })
    }

    /// 清空所有规则与策略（用于 reload confirm_clear）
    pub fn invalidate_all(&self) {
        self.rule_registry.invalidate_all();
        self.policy_registry.invalidate_all();
        self.metrics.set_rule_total(0);
        self.metrics.set_policy_total(0);
    }

    /// 广播 ConfigReloaded 事件
    pub fn broadcast_config_reloaded(&self, gen: u64, target_table: &str) {
        self.notifier.broadcast(ChangeEvent::ConfigReloaded {
            generation: gen,
            target_table: target_table.to_string(),
            timestamp: Utc::now(),
        });
    }

    /// 内部引用：规则注册表（供 ConfigLoader 使用）
    pub fn rule_registry(&self) -> &Arc<DataScopeRuleRegistry> {
        &self.rule_registry
    }

    /// 内部引用：策略注册表
    pub fn policy_registry(&self) -> &Arc<FieldScopePolicyRegistry> {
        &self.policy_registry
    }

    /// 内部引用：指标
    pub fn metrics(&self) -> &Arc<DataScopeMetrics> {
        &self.metrics
    }

    /// 内部引用：世代号（用于 ConfigLoader 在 reload 后 bump）
    pub fn generation_handle(&self) -> &PolicyGeneration {
        &self.generation
    }

    /// 记审计日志（供 ConfigLoader 调用）
    pub async fn audit_record(&self, operation: &str, target_id: &str) {
        self.audit
            .record(AuditEvent {
                operator_id: 0,
                operation: operation.to_string(),
                target_type: "config".into(),
                target_id: target_id.to_string(),
                before: None,
                after: None,
                timestamp: Utc::now(),
            })
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::ext::audit::TracingAuditLogger;

    fn make_manager() -> HotReloadManager {
        let rule_registry = Arc::new(DataScopeRuleRegistry::new());
        let policy_registry = Arc::new(FieldScopePolicyRegistry::new());
        let custom_registry = Arc::new(CustomGeneratorRegistry::new());
        let generation = PolicyGeneration::new();
        let notifier = ChangeNotifier::new(64);
        let metrics = Arc::new(DataScopeMetrics::new());
        let audit: Arc<dyn AuditLogger> = Arc::new(TracingAuditLogger);
        HotReloadManager::new(
            rule_registry,
            policy_registry,
            custom_registry,
            generation,
            notifier,
            metrics,
            audit,
        )
    }

    #[tokio::test]
    async fn test_ac01_create_rule_success() {
        let mgr = make_manager();
        let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");
        let result = mgr.create_rule(rule).await.unwrap();
        assert_eq!(result.status, "created");
        assert_eq!(mgr.rule_count(), 1);
        assert_eq!(mgr.generation(), 1);
    }

    #[tokio::test]
    async fn test_ac03_upsert_overrides_existing() {
        let mgr = make_manager();
        let r1 = DataScopeRule::new("order", DataScopeMode::Dept)
            .with_dept_field("dept_id")
            .with_priority(5);
        mgr.create_rule(r1).await.unwrap();
        let r2 = DataScopeRule::new("order", DataScopeMode::Dept)
            .with_dept_field("dept_id")
            .with_priority(10);
        let result = mgr.create_rule(r2).await.unwrap();
        assert_eq!(result.status, "updated");
        assert_eq!(mgr.rule_count(), 1, "upsert should not increase count");
    }

    #[tokio::test]
    async fn test_ac04_delete_rule() {
        let mgr = make_manager();
        let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_dept_field("dept_id");
        let created = mgr.create_rule(rule).await.unwrap();
        let deleted = mgr.delete_rule(&created.rule_id).await.unwrap();
        assert_eq!(deleted.status, "deleted");
        assert_eq!(mgr.rule_count(), 0);
    }

    #[tokio::test]
    async fn test_ac19_dept_missing_dept_field() {
        let mgr = make_manager();
        let rule = DataScopeRule::new("order", DataScopeMode::Dept);
        let err = mgr.create_rule(rule).await.unwrap_err();
        assert_eq!(err.error_code(), "RULE_FIELD_MISSING");
    }

    #[tokio::test]
    async fn test_ac19_self_missing_creator_field() {
        let mgr = make_manager();
        let rule = DataScopeRule::new("order", DataScopeMode::Self_);
        let err = mgr.create_rule(rule).await.unwrap_err();
        assert_eq!(err.error_code(), "RULE_FIELD_MISSING");
    }

    #[tokio::test]
    async fn test_ac19_custom_missing_generator_field() {
        let mgr = make_manager();
        let rule = DataScopeRule::new("order", DataScopeMode::Custom);
        let err = mgr.create_rule(rule).await.unwrap_err();
        assert_eq!(err.error_code(), "RULE_FIELD_MISSING");
    }

    #[tokio::test]
    async fn test_ac20_custom_generator_not_registered() {
        let mgr = make_manager();
        let rule = DataScopeRule::new("order", DataScopeMode::Custom)
            .with_custom_generator("not_registered");
        let err = mgr.create_rule(rule).await.unwrap_err();
        assert_eq!(err.error_code(), "CUSTOM_GENERATOR_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_ac15_generation_monotonic() {
        let mgr = make_manager();
        let g0 = mgr.generation();
        let r1 = DataScopeRule::new("t1", DataScopeMode::All);
        let res1 = mgr.create_rule(r1).await.unwrap();
        let r2 = DataScopeRule::new("t2", DataScopeMode::All);
        let res2 = mgr.create_rule(r2).await.unwrap();
        assert!(res1.generation > g0);
        assert!(res2.generation > res1.generation);
    }

    #[tokio::test]
    async fn test_update_rule_with_patch() {
        let mgr = make_manager();
        let rule = DataScopeRule::new("order", DataScopeMode::Dept)
            .with_dept_field("dept_id")
            .with_priority(5);
        let created = mgr.create_rule(rule).await.unwrap();
        let patch = RulePatch {
            priority: Some(20),
            enabled: Some(false),
            ..Default::default()
        };
        let updated = mgr.update_rule(&created.rule_id, patch).await.unwrap();
        assert_eq!(updated.status, "updated");
        let got = mgr.rule_registry.get_rule_by_id(&created.rule_id).unwrap();
        assert_eq!(got.priority, 20);
        assert!(!got.enabled);
    }

    #[tokio::test]
    async fn test_update_rule_not_found() {
        let mgr = make_manager();
        let err = mgr
            .update_rule("nonexistent", RulePatch::default())
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "RULE_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_delete_rule_not_found() {
        let mgr = make_manager();
        let err = mgr.delete_rule("nonexistent").await.unwrap_err();
        assert_eq!(err.error_code(), "RULE_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_policy_create_upsert() {
        let mgr = make_manager();
        let p1 = FieldScopePolicy::new("employee", "hr")
            .with_field_rule("salary", FieldVisibility::Hidden);
        let r1 = mgr.create_policy(p1).await.unwrap();
        assert_eq!(r1.status, "created");
        assert_eq!(mgr.policy_count(), 1);
        // 同 table+role 再次创建 → 覆盖
        let p2 = FieldScopePolicy::new("employee", "hr")
            .with_field_rule("salary", FieldVisibility::ReadOnly);
        let r2 = mgr.create_policy(p2).await.unwrap();
        assert_eq!(r2.status, "created");
        assert_eq!(mgr.policy_count(), 1, "same table+role should override");
    }

    #[tokio::test]
    async fn test_policy_delete() {
        let mgr = make_manager();
        let policy = FieldScopePolicy::new("employee", "hr")
            .with_field_rule("salary", FieldVisibility::Hidden);
        mgr.create_policy(policy).await.unwrap();
        let result = mgr.delete_policy("employee", "hr").await.unwrap();
        assert_eq!(result.status, "deleted");
        assert_eq!(mgr.policy_count(), 0);
    }

    #[tokio::test]
    async fn test_policy_not_found_on_update() {
        let mgr = make_manager();
        let err = mgr
            .update_policy("t", "r", PolicyPatch::default())
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "POLICY_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_policy_not_found_on_delete() {
        let mgr = make_manager();
        let err = mgr.delete_policy("t", "r").await.unwrap_err();
        assert_eq!(err.error_code(), "POLICY_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_policy_generation_increment() {
        let mgr = make_manager();
        let g0 = mgr.generation();
        let p1 = FieldScopePolicy::new("t1", "r1");
        let r1 = mgr.create_policy(p1).await.unwrap();
        let p2 = FieldScopePolicy::new("t2", "r2");
        let r2 = mgr.create_policy(p2).await.unwrap();
        assert!(r1.generation > g0);
        assert!(r2.generation > r1.generation);
    }

    #[tokio::test]
    async fn test_policy_duplicate_field_rejected() {
        let mgr = make_manager();
        let policy = FieldScopePolicy::new("employee", "hr")
            .with_field_rule("salary", FieldVisibility::Hidden)
            .with_field_rule("salary", FieldVisibility::Visible);
        let err = mgr.create_policy(policy).await.unwrap_err();
        assert_eq!(err.error_code(), "DATA_SCOPE_INVALID_RULE");
    }

    #[tokio::test]
    async fn test_concurrent_update_no_intermediate_state() {
        use std::sync::Arc;
        let mgr = Arc::new(make_manager());
        let rule = DataScopeRule::new("order", DataScopeMode::All).with_priority(1);
        let created = mgr.create_rule(rule).await.unwrap();
        let rule_id = created.rule_id.clone();

        // 并发 8 个 update + 8 个 query，全部应成功且不 panic
        let mut handles = vec![];
        for i in 0..8u32 {
            let m = mgr.clone();
            let id = rule_id.clone();
            handles.push(tokio::spawn(async move {
                let patch = RulePatch {
                    priority: Some(i),
                    ..Default::default()
                };
                m.update_rule(&id, patch).await.is_ok()
            }));
        }
        for _ in 0..8 {
            let m = mgr.clone();
            let id = rule_id.clone();
            handles.push(tokio::spawn(async move {
                m.rule_registry.get_rule_by_id(&id).is_some()
            }));
        }
        for h in handles {
            assert!(h.await.unwrap(), "concurrent op should succeed");
        }
        // 最终仍只有 1 条规则
        assert_eq!(mgr.rule_count(), 1);
    }

    #[tokio::test]
    async fn test_change_event_broadcast_on_create() {
        let mgr = make_manager();
        let mut rx = mgr.subscribe();
        let rule = DataScopeRule::new("order", DataScopeMode::All);
        let result = mgr.create_rule(rule).await.unwrap();
        let event = rx.recv().await.unwrap();
        match event {
            ChangeEvent::RuleCreated {
                generation,
                target_table,
                ..
            } => {
                assert_eq!(generation, result.generation);
                assert_eq!(target_table, "order");
            }
            _ => panic!("expected RuleCreated event"),
        }
    }

    #[tokio::test]
    async fn test_change_event_broadcast_on_policy_create() {
        let mgr = make_manager();
        let mut rx = mgr.subscribe();
        let policy = FieldScopePolicy::new("employee", "hr");
        let result = mgr.create_policy(policy).await.unwrap();
        let event = rx.recv().await.unwrap();
        match event {
            ChangeEvent::PolicyCreated {
                generation,
                target_table,
                target_role,
                ..
            } => {
                assert_eq!(generation, result.generation);
                assert_eq!(target_table, "employee");
                assert_eq!(target_role, "hr");
            }
            _ => panic!("expected PolicyCreated event"),
        }
    }

    #[tokio::test]
    async fn test_invalidate_all_clears_everything() {
        let mgr = make_manager();
        mgr.create_rule(DataScopeRule::new("t1", DataScopeMode::All))
            .await
            .unwrap();
        mgr.create_policy(FieldScopePolicy::new("t1", "r1"))
            .await
            .unwrap();
        assert_eq!(mgr.rule_count(), 1);
        assert_eq!(mgr.policy_count(), 1);
        mgr.invalidate_all();
        assert_eq!(mgr.rule_count(), 0);
        assert_eq!(mgr.policy_count(), 0);
    }
}
