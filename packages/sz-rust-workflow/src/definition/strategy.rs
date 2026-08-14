use serde::{Deserialize, Serialize};

/// 候选人解析策略，对齐 design 2.2.2.6。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CandidateStrategy {
    /// 静态指定用户与角色
    Static {
        #[serde(default)]
        users: Vec<String>,
        #[serde(default)]
        roles: Vec<String>,
    },
    /// 动态表达式求值（返回 `Vec<String>`）
    Dynamic { expr: String },
    /// 调用能力获取候选人
    Capability {
        capability_name: String,
        #[serde(default)]
        args: serde_json::Value,
    },
}

/// 审批策略类型，对齐 spec 6.3.2。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStrategyType {
    /// 会签：所有候选人均同意才完成
    AndSign,
    /// 或签：任一候选人同意即完成
    OrSign,
}

impl Default for ApprovalStrategyType {
    fn default() -> Self {
        Self::AndSign
    }
}

/// 容错策略，对齐 spec 5.4.1。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultStrategy {
    /// 失败终止实例
    Fail,
    /// 跳过节点继续
    Skip,
    /// 重试（指数退避）
    Retry,
}

impl Default for FaultStrategy {
    fn default() -> Self {
        Self::Fail
    }
}

/// 状态机迁移定义，对齐 spec 6.2.4-6.2.5。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// 源状态
    pub from: String,
    /// 目标状态
    pub to: String,
    /// 触发事件名
    pub event: String,
    /// 守卫条件表达式（可选，求值为 true 才迁移）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_strategy_static_serde() {
        let s = CandidateStrategy::Static {
            users: vec!["u1".into()],
            roles: vec!["admin".into()],
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: CandidateStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn candidate_strategy_dynamic_serde() {
        let s = CandidateStrategy::Dynamic {
            expr: "$.candidates".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"type\":\"dynamic\""));
        let back: CandidateStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn candidate_strategy_capability_serde() {
        let s = CandidateStrategy::Capability {
            capability_name: "hr.get_managers".into(),
            args: serde_json::json!({"dept": "tech"}),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: CandidateStrategy = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn approval_strategy_type_serde() {
        let json = serde_json::to_string(&ApprovalStrategyType::AndSign).unwrap();
        assert_eq!(json, "\"and_sign\"");
        let json = serde_json::to_string(&ApprovalStrategyType::OrSign).unwrap();
        assert_eq!(json, "\"or_sign\"");
    }

    #[test]
    fn fault_strategy_serde() {
        assert_eq!(
            serde_json::to_string(&FaultStrategy::Fail).unwrap(),
            "\"fail\""
        );
        assert_eq!(
            serde_json::to_string(&FaultStrategy::Skip).unwrap(),
            "\"skip\""
        );
        assert_eq!(
            serde_json::to_string(&FaultStrategy::Retry).unwrap(),
            "\"retry\""
        );
    }

    #[test]
    fn transition_serde() {
        let t = Transition {
            from: "draft".into(),
            to: "review".into(),
            event: "submit".into(),
            guard: Some("$.amount > 100".into()),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: Transition = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);

        let t2 = Transition {
            from: "draft".into(),
            to: "review".into(),
            event: "submit".into(),
            guard: None,
        };
        let json2 = serde_json::to_string(&t2).unwrap();
        assert!(!json2.contains("guard"));
    }
}
