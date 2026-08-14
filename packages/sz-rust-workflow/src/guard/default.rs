use async_trait::async_trait;

use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
use crate::guard::GuardEvaluator;

/// 默认守卫求值器，支持纯函数表达式子集。
///
/// 支持的语法：
/// - 字段访问：`$.field` 或 `$.field.subfield`
/// - 比较：`==`、`!=`、`>`、`<`、`>=`、`<=`
/// - 逻辑：`and`、`or`、`not`
/// - 字面量：数字、字符串（单引号）、`true`/`false`/`null`
///
/// 不支持函数调用（副作用检测）。
pub struct DefaultGuardEvaluator {
    max_expr_length: usize,
}

impl DefaultGuardEvaluator {
    pub fn new(max_expr_length: usize) -> Self {
        Self { max_expr_length }
    }
}

impl Default for DefaultGuardEvaluator {
    fn default() -> Self {
        Self::new(1024)
    }
}

#[async_trait]
impl GuardEvaluator for DefaultGuardEvaluator {
    async fn evaluate(&self, expr: &str, context: &serde_json::Value) -> WorkflowResult<bool> {
        if expr.len() > self.max_expr_length {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::GuardSideEffect,
                "表达式超长",
                "expr_len",
                &expr.len().to_string(),
            ));
        }
        if contains_function_call(expr) {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::GuardSideEffect,
                "表达式含函数调用",
                "expr",
                expr,
            ));
        }
        let result = eval_expr(expr.trim(), context)?;
        if let serde_json::Value::Bool(b) = result {
            Ok(b)
        } else {
            Err(WorkflowError::with_field(
                WorkflowErrorCode::GuardTypeError,
                "求值结果非布尔",
                "result",
                &result.to_string(),
            ))
        }
    }
}

fn contains_function_call(expr: &str) -> bool {
    let bytes = expr.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_alphanumeric()
                || (i < bytes.len() && bytes[i] == b'_')
            {
                i += 1;
            }
            let word = &expr[start..i];
            while i < bytes.len() && bytes[i] as char == ' ' {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'(' && word != "not" {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

fn eval_expr(expr: &str, ctx: &serde_json::Value) -> WorkflowResult<serde_json::Value> {
    let expr = expr.trim();
    if let Some(pos) = find_top_level(expr, " or ") {
        let left = eval_expr(&expr[..pos], ctx)?;
        let right = eval_expr(&expr[pos + 4..], ctx)?;
        return Ok(serde_json::Value::Bool(as_bool(&left)? || as_bool(&right)?));
    }
    if let Some(pos) = find_top_level(expr, " and ") {
        let left = eval_expr(&expr[..pos], ctx)?;
        let right = eval_expr(&expr[pos + 5..], ctx)?;
        return Ok(serde_json::Value::Bool(as_bool(&left)? && as_bool(&right)?));
    }
    if expr.starts_with("not ") || expr.starts_with("not(") {
        let inner = if expr.starts_with("not ") {
            &expr[4..]
        } else {
            &expr[3..expr.len() - 1]
        };
        let val = eval_expr(inner, ctx)?;
        return Ok(serde_json::Value::Bool(!as_bool(&val)?));
    }
    for op in &[" == ", " != ", " >= ", " <= ", " > ", " < "] {
        if let Some(pos) = find_top_level(expr, op) {
            let left = eval_value(&expr[..pos], ctx)?;
            let right = eval_value(&expr[pos + op.len()..], ctx)?;
            return Ok(serde_json::Value::Bool(compare(&left, &right, op.trim())));
        }
    }
    eval_value(expr, ctx)
}

fn find_top_level(expr: &str, sep: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let bytes = expr.as_bytes();
    let sep_bytes = sep.as_bytes();
    let sep_len = sep_bytes.len();
    for i in 0..bytes.len().saturating_sub(sep_len) {
        let c = bytes[i] as char;
        if c == '\'' {
            in_string = !in_string;
        }
        if !in_string {
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
            }
            if depth == 0 && &expr[i..i + sep_len] == sep {
                return Some(i);
            }
        }
    }
    None
}

fn eval_value(expr: &str, ctx: &serde_json::Value) -> WorkflowResult<serde_json::Value> {
    let expr = expr.trim();
    if expr.starts_with('\'') && expr.ends_with('\'') && expr.len() >= 2 {
        return Ok(serde_json::Value::String(
            expr[1..expr.len() - 1].to_string(),
        ));
    }
    if expr == "true" {
        return Ok(serde_json::Value::Bool(true));
    }
    if expr == "false" {
        return Ok(serde_json::Value::Bool(false));
    }
    if expr == "null" {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(n) = expr.parse::<i64>() {
        return Ok(serde_json::Value::Number(n.into()));
    }
    if let Ok(n) = expr.parse::<f64>() {
        return Ok(serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null));
    }
    if expr.starts_with("$.") {
        return lookup_path(&expr[2..], ctx);
    }
    Err(WorkflowError::with_field(
        WorkflowErrorCode::GuardEvalFailed,
        "无法求值表达式",
        "expr",
        expr,
    ))
}

fn lookup_path(path: &str, ctx: &serde_json::Value) -> WorkflowResult<serde_json::Value> {
    let mut current = ctx;
    for part in path.split('.') {
        if let serde_json::Value::Object(obj) = current {
            match obj.get(part) {
                Some(v) => current = v,
                None => {
                    return Err(WorkflowError::with_field(
                        WorkflowErrorCode::GuardEvalFailed,
                        "引用不存在的字段",
                        "field",
                        part,
                    ))
                }
            }
        } else {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::GuardEvalFailed,
                "路径访问非对象",
                "path",
                path,
            ));
        }
    }
    Ok(current.clone())
}

