// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 工作流引擎错误码，覆盖 `WF_001`～`WF_051`，按功能分区。
///
/// | 区段 | 功能 | 数量 |
/// |------|------|------|
/// | WF_001-009 | 定义加载 | 5 |
/// | WF_010-019 | 状态机 | 5 |
/// | WF_020-029 | 审批流 | 5 |
/// | WF_030-039 | 插件节点 | 4 |
/// | WF_040-049 | 实例管理 | 3 |
/// | WF_050-099 | 设计器 API | 2 |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WorkflowErrorCode {
    // ── 定义加载 WF_001-009 ──
    /// WF_001：定义格式不支持或解析失败
    FormatUnsupported,
    /// WF_002：定义结构不完整（缺必需字段/节点）
    StructureIncomplete,
    /// WF_003：存在不可达节点（Warning 级）
    UnreachableNode,
    /// WF_004：流程无法终止（无 end 节点或存在无法到达 end 的节点）
    CannotTerminate,
    /// WF_005：插件节点引用的插件未启用或命名规范违规
    PluginUnavailable,
    /// WF_006：定义冲突（同 flow_key+version 已存在）
    DefinitionConflict,

    // ── 状态机 WF_010-019 ──
    /// WF_010：无匹配迁移（当前状态不接受该事件）
    NoMatchingTransition,
    /// WF_011：守卫表达式求值类型错误（非布尔）
    GuardTypeError,
    /// WF_012：迁移持久化失败
    TransitionPersistFailed,
    /// WF_013：守卫表达式含副作用调用
    GuardSideEffect,
    /// WF_014：实例不存在
    InstanceNotFound,
    /// WF_015：守卫求值失败（引用不存在字段等）
    GuardEvalFailed,
    /// WF_016：乐观锁冲突（实例版本号不匹配）
    OptimisticLockConflict,

    // ── 审批流 WF_020-029 ──
    /// WF_020：候选人为空集合
    NoCandidates,
    /// WF_021：撤回非首个审批节点
    WithdrawNotFirstNode,
    /// WF_022：越权办理（actor 不属于候选人集合）
    UnauthorizedHandle,
    /// WF_023：实例非 running 状态，不可办理
    InstanceNotHandleable,
    /// WF_024：任务非 pending 状态，不可办理
    TaskNotHandleable,
    /// WF_026：加签目标非法
    AddSignTargetInvalid,

    // ── 插件节点 WF_030-039 ──
    /// WF_030：能力不存在
    CapabilityNotFound,
    /// WF_031：能力调用超时
    CapabilityTimeout,
    /// WF_032：候选人能力返回格式错误（非数组）
    CandidateFormatError,
    /// WF_033：插件节点输出 Schema 校验失败
    PluginOutputSchemaFailed,

    // ── 实例管理 WF_040-049 ──
    /// WF_040：实例已挂起，拒绝事件与办理
    InstanceSuspended,
    /// WF_041：非管理员无权操作
    NotAdmin,
    /// WF_042：实例状态非法转换
    IllegalStatusTransition,

    // ── 设计器 API WF_050-099 ──
    /// WF_050：定义不存在（导出/查询时）
    DefinitionNotFound,
    /// WF_051：版本不存在（设置生效版本时）
    VersionNotFound,
}

