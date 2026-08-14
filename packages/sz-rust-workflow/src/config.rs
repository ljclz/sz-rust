use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};

/// 工作流引擎配置，对齐 design 2.1.2。
///
/// 所有字段均有默认值与取值范围，[`WorkflowConfig::validate`] 校验越界。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfig {
    /// 单次执行最大节点跳转数（死循环防护），默认 10000，范围 [100, 1_000_000]
    pub max_node_hops: u32,
    /// 插件能力调用超时，默认 5s，范围 [100ms, 60s]
    pub plugin_call_timeout: Duration,
    /// 插件能力调用重试上限，默认 3，范围 [0, 10]
    pub plugin_retry_max: u32,
    /// 插件能力调用重试初始退避，默认 100ms，范围 [10ms, 10s]
    pub plugin_retry_backoff: Duration,
    /// 守卫表达式最大长度（字符），默认 1024，范围 [64, 8192]
    pub guard_expr_max_length: usize,
    /// 流程上下文最大体积（KB），默认 256，范围 [16, 16384]
    pub context_max_size_kb: u32,
    /// 故障恢复批量加载实例数，默认 500，范围 [10, 10000]
    pub instance_recovery_batch: u32,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self {
            max_node_hops: 10_000,
            plugin_call_timeout: Duration::from_secs(5),
            plugin_retry_max: 3,
            plugin_retry_backoff: Duration::from_millis(100),
            guard_expr_max_length: 1024,
            context_max_size_kb: 256,
            instance_recovery_batch: 500,
        }
    }
}

impl WorkflowConfig {
    /// 校验配置取值范围，越界返回 [`WorkflowError`]。
    pub fn validate(&self) -> WorkflowResult<()> {
        if self.max_node_hops < 100 || self.max_node_hops > 1_000_000 {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::StructureIncomplete,
                "max_node_hops 越界，合法范围 [100, 1_000_000]",
                "field",
                "max_node_hops",
            ));
        }
        if self.plugin_call_timeout < Duration::from_millis(100)
            || self.plugin_call_timeout > Duration::from_secs(60)
        {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::StructureIncomplete,
                "plugin_call_timeout 越界，合法范围 [100ms, 60s]",
                "field",
                "plugin_call_timeout",
            ));
        }
        if self.plugin_retry_max > 10 {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::StructureIncomplete,
                "plugin_retry_max 越界，合法范围 [0, 10]",
                "field",
                "plugin_retry_max",
            ));
        }
        if self.plugin_retry_backoff < Duration::from_millis(10)
            || self.plugin_retry_backoff > Duration::from_secs(10)
        {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::StructureIncomplete,
                "plugin_retry_backoff 越界，合法范围 [10ms, 10s]",
                "field",
                "plugin_retry_backoff",
            ));
        }
        if self.guard_expr_max_length < 64 || self.guard_expr_max_length > 8192 {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::StructureIncomplete,
                "guard_expr_max_length 越界，合法范围 [64, 8192]",
                "field",
                "guard_expr_max_length",
            ));
        }
        if self.context_max_size_kb < 16 || self.context_max_size_kb > 16384 {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::StructureIncomplete,
                "context_max_size_kb 越界，合法范围 [16, 16384]",
                "field",
                "context_max_size_kb",
            ));
        }
        if self.instance_recovery_batch < 10 || self.instance_recovery_batch > 10000 {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::StructureIncomplete,
                "instance_recovery_batch 越界，合法范围 [10, 10000]",
                "field",
                "instance_recovery_batch",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = WorkflowConfig::default();
        assert_eq!(cfg.max_node_hops, 10_000);
        assert_eq!(cfg.plugin_call_timeout, Duration::from_secs(5));
        assert_eq!(cfg.plugin_retry_max, 3);
        assert_eq!(cfg.plugin_retry_backoff, Duration::from_millis(100));
        assert_eq!(cfg.guard_expr_max_length, 1024);
        assert_eq!(cfg.context_max_size_kb, 256);
        assert_eq!(cfg.instance_recovery_batch, 500);
    }

    #[test]
    fn default_validates() {
        assert!(WorkflowConfig::default().validate().is_ok());
    }

    #[test]
    fn max_node_hops_out_of_range() {
        let mut cfg = WorkflowConfig::default();
        cfg.max_node_hops = 0;
        assert!(cfg.validate().is_err());
        cfg.max_node_hops = 99;
        assert!(cfg.validate().is_err());
        cfg.max_node_hops = 1_000_001;
        assert!(cfg.validate().is_err());
        cfg.max_node_hops = 100;
        assert!(cfg.validate().is_ok());
        cfg.max_node_hops = 1_000_000;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn plugin_call_timeout_out_of_range() {
        let mut cfg = WorkflowConfig::default();
        cfg.plugin_call_timeout = Duration::from_millis(99);
        assert!(cfg.validate().is_err());
        cfg.plugin_call_timeout = Duration::from_secs(61);
        assert!(cfg.validate().is_err());
        cfg.plugin_call_timeout = Duration::from_millis(100);
        assert!(cfg.validate().is_ok());
        cfg.plugin_call_timeout = Duration::from_secs(60);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn plugin_retry_max_out_of_range() {
        let mut cfg = WorkflowConfig::default();
        cfg.plugin_retry_max = 11;
        assert!(cfg.validate().is_err());
        cfg.plugin_retry_max = 10;
        assert!(cfg.validate().is_ok());
        cfg.plugin_retry_max = 0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn context_max_size_out_of_range() {
        let mut cfg = WorkflowConfig::default();
        cfg.context_max_size_kb = 0;
        assert!(cfg.validate().is_err());
        cfg.context_max_size_kb = 15;
        assert!(cfg.validate().is_err());
        cfg.context_max_size_kb = 16385;
        assert!(cfg.validate().is_err());
        cfg.context_max_size_kb = 16;
        assert!(cfg.validate().is_ok());
        cfg.context_max_size_kb = 16384;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn yaml_deserialize() {
        let yaml = r#"
max_node_hops: 5000
plugin_call_timeout:
  secs: 10
  nanos: 0
plugin_retry_max: 5
plugin_retry_backoff:
  secs: 0
  nanos: 200000000
guard_expr_max_length: 2048
context_max_size_kb: 512
instance_recovery_batch: 1000
"#;
        let cfg: WorkflowConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.max_node_hops, 5000);
        assert_eq!(cfg.plugin_call_timeout, Duration::from_secs(10));
        assert_eq!(cfg.plugin_retry_max, 5);
        assert_eq!(cfg.plugin_retry_backoff, Duration::from_millis(200));
        assert!(cfg.validate().is_ok());
    }
}
