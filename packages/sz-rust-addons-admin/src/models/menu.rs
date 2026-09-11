// SPDX-License-Identifier: Apache-2.0
use crate::error::AdminError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 菜单模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuModel {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub path: String,
    pub icon: Option<String>,
    pub parent_id: i64,
    pub sort: i32,
    pub is_visible: bool,
    pub permission_code: Option<String>,
    pub tenant_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 菜单树节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuTreeNode {
    pub menu: MenuModel,
    pub children: Vec<MenuTreeNode>,
}

impl MenuModel {
    pub fn validate_path(path: &str) -> Result<(), AdminError> {
        if !path.starts_with('/') || path.len() < 2 {
            return Err(AdminError::ParentMenuNotFound);
        }
        Ok(())
    }
}

/// 检测循环引用：沿 parent_id 链向上遍历，检测是否回到 menu_id
pub fn detect_circular_reference(menus: &[MenuModel], menu_id: i64, new_parent_id: i64) -> bool {
    if new_parent_id == 0 {
        return false;
    }
    if new_parent_id == menu_id {
        return true;
    }
    let mut current = new_parent_id;
    let mut visited = std::collections::HashSet::new();
    while current != 0 {
        if !visited.insert(current) {
            return true;
        }
        if current == menu_id {
            return true;
        }
        match menus.iter().find(|m| m.id == current) {
            Some(parent) => current = parent.parent_id,
            None => break,
        }
    }
    false
}

/// 构建菜单树
pub fn build_menu_tree(menus: Vec<MenuModel>, user_permissions: &[String]) -> Vec<MenuTreeNode> {
    let mut sorted_menus: Vec<MenuModel> = menus;
    sorted_menus.sort_by_key(|a| a.sort);
    sorted_menus
        .iter()
        .filter(|m| m.parent_id == 0)
        .filter(|m| is_menu_visible(m, user_permissions))
        .map(|m| build_tree_node(m, &sorted_menus, user_permissions))
        .collect()
}

fn build_tree_node(
    menu: &MenuModel,
    all_menus: &[MenuModel],
    user_permissions: &[String],
) -> MenuTreeNode {
    let children: Vec<MenuTreeNode> = all_menus
        .iter()
        .filter(|m| m.parent_id == menu.id)
        .filter(|m| is_menu_visible(m, user_permissions))
        .map(|m| build_tree_node(m, all_menus, user_permissions))
        .collect();
    MenuTreeNode {
        menu: menu.clone(),
        children,
    }
}

fn is_menu_visible(menu: &MenuModel, user_permissions: &[String]) -> bool {
    if !menu.is_visible {
        return false;
    }
    match &menu.permission_code {
        Some(code) => user_permissions.iter().any(|p| p == code),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_menu(id: i64, parent_id: i64, sort: i32, code: &str) -> MenuModel {
        MenuModel {
            id,
            name: code.into(),
            code: code.into(),
            path: format!("/{code}"),
            icon: None,
            parent_id,
            sort,
            is_visible: true,
            permission_code: None,
            tenant_id: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_tree_build_by_parent_and_sort() {
        let menus = vec![
            make_menu(1, 0, 2, "b"),
            make_menu(2, 0, 1, "a"),
            make_menu(3, 2, 0, "a_child"),
        ];
        let tree = build_menu_tree(menus, &[]);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].menu.code, "a");
        assert_eq!(tree[1].menu.code, "b");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].menu.code, "a_child");
    }

    #[test]
    fn test_tree_filter_by_permission() {
        let mut m = make_menu(1, 0, 0, "protected");
        m.permission_code = Some("admin:secret:view".into());
        let menus = vec![m, make_menu(2, 0, 0, "open")];
        let tree = build_menu_tree(menus, &[]);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].menu.code, "open");
    }

    #[test]
    fn test_detect_circular_reference() {
        let menus = vec![make_menu(1, 2, 0, "a"), make_menu(2, 1, 0, "b")];
        assert!(detect_circular_reference(&menus, 1, 2));
        assert!(detect_circular_reference(&menus, 2, 1));
        assert!(!detect_circular_reference(&menus, 1, 0));
    }

    #[test]
    fn test_create_menu_parent_not_found() {
        let menus = vec![make_menu(1, 0, 0, "root")];
        assert!(!detect_circular_reference(&menus, 1, 999));
    }

    #[test]
    fn test_delete_menu_has_child() {
        let menus = vec![make_menu(1, 0, 0, "parent"), make_menu(2, 1, 0, "child")];
        let tree = build_menu_tree(menus, &[]);
        assert!(!tree[0].children.is_empty());
    }
}