impl WorkflowErrorCode {
    /// 返回 `WF_xxx` 格式的错误码字符串。
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::FormatUnsupported => "WF_001",
            Self::StructureIncomplete => "WF_002",
            Self::UnreachableNode => "WF_003",
            Self::CannotTerminate => "WF_004",
            Self::PluginUnavailable => "WF_005",
            Self::DefinitionConflict => "WF_006",
            Self::NoMatchingTransition => "WF_010",
            Self::GuardTypeError => "WF_011",
            Self::TransitionPersistFailed => "WF_012",
            Self::GuardSideEffect => "WF_013",
            Self::InstanceNotFound => "WF_014",
            Self::GuardEvalFailed => "WF_015",
            Self::OptimisticLockConflict => "WF_016",
            Self::NoCandidates => "WF_020",
            Self::WithdrawNotFirstNode => "WF_021",
            Self::UnauthorizedHandle => "WF_022",
            Self::InstanceNotHandleable => "WF_023",
            Self::TaskNotHandleable => "WF_024",
            Self::AddSignTargetInvalid => "WF_026",
            Self::CapabilityNotFound => "WF_030",
            Self::CapabilityTimeout => "WF_031",
            Self::CandidateFormatError => "WF_032",
            Self::PluginOutputSchemaFailed => "WF_033",
            Self::InstanceSuspended => "WF_040",
            Self::NotAdmin => "WF_041",
            Self::IllegalStatusTransition => "WF_042",
            Self::DefinitionNotFound => "WF_050",
            Self::VersionNotFound => "WF_051",
        }
    }

    /// 映射 HTTP 状态码。
    ///
    /// | 错误码 | HTTP | 语义 |
    /// |--------|------|------|
    /// | WF_014/WF_050/WF_051 | 404 | 资源不存在 |
    /// | WF_022/WF_041 | 403 | 越权 |
    /// | WF_006/WF_016/WF_042 | 409 | 冲突 |
    /// | WF_023/WF_024/WF_021/WF_026 | 409 | 状态冲突 |
    /// | WF_040 | 409 | 实例挂起 |
    /// | 其他 | 400 | 请求错误 |
    pub fn http_status(&self) -> u16 {
        match self {
            Self::InstanceNotFound | Self::DefinitionNotFound | Self::VersionNotFound => 404,
            Self::UnauthorizedHandle | Self::NotAdmin => 403,
            Self::DefinitionConflict
            | Self::OptimisticLockConflict
            | Self::IllegalStatusTransition
            | Self::InstanceNotHandleable
            | Self::TaskNotHandleable
            | Self::WithdrawNotFirstNode
            | Self::AddSignTargetInvalid
            | Self::InstanceSuspended => 409,
            _ => 400,
        }
    }
}

impl fmt::Display for WorkflowErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code())
    }
}

/// 工作流引擎统一错误类型。
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub struct WorkflowError {
    /// 错误码
    pub code: WorkflowErrorCode,
    /// 人类可读消息
    pub message: String,
    /// 结构化详情（附加上下文，如缺失字段名、节点 ID 等）
    pub details: serde_json::Value,
}

impl fmt::Display for WorkflowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl WorkflowError {
    /// 构造新错误。
    pub fn new(code: WorkflowErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: serde_json::Value::Null,
        }
    }

    /// 附结构化详情。
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }

    /// 便捷构造：附单个字段详情。
    pub fn with_field(
        code: WorkflowErrorCode,
        message: impl Into<String>,
        field: &str,
        value: &str,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            details: serde_json::json!({ field: value }),
        }
    }
}

