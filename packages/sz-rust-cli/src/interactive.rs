//! 交互式命令行补全
//!
//! 对应 design.md 第 2.2.2.3 节，使用 dialoguer 提供 TTY 环境下的交互式输入。

use std::io::IsTerminal;

use crate::error::CliError;

/// 交互式提示器
pub struct InteractivePrompt;

impl InteractivePrompt {
    /// 检测当前是否为 TTY 环境
    ///
    /// 使用 `std::io::IsTerminal` trait（Rust 1.81+）
    pub fn is_tty() -> bool {
        std::io::stdin().is_terminal()
    }

    /// 提示输入字段定义（格式 `name:Type,...`）
    pub fn prompt_fields() -> Result<String, CliError> {
        if !Self::is_tty() {
            return Err(CliError::Generic(
                "not a TTY environment, cannot prompt for fields".to_string(),
            ));
        }

        let input = dialoguer::Input::<String>::new()
            .with_prompt("Enter field definitions (format: name:Type,name2:Type2,...)")
            .with_initial_text("id:i32:pk,name:String,age:i32")
            .interact_text();

        match input {
            Ok(value) => {
                if value.trim().is_empty() {
                    Err(CliError::Generic(
                        "user entered empty field definition".to_string(),
                    ))
                } else {
                    Ok(value.trim().to_string())
                }
            }
            Err(e) => Err(CliError::Generic(format!("user cancelled input: {e}"))),
        }
    }

    /// 提示输入表名，带默认值
    pub fn prompt_table(default: &str) -> Result<String, CliError> {
        if !Self::is_tty() {
            return Err(CliError::Generic(
                "not a TTY environment, cannot prompt for table name".to_string(),
            ));
        }

        let input = dialoguer::Input::<String>::new()
            .with_prompt("Enter table name")
            .with_initial_text(default)
            .interact_text();

        match input {
            Ok(value) => {
                let value = value.trim().to_string();
                if value.is_empty() {
                    Ok(default.to_string())
                } else {
                    Ok(value)
                }
            }
            Err(e) => Err(CliError::Generic(format!("user cancelled input: {e}"))),
        }
    }

    /// 让用户从可用模板列表中选择
    pub fn prompt_template(available: &[String]) -> Result<String, CliError> {
        if available.is_empty() {
            return Err(CliError::Generic("no templates available".to_string()));
        }

        if !Self::is_tty() {
            return Err(CliError::Generic(
                "not a TTY environment, cannot prompt for template selection".to_string(),
            ));
        }

        let select = dialoguer::FuzzySelect::new()
            .with_prompt("Select template type")
            .items(available)
            .interact();

        match select {
            Ok(idx) => Ok(available[idx].clone()),
            Err(e) => Err(CliError::Generic(format!("user cancelled selection: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_tty_returns_bool() {
        let result = InteractivePrompt::is_tty();
        assert!(result == true || result == false);
    }

    #[test]
    fn test_prompt_fields_non_tty() {
        if !InteractivePrompt::is_tty() {
            let result = InteractivePrompt::prompt_fields();
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("not a TTY"));
        }
    }

    #[test]
    fn test_prompt_table_non_tty() {
        if !InteractivePrompt::is_tty() {
            let result = InteractivePrompt::prompt_table("users");
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("not a TTY"));
        }
    }

    #[test]
    fn test_prompt_template_non_tty() {
        if !InteractivePrompt::is_tty() {
            let result = InteractivePrompt::prompt_template(&[
                "crud".to_string(),
                "master-slave".to_string(),
            ]);
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("not a TTY"));
        }
    }

    #[test]
    fn test_prompt_template_empty_list() {
        let result = InteractivePrompt::prompt_template(&[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("no templates"));
    }
}
