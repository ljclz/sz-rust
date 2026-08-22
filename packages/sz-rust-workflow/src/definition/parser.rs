use regex::Regex;

use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};

use super::models::{DefinitionFormat, FlowDefinition};

/// 流程定义解析器，支持 YAML/JSON 格式。
#[derive(Debug, Clone)]
pub struct DefinitionParser {
    flow_key_re: Regex,
}

impl DefinitionParser {
    pub fn new() -> Self {
        Self {
            flow_key_re: Regex::new(r"^[a-z][a-z0-9_.]{0,63}$")
                .expect("flow_key 正则编译失败（编译期常量，必然合法）"),
        }
    }

    /// 解析流程定义文本。
    ///
    /// - `format`：显式指定格式；若传入 [`DefinitionFormat::Yaml`] 但文本首字符为 `{`，仍按 YAML 解析（serde_yaml 兼容 JSON 子集）
    pub fn parse(&self, text: &str, format: DefinitionFormat) -> WorkflowResult<FlowDefinition> {
        let def = match format {
            DefinitionFormat::Json => serde_json::from_str::<FlowDefinition>(text)
                .map_err(|e| Self::parse_error(text, &e.to_string()))?,
            DefinitionFormat::Yaml => serde_yaml::from_str::<FlowDefinition>(text)
                .map_err(|e| Self::parse_error(text, &e.to_string()))?,
        };
        self.validate_flow_key(&def)?;
        Ok(def)
    }

    /// 自动检测格式并解析。
    pub fn parse_auto(&self, text: &str) -> WorkflowResult<FlowDefinition> {
        self.parse(text, DefinitionFormat::detect(text))
    }

    fn parse_error(text: &str, msg: &str) -> WorkflowError {
        let snippet: String = text.chars().take(128).collect();
        WorkflowError::new(
            WorkflowErrorCode::FormatUnsupported,
            format!("定义解析失败：{msg}"),
        )
        .with_details(serde_json::json!({ "snippet": snippet }))
    }

    fn validate_flow_key(&self, def: &FlowDefinition) -> WorkflowResult<()> {
        if !self.flow_key_re.is_match(&def.flow_key) {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::FormatUnsupported,
                "flow_key 命名违规：仅允许小写字母/数字/下划线/点号，长度 1～64，首字符须为小写字母",
                "flow_key",
                &def.flow_key,
            ));
        }
        if def.name.is_empty() || def.name.chars().count() > 128 {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::FormatUnsupported,
                "name 长度须为 1～128",
                "name",
                &def.name,
            ));
        }
        Ok(())
    }
}

impl Default for DefinitionParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_YAML: &str = r#"
flow_key: leave_request
version: "1.0.0"
name: 请假申请
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
active: true
"#;

    #[test]
    fn parse_valid_yaml() {
        let parser = DefinitionParser::new();
        let def = parser.parse(VALID_YAML, DefinitionFormat::Yaml).unwrap();
        assert_eq!(def.flow_key, "leave_request");
        assert_eq!(def.name, "请假申请");
        assert_eq!(def.nodes.len(), 2);
    }

    #[test]
    fn parse_valid_json() {
        let json = r#"{
            "flow_key": "leave_req",
            "version": "1.0.0",
            "name": "请假",
            "nodes": [
                {"node_id": "start", "node_type": "start", "kind": "start", "next": "end"},
                {"node_id": "end", "node_type": "end", "kind": "end"}
            ],
            "start_node": "start",
            "active": true
        }"#;
        let parser = DefinitionParser::new();
        let def = parser.parse(json, DefinitionFormat::Json).unwrap();
        assert_eq!(def.flow_key, "leave_req");
    }

    #[test]
    fn parse_auto_detect() {
        let parser = DefinitionParser::new();
        let def = parser.parse_auto(VALID_YAML).unwrap();
        assert_eq!(def.flow_key, "leave_request");

        let json = r#"{"flow_key":"x","version":"1.0.0","name":"x","nodes":[{"node_id":"s","node_type":"start","kind":"start","next":"e"},{"node_id":"e","node_type":"end","kind":"end"}],"start_node":"s"}"#;
        let def2 = parser.parse_auto(json).unwrap();
        assert_eq!(def2.flow_key, "x");
    }

    #[test]
    fn parse_invalid_format() {
        let parser = DefinitionParser::new();
        let result = parser.parse("not a valid yaml: [", DefinitionFormat::Yaml);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code,
            WorkflowErrorCode::FormatUnsupported
        );
    }

    #[test]
    fn parse_invalid_flow_key() {
        let yaml = r#"
flow_key: InvalidKey
version: "1.0.0"
name: test
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;
        let parser = DefinitionParser::new();
        let result = parser.parse(yaml, DefinitionFormat::Yaml);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code,
            WorkflowErrorCode::FormatUnsupported
        );
    }

    #[test]
    fn parse_empty_name() {
        let yaml = r#"
flow_key: valid_key
version: "1.0.0"
name: ""
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;
        let parser = DefinitionParser::new();
        let result = parser.parse(yaml, DefinitionFormat::Yaml);
        assert!(result.is_err());
    }
}