/// 工作流引擎统一 Result。
pub type WorkflowResult<T> = Result<T, WorkflowError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_display_format() {
        assert_eq!(WorkflowErrorCode::FormatUnsupported.to_string(), "WF_001");
        assert_eq!(
            WorkflowErrorCode::NoMatchingTransition.to_string(),
            "WF_010"
        );
        assert_eq!(WorkflowErrorCode::NoCandidates.to_string(), "WF_020");
        assert_eq!(WorkflowErrorCode::CapabilityNotFound.to_string(), "WF_030");
        assert_eq!(WorkflowErrorCode::InstanceSuspended.to_string(), "WF_040");
        assert_eq!(WorkflowErrorCode::DefinitionNotFound.to_string(), "WF_050");
    }

    #[test]
    fn http_status_mapping() {
        assert_eq!(WorkflowErrorCode::InstanceNotFound.http_status(), 404);
        assert_eq!(WorkflowErrorCode::DefinitionNotFound.http_status(), 404);
        assert_eq!(WorkflowErrorCode::VersionNotFound.http_status(), 404);
        assert_eq!(WorkflowErrorCode::UnauthorizedHandle.http_status(), 403);
        assert_eq!(WorkflowErrorCode::NotAdmin.http_status(), 403);
        assert_eq!(WorkflowErrorCode::DefinitionConflict.http_status(), 409);
        assert_eq!(WorkflowErrorCode::OptimisticLockConflict.http_status(), 409);
        assert_eq!(
            WorkflowErrorCode::IllegalStatusTransition.http_status(),
            409
        );
        assert_eq!(WorkflowErrorCode::InstanceNotHandleable.http_status(), 409);
        assert_eq!(WorkflowErrorCode::TaskNotHandleable.http_status(), 409);
        assert_eq!(WorkflowErrorCode::WithdrawNotFirstNode.http_status(), 409);
        assert_eq!(WorkflowErrorCode::AddSignTargetInvalid.http_status(), 409);
        assert_eq!(WorkflowErrorCode::InstanceSuspended.http_status(), 409);
        assert_eq!(WorkflowErrorCode::FormatUnsupported.http_status(), 400);
        assert_eq!(WorkflowErrorCode::NoMatchingTransition.http_status(), 400);
        assert_eq!(WorkflowErrorCode::GuardSideEffect.http_status(), 400);
    }

    #[test]
    fn error_construct_and_display() {
        let err = WorkflowError::new(WorkflowErrorCode::NoMatchingTransition, "无匹配迁移");
        assert_eq!(err.code, WorkflowErrorCode::NoMatchingTransition);
        assert_eq!(err.message, "无匹配迁移");
        assert_eq!(err.details, serde_json::Value::Null);
        assert_eq!(format!("{}", err), "WF_010: 无匹配迁移");

        let err2 = WorkflowError::with_field(
            WorkflowErrorCode::StructureIncomplete,
            "缺少 start 节点",
            "missing",
            "start",
        );
        assert_eq!(err2.details["missing"], "start");
    }

    #[test]
    fn error_with_details() {
        let err = WorkflowError::new(WorkflowErrorCode::PluginUnavailable, "插件未启用")
            .with_details(serde_json::json!({"plugin": "crm", "node_id": "n1"}));
        assert_eq!(err.details["plugin"], "crm");
        assert_eq!(err.details["node_id"], "n1");
    }

    #[test]
    fn error_code_count() {
        let all = [
            WorkflowErrorCode::FormatUnsupported,
            WorkflowErrorCode::StructureIncomplete,
            WorkflowErrorCode::UnreachableNode,
            WorkflowErrorCode::CannotTerminate,
            WorkflowErrorCode::PluginUnavailable,
            WorkflowErrorCode::DefinitionConflict,
            WorkflowErrorCode::NoMatchingTransition,
            WorkflowErrorCode::GuardTypeError,
            WorkflowErrorCode::TransitionPersistFailed,
            WorkflowErrorCode::GuardSideEffect,
            WorkflowErrorCode::InstanceNotFound,
            WorkflowErrorCode::GuardEvalFailed,
            WorkflowErrorCode::OptimisticLockConflict,
            WorkflowErrorCode::NoCandidates,
            WorkflowErrorCode::WithdrawNotFirstNode,
            WorkflowErrorCode::UnauthorizedHandle,
            WorkflowErrorCode::InstanceNotHandleable,
            WorkflowErrorCode::TaskNotHandleable,
            WorkflowErrorCode::AddSignTargetInvalid,
            WorkflowErrorCode::CapabilityNotFound,
            WorkflowErrorCode::CapabilityTimeout,
            WorkflowErrorCode::CandidateFormatError,
            WorkflowErrorCode::PluginOutputSchemaFailed,
            WorkflowErrorCode::InstanceSuspended,
            WorkflowErrorCode::NotAdmin,
            WorkflowErrorCode::IllegalStatusTransition,
            WorkflowErrorCode::DefinitionNotFound,
            WorkflowErrorCode::VersionNotFound,
        ];
        let codes: std::collections::HashSet<_> = all.iter().map(|c| c.as_code()).collect();
        assert_eq!(codes.len(), 28, "28 个唯一错误码");
    }
}
