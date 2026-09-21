// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;

use serde::Serialize;
use sz_rust_orm_facade::{Pool, Value};

use crate::error::AdminError;

#[derive(Debug, Clone, Serialize)]
pub struct PermissionTree {
    pub modules: Vec<ModuleNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleNode {
    pub name: String,
    pub resources: Vec<ResourceNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceNode {
    pub name: String,
    pub actions: Vec<PermissionNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionNode {
    pub code: String,
    pub name: String,
}

#[derive(Clone)]
pub struct PermissionService {
    pool: Arc<Pool>,
}

impl PermissionService {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    pub async fn list_all(
        &self,
        tenant_id: i64,
    ) -> Result<Vec<(String, String, String)>, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT code FROM permissions WHERE tenant_id = ? OR tenant_id = 0 ORDER BY code ASC",
                &[Value::I64(tenant_id)],
            )
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                let code = r.get("code")?.as_str()?;
                let parts: Vec<&str> = code.split(':').collect();
                if parts.len() == 3 {
                    Some((
                        parts[0].to_string(),
                        parts[1].to_string(),
                        parts[2].to_string(),
                    ))
                } else {
                    None
                }
            })
            .collect())
    }

    pub async fn tree(&self, tenant_id: i64) -> Result<PermissionTree, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT code, name FROM permissions WHERE tenant_id = ? OR tenant_id = 0 ORDER BY code ASC",
                &[Value::I64(tenant_id)],
            )
            .await?;

        let mut modules: std::collections::BTreeMap<
            String,
            std::collections::BTreeMap<String, Vec<PermissionNode>>,
        > = std::collections::BTreeMap::new();

        for row in &rows {
            let code = match row.get("code").and_then(|v| v.as_str()) {
                Some(c) => c.to_string(),
                None => continue,
            };
            let name = row
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let parts: Vec<&str> = code.split(':').collect();
            if parts.len() != 3 {
                continue;
            }
            let module = parts[0].to_string();
            let resource = parts[1].to_string();
            modules
                .entry(module)
                .or_default()
                .entry(resource)
                .or_default()
                .push(PermissionNode { code, name });
        }

        let module_nodes: Vec<ModuleNode> = modules
            .into_iter()
            .map(|(module_name, resources)| ModuleNode {
                name: module_name,
                resources: resources
                    .into_iter()
                    .map(|(res_name, actions)| ResourceNode {
                        name: res_name,
                        actions,
                    })
                    .collect(),
            })
            .collect();

        Ok(PermissionTree {
            modules: module_nodes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_tree_empty() {
        let tree = PermissionTree { modules: vec![] };
        assert!(tree.modules.is_empty());
    }

    #[test]
    fn test_permission_tree_structure() {
        let tree = PermissionTree {
            modules: vec![ModuleNode {
                name: "admin".into(),
                resources: vec![ResourceNode {
                    name: "user".into(),
                    actions: vec![
                        PermissionNode {
                            code: "admin:user:list".into(),
                            name: "用户列表".into(),
                        },
                        PermissionNode {
                            code: "admin:user:create".into(),
                            name: "创建用户".into(),
                        },
                    ],
                }],
            }],
        };
        assert_eq!(tree.modules.len(), 1);
        assert_eq!(tree.modules[0].resources[0].actions.len(), 2);
    }
}
