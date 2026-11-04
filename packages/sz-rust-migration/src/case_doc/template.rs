// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

pub struct TemplateGenerator;

impl TemplateGenerator {
    pub fn generate_handler_template(interface_id: &str) -> String {
        let handler_name = interface_id
            .replace("::", "_")
            .replace("Controller", "")
            .to_lowercase();
        format!(
            r#"async fn {handler_name}_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {{
    // TODO: implement handler for {interface_id}
    Json(serde_json::json!({{"code": 0, "msg": "ok"}}))
}}"#
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_has_handler_fn() {
        let template = TemplateGenerator::generate_handler_template("UserController::info");
        assert!(template.contains("async fn"));
        assert!(template.contains("user_info_handler"));
        assert!(template.contains("UserController::info"));
    }
}