fn as_bool(v: &serde_json::Value) -> WorkflowResult<bool> {
    match v {
        serde_json::Value::Bool(b) => Ok(*b),
        _ => Err(WorkflowError::with_field(
            WorkflowErrorCode::GuardTypeError,
            "非布尔值",
            "value",
            &v.to_string(),
        )),
    }
}

fn compare(left: &serde_json::Value, right: &serde_json::Value, op: &str) -> bool {
    match op {
        "==" => left == right,
        "!=" => left != right,
        ">" | "<" | ">=" | "<=" => {
            let l = left
                .as_f64()
                .or_else(|| left.as_str().and_then(|s| s.parse::<f64>().ok()));
            let r = right
                .as_f64()
                .or_else(|| right.as_str().and_then(|s| s.parse::<f64>().ok()));
            match (l, r) {
                (Some(l), Some(r)) => match op {
                    ">" => l > r,
                    "<" => l < r,
                    ">=" => l >= r,
                    "<=" => l <= r,
                    _ => false,
                },
                _ => false,
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> serde_json::Value {
        serde_json::json!({"amount": 200, "name": "test", "flag": true, "nested": {"value": 50}})
    }

    #[tokio::test]
    async fn field_access_and_compare() {
        let ev = DefaultGuardEvaluator::default();
        assert!(ev.evaluate("$.amount > 100", &ctx()).await.unwrap());
        assert!(!ev.evaluate("$.amount > 300", &ctx()).await.unwrap());
        assert!(ev.evaluate("$.amount == 200", &ctx()).await.unwrap());
        assert!(ev.evaluate("$.amount != 100", &ctx()).await.unwrap());
        assert!(ev.evaluate("$.amount >= 200", &ctx()).await.unwrap());
        assert!(ev.evaluate("$.amount <= 200", &ctx()).await.unwrap());
    }

    #[tokio::test]
    async fn nested_field_access() {
        let ev = DefaultGuardEvaluator::default();
        assert!(ev.evaluate("$.nested.value > 40", &ctx()).await.unwrap());
        assert!(!ev.evaluate("$.nested.value > 60", &ctx()).await.unwrap());
    }

    #[tokio::test]
    async fn logical_and_or() {
        let ev = DefaultGuardEvaluator::default();
        assert!(ev
            .evaluate("$.amount > 100 and $.flag == true", &ctx())
            .await
            .unwrap());
        assert!(!ev
            .evaluate("$.amount > 100 and $.flag == false", &ctx())
            .await
            .unwrap());
        assert!(ev
            .evaluate("$.amount > 300 or $.flag == true", &ctx())
            .await
            .unwrap());
        assert!(!ev
            .evaluate("$.amount > 300 or $.flag == false", &ctx())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn logical_not() {
        let ev = DefaultGuardEvaluator::default();
        assert!(ev.evaluate("not $.flag == false", &ctx()).await.unwrap());
        assert!(!ev.evaluate("not $.flag == true", &ctx()).await.unwrap());
    }

    #[tokio::test]
    async fn string_compare() {
        let ev = DefaultGuardEvaluator::default();
        assert!(ev.evaluate("$.name == 'test'", &ctx()).await.unwrap());
        assert!(!ev.evaluate("$.name == 'other'", &ctx()).await.unwrap());
    }

    #[tokio::test]
    async fn missing_field_error() {
        let ev = DefaultGuardEvaluator::default();
        let result = ev.evaluate("$.nonexistent > 100", &ctx()).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, WorkflowErrorCode::GuardEvalFailed);
    }

    #[tokio::test]
    async fn function_call_rejected() {
        let ev = DefaultGuardEvaluator::default();
        let result = ev.evaluate("eval('1+1') == 2", &ctx()).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, WorkflowErrorCode::GuardSideEffect);
    }

    #[tokio::test]
    async fn expr_too_long() {
        let ev = DefaultGuardEvaluator::new(10);
        let result = ev
            .evaluate("$.amount > 100 and $.flag == true", &ctx())
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, WorkflowErrorCode::GuardSideEffect);
    }

    #[tokio::test]
    async fn non_boolean_result_error() {
        let ev = DefaultGuardEvaluator::default();
        let result = ev.evaluate("$.amount", &ctx()).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, WorkflowErrorCode::GuardTypeError);
    }
}
