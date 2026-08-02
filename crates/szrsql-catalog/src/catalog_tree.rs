//! 层次化数据目录（P5）— 树状数据资产组织。
//!
//! 对应 `TDengine启发评估与改进规划.md` §十八。
//!
//! # 设计
//!
//! 在扁平 `HashMap<String, TableSchema>` 之上叠加一层**路径索引**，
//! 将表/视图组织为树状结构（类似文件系统目录）。
//!
//! - **`CatalogPath`**：路径类型，如 `/sales/orders`、`/hr/employees`
//! - **`CatalogNode`**：树节点（Root / Directory / Table / View）
//! - **`CatalogTree`**：树结构，提供路径解析、挂载、卸载、移动、列出子节点等操作
//!
//! # 与现有 Catalog 的关系
//!
//! `CatalogTree` **不替代** `MutableCatalog` trait，而是在其之上叠加路径组织层：
//! - 表元数据仍存储在 `ManagedCatalog` 中
//! - `CatalogTree` 仅维护 `path → table_name` 的映射关系
//! - 卸载路径不会删除表本身（需调用 `MutableCatalog::drop_table`）
//!
//! # 路径规范
//!
//! - 必须以 `/` 开头
//! - 路径段之间用 `/` 分隔
//! - 路径段不能包含 `/`、空字符
//! - 路径段不能为空（如 `//a` 非法）
//! - 根路径为 `/`
//! - 路径大小写敏感

use std::collections::HashMap;

// =====================================================================
//  错误类型
// =====================================================================

/// 数据目录错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogTreeError {
    /// 路径格式无效
    #[error("invalid path: {0}")]
    InvalidPath(String),
    /// 路径不存在
    #[error("path not found: {0}")]
    NotFound(String),
    /// 路径已存在
    #[error("path already exists: {0}")]
    AlreadyExists(String),
    /// 路径非空（删除目录时目录下仍有子节点）
    #[error("directory not empty: {0}")]
    DirectoryNotEmpty(String),
    /// 节点类型不匹配（如对表节点调用 create_dir）
    #[error("node type mismatch: expected {expected}, found {found} at {path}")]
    NodeTypeMismatch {
        expected: String,
        found: String,
        path: String,
    },
    /// 不能操作根节点
    #[error("cannot operate on root node")]
    CannotOperateRoot,
}

// =====================================================================
//  节点类型
// =====================================================================

/// 节点类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// 根节点（/）
    Root,
    /// 目录节点（可包含子节点）
    Directory,
    /// 表节点（叶子，关联一个表名）
    Table,
    /// 视图节点（叶子，关联一个视图名）
    View,
}

impl NodeKind {
    /// 是否为叶子节点
    pub fn is_leaf(&self) -> bool {
        matches!(self, NodeKind::Table | NodeKind::View)
    }

    /// 是否为目录节点（含根）
    pub fn is_directory(&self) -> bool {
        matches!(self, NodeKind::Root | NodeKind::Directory)
    }

    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeKind::Root => "root",
            NodeKind::Directory => "directory",
            NodeKind::Table => "table",
            NodeKind::View => "view",
        }
    }
}

/// 树节点
#[derive(Debug, Clone)]
pub struct CatalogNode {
    /// 节点 ID（内部自增）
    pub node_id: u32,
    /// 节点类型
    pub kind: NodeKind,
    /// 节点名称（路径最后一段，根节点为空）
    pub name: String,
    /// 父节点 ID（根节点为 None）
    pub parent_id: Option<u32>,
    /// 子节点 ID 列表（仅 Directory/Root 有）
    pub children: Vec<u32>,
    /// 关联的表名/视图名（仅 Table/View 有）
    pub table_name: Option<String>,
}

impl CatalogNode {
    fn new_root(node_id: u32) -> Self {
        Self {
            node_id,
            kind: NodeKind::Root,
            name: String::new(),
            parent_id: None,
            children: Vec::new(),
            table_name: None,
        }
    }

    fn new_directory(node_id: u32, name: String, parent_id: u32) -> Self {
        Self {
            node_id,
            kind: NodeKind::Directory,
            name,
            parent_id: Some(parent_id),
            children: Vec::new(),
            table_name: None,
        }
    }

    fn new_table(node_id: u32, name: String, parent_id: u32, table_name: String) -> Self {
        Self {
            node_id,
            kind: NodeKind::Table,
            name,
            parent_id: Some(parent_id),
            children: Vec::new(),
            table_name: Some(table_name),
        }
    }

    fn new_view(node_id: u32, name: String, parent_id: u32, view_name: String) -> Self {
        Self {
            node_id,
            kind: NodeKind::View,
            name,
            parent_id: Some(parent_id),
            children: Vec::new(),
            table_name: Some(view_name),
        }
    }
}

// =====================================================================
//  路径类型
// =====================================================================

/// 路径类型，封装规范化后的路径字符串
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CatalogPath {
    inner: String,
    segments: Vec<String>,
}

impl CatalogPath {
    /// 从字符串解析路径
    ///
    /// 规则：
    /// - 必须以 `/` 开头
    /// - 路径段不能为空（如 `//a` 非法）
    /// - 路径段不能包含空字符
    /// - 根路径为 `/`
    /// - 尾部 `/` 会被规范化（`/a/b/` → `/a/b`）
    pub fn parse(path: &str) -> Result<Self, CatalogTreeError> {
        if path.is_empty() || !path.starts_with('/') {
            return Err(CatalogTreeError::InvalidPath(path.to_string()));
        }

        // 根路径
        if path == "/" {
            return Ok(Self {
                inner: "/".to_string(),
                segments: Vec::new(),
            });
        }

        // 去除尾部 `/` 后再校验（避免尾部 `/` 被识别为空段）
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            return Ok(Self::root());
        }

        // 校验路径段
        let segments: Vec<String> = trimmed.split('/').skip(1).map(String::from).collect();
        for seg in &segments {
            if seg.is_empty() {
                return Err(CatalogTreeError::InvalidPath(path.to_string()));
            }
            if seg.contains('\0') {
                return Err(CatalogTreeError::InvalidPath(path.to_string()));
            }
        }

        Ok(Self {
            inner: trimmed.to_string(),
            segments,
        })
    }

    /// 根路径
    pub fn root() -> Self {
        Self {
            inner: "/".to_string(),
            segments: Vec::new(),
        }
    }

    /// 是否为根路径
    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// 路径段
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// 父路径
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        if self.segments.len() == 1 {
            return Some(Self::root());
        }
        let parent_segs = &self.segments[..self.segments.len() - 1];
        let inner = format!("/{}", parent_segs.join("/"));
        Some(Self {
            inner,
            segments: parent_segs.to_vec(),
        })
    }

    /// 末段名称（根节点返回空字符串）
    pub fn name(&self) -> &str {
        self.segments.last().map(String::as_str).unwrap_or("")
    }

    /// 路径字符串
    pub fn as_str(&self) -> &str {
        &self.inner
    }
}

impl std::fmt::Display for CatalogPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

// =====================================================================
//  树结构
// =====================================================================

/// 层次化数据目录树
#[derive(Debug, Default)]
pub struct CatalogTree {
    /// 节点 ID → 节点
    nodes: HashMap<u32, CatalogNode>,
    /// 路径字符串 → 节点 ID
    path_index: HashMap<String, u32>,
    /// 表名 → 节点 ID（用于反向查找）
    table_index: HashMap<String, u32>,
    /// 下一个节点 ID
    next_id: u32,
}

impl CatalogTree {
    /// 创建空树（仅含根节点）
    pub fn new() -> Self {
        let mut tree = Self::default();
        let root = CatalogNode::new_root(0);
        tree.nodes.insert(0, root);
        tree.path_index.insert("/".to_string(), 0);
        tree.next_id = 1;
        tree
    }

    /// 节点数（含根）
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 是否为空树（仅根节点）
    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// 根节点 ID（恒为 0）
    pub fn root_id(&self) -> u32 {
        0
    }

    /// 获取节点
    pub fn get_node(&self, node_id: u32) -> Option<&CatalogNode> {
        self.nodes.get(&node_id)
    }

    /// 通过路径获取节点
    pub fn get_node_by_path(&self, path: &str) -> Option<&CatalogNode> {
        let normalized = normalize_path_str(path)?;
        let node_id = self.path_index.get(&normalized)?;
        self.nodes.get(node_id)
    }

    /// 获取节点的完整路径
    pub fn get_path(&self, node_id: u32) -> Option<String> {
        let node = self.nodes.get(&node_id)?;
        if node.kind == NodeKind::Root {
            return Some("/".to_string());
        }
        let mut segs = vec![node.name.clone()];
        let mut current = node.parent_id;
        while let Some(pid) = current {
            let p = self.nodes.get(&pid)?;
            if p.kind == NodeKind::Root {
                break;
            }
            segs.push(p.name.clone());
            current = p.parent_id;
        }
        segs.reverse();
        Some(format!("/{}", segs.join("/")))
    }

    /// 创建目录节点
    pub fn create_dir(&mut self, path: &str) -> Result<u32, CatalogTreeError> {
        let catalog_path = CatalogPath::parse(path)?;
        if catalog_path.is_root() {
            return Err(CatalogTreeError::CannotOperateRoot);
        }
        let normalized = catalog_path.as_str().to_string();
        if self.path_index.contains_key(&normalized) {
            return Err(CatalogTreeError::AlreadyExists(normalized));
        }
        let parent_path = catalog_path
            .parent()
            .ok_or_else(|| CatalogTreeError::InvalidPath(path.to_string()))?;
        let parent_id = self
            .path_index
            .get(parent_path.as_str())
            .copied()
            .ok_or_else(|| CatalogTreeError::NotFound(parent_path.as_str().to_string()))?;
        let parent = self
            .nodes
            .get(&parent_id)
            .ok_or_else(|| CatalogTreeError::NotFound(parent_path.as_str().to_string()))?;
        if !parent.kind.is_directory() {
            return Err(CatalogTreeError::NodeTypeMismatch {
                expected: "directory".to_string(),
                found: parent.kind.as_str().to_string(),
                path: parent_path.as_str().to_string(),
            });
        }
        let node_id = self.next_id;
        self.next_id += 1;
        let name = catalog_path.name().to_string();
        let node = CatalogNode::new_directory(node_id, name, parent_id);
        self.nodes.insert(node_id, node);
        self.path_index.insert(normalized, node_id);
        // 加入父节点 children
        if let Some(p) = self.nodes.get_mut(&parent_id) {
            p.children.push(node_id);
        }
        Ok(node_id)
    }

    /// 挂载表节点到指定路径
    pub fn mount_table(&mut self, path: &str, table_name: &str) -> Result<u32, CatalogTreeError> {
        self.mount_leaf(path, table_name, NodeKind::Table)
    }

    /// 挂载视图节点到指定路径
    pub fn mount_view(&mut self, path: &str, view_name: &str) -> Result<u32, CatalogTreeError> {
        self.mount_leaf(path, view_name, NodeKind::View)
    }

    fn mount_leaf(
        &mut self,
        path: &str,
        table_name: &str,
        kind: NodeKind,
    ) -> Result<u32, CatalogTreeError> {
        let catalog_path = CatalogPath::parse(path)?;
        if catalog_path.is_root() {
            return Err(CatalogTreeError::CannotOperateRoot);
        }
        let normalized = catalog_path.as_str().to_string();
        if self.path_index.contains_key(&normalized) {
            return Err(CatalogTreeError::AlreadyExists(normalized));
        }
        // 同一表名不能重复挂载
        if self.table_index.contains_key(table_name) {
            return Err(CatalogTreeError::AlreadyExists(format!(
                "table already mounted: {}",
                table_name
            )));
        }
        let parent_path = catalog_path
            .parent()
            .ok_or_else(|| CatalogTreeError::InvalidPath(path.to_string()))?;
        let parent_id = self
            .path_index
            .get(parent_path.as_str())
            .copied()
            .ok_or_else(|| CatalogTreeError::NotFound(parent_path.as_str().to_string()))?;
        let parent = self
            .nodes
            .get(&parent_id)
            .ok_or_else(|| CatalogTreeError::NotFound(parent_path.as_str().to_string()))?;
        if !parent.kind.is_directory() {
            return Err(CatalogTreeError::NodeTypeMismatch {
                expected: "directory".to_string(),
                found: parent.kind.as_str().to_string(),
                path: parent_path.as_str().to_string(),
            });
        }
        let node_id = self.next_id;
        self.next_id += 1;
        let name = catalog_path.name().to_string();
        let node = match kind {
            NodeKind::Table => {
                CatalogNode::new_table(node_id, name, parent_id, table_name.to_string())
            }
            NodeKind::View => {
                CatalogNode::new_view(node_id, name, parent_id, table_name.to_string())
            }
            _ => unreachable!(),
        };
        self.nodes.insert(node_id, node);
        self.path_index.insert(normalized, node_id);
        self.table_index.insert(table_name.to_string(), node_id);
        if let Some(p) = self.nodes.get_mut(&parent_id) {
            p.children.push(node_id);
        }
        Ok(node_id)
    }

    /// 卸载节点（不删除表本身）
    pub fn unmount(&mut self, path: &str) -> Result<(), CatalogTreeError> {
        let catalog_path = CatalogPath::parse(path)?;
        if catalog_path.is_root() {
            return Err(CatalogTreeError::CannotOperateRoot);
        }
        let normalized = catalog_path.as_str().to_string();
        let node_id = self
            .path_index
            .get(&normalized)
            .copied()
            .ok_or_else(|| CatalogTreeError::NotFound(normalized.clone()))?;
        let node = self
            .nodes
            .get(&node_id)
            .ok_or_else(|| CatalogTreeError::NotFound(normalized.clone()))?
            .clone();
        // 目录非空检查
        if node.kind.is_directory() && !node.children.is_empty() {
            return Err(CatalogTreeError::DirectoryNotEmpty(normalized));
        }
        // 从父节点 children 移除
        if let Some(pid) = node.parent_id {
            if let Some(p) = self.nodes.get_mut(&pid) {
                p.children.retain(|&c| c != node_id);
            }
        }
        // 清理索引
        self.path_index.remove(&normalized);
        if let Some(table_name) = &node.table_name {
            self.table_index.remove(table_name);
        }
        self.nodes.remove(&node_id);
        Ok(())
    }

    /// 列出子节点
    pub fn list_children(&self, path: &str) -> Result<Vec<&CatalogNode>, CatalogTreeError> {
        let catalog_path = CatalogPath::parse(path)?;
        let normalized = catalog_path.as_str().to_string();
        let node_id = self
            .path_index
            .get(&normalized)
            .copied()
            .ok_or_else(|| CatalogTreeError::NotFound(normalized.clone()))?;
        let node = self
            .nodes
            .get(&node_id)
            .ok_or_else(|| CatalogTreeError::NotFound(normalized.clone()))?;
        if !node.kind.is_directory() {
            return Err(CatalogTreeError::NodeTypeMismatch {
                expected: "directory".to_string(),
                found: node.kind.as_str().to_string(),
                path: normalized,
            });
        }
        let mut result = Vec::new();
        for &cid in &node.children {
            if let Some(child) = self.nodes.get(&cid) {
                result.push(child);
            }
        }
        // 按名称排序
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }

    /// 移动节点（修改父节点）
    pub fn move_node(&mut self, src: &str, dst_parent: &str) -> Result<(), CatalogTreeError> {
        let src_path = CatalogPath::parse(src)?;
        let dst_path = CatalogPath::parse(dst_parent)?;
        if src_path.is_root() {
            return Err(CatalogTreeError::CannotOperateRoot);
        }
        let src_normalized = src_path.as_str().to_string();
        let src_id = self
            .path_index
            .get(&src_normalized)
            .copied()
            .ok_or_else(|| CatalogTreeError::NotFound(src_normalized.clone()))?;
        let src_node = self
            .nodes
            .get(&src_id)
            .ok_or_else(|| CatalogTreeError::NotFound(src_normalized.clone()))?
            .clone();
        // 不能移动到自己下面
        if dst_path.as_str().starts_with(src_path.as_str()) {
            return Err(CatalogTreeError::InvalidPath(format!(
                "cannot move {} into its own subtree",
                src_normalized
            )));
        }
        let dst_normalized = dst_path.as_str().to_string();
        let dst_id = self
            .path_index
            .get(&dst_normalized)
            .copied()
            .ok_or_else(|| CatalogTreeError::NotFound(dst_normalized.clone()))?;
        let dst_node = self
            .nodes
            .get(&dst_id)
            .ok_or_else(|| CatalogTreeError::NotFound(dst_normalized.clone()))?;
        if !dst_node.kind.is_directory() {
            return Err(CatalogTreeError::NodeTypeMismatch {
                expected: "directory".to_string(),
                found: dst_node.kind.as_str().to_string(),
                path: dst_normalized,
            });
        }
        // 新路径 = dst_parent + src.name
        let new_path = if dst_path.is_root() {
            format!("/{}", src_node.name)
        } else {
            format!("{}/{}", dst_path.as_str(), src_node.name)
        };
        if self.path_index.contains_key(&new_path) {
            return Err(CatalogTreeError::AlreadyExists(new_path));
        }
        // 从旧父节点 children 移除
        if let Some(old_pid) = src_node.parent_id {
            if let Some(p) = self.nodes.get_mut(&old_pid) {
                p.children.retain(|&c| c != src_id);
            }
        }
        // 加入新父节点 children
        if let Some(p) = self.nodes.get_mut(&dst_id) {
            p.children.push(src_id);
        }
        // 更新节点的 parent_id
        if let Some(n) = self.nodes.get_mut(&src_id) {
            n.parent_id = Some(dst_id);
        }
        // 更新 path_index（递归更新子树）
        let old_prefix = src_normalized.clone();
        let new_prefix = new_path.clone();
        let mut updates: Vec<(String, u32)> = Vec::new();
        for (path, &nid) in self.path_index.iter() {
            if path == &old_prefix || path.starts_with(&format!("{}/", old_prefix)) {
                let suffix = &path[old_prefix.len()..];
                let new_p = format!("{}{}", new_prefix, suffix);
                updates.push((new_p, nid));
            }
        }
        // 先删除旧路径
        let keys_to_remove: Vec<String> = self
            .path_index
            .keys()
            .filter(|p| *p == &old_prefix || p.starts_with(&format!("{}/", old_prefix)))
            .cloned()
            .collect();
        for k in keys_to_remove {
            self.path_index.remove(&k);
        }
        // 插入新路径
        for (p, nid) in updates {
            self.path_index.insert(p, nid);
        }
        Ok(())
    }

    /// 通过表名查找节点
    pub fn find_by_table_name(&self, table_name: &str) -> Option<&CatalogNode> {
        let node_id = self.table_index.get(table_name)?;
        self.nodes.get(node_id)
    }

    /// 通过表名查找路径
    pub fn find_path_by_table_name(&self, table_name: &str) -> Option<String> {
        let node_id = self.table_index.get(table_name)?;
        self.get_path(*node_id)
    }

    /// 整树视图（BFS 遍历）
    pub fn tree_view(&self) -> Vec<TreeEntry> {
        use std::collections::VecDeque;
        let mut result = Vec::new();
        let mut queue: VecDeque<(u32, usize)> = VecDeque::new();
        queue.push_back((0, 0)); // (node_id, depth)
        while let Some((nid, depth)) = queue.pop_front() {
            if let Some(node) = self.nodes.get(&nid) {
                let path = self.get_path(nid).unwrap_or_default();
                result.push(TreeEntry {
                    path,
                    name: node.name.clone(),
                    kind: node.kind.clone(),
                    depth,
                    table_name: node.table_name.clone(),
                });
                // 子节点入队（按名称排序保证输出顺序稳定）
                let mut children: Vec<u32> = node.children.clone();
                children.sort_by(|&a, &b| {
                    let na = self.nodes.get(&a).map(|n| n.name.as_str()).unwrap_or("");
                    let nb = self.nodes.get(&b).map(|n| n.name.as_str()).unwrap_or("");
                    na.cmp(nb)
                });
                for cid in children {
                    queue.push_back((cid, depth + 1));
                }
            }
        }
        result
    }
}

/// 树视图条目
#[derive(Debug, Clone)]
pub struct TreeEntry {
    /// 完整路径
    pub path: String,
    /// 节点名称
    pub name: String,
    /// 节点类型
    pub kind: NodeKind,
    /// 深度（根为 0）
    pub depth: usize,
    /// 关联表名/视图名（叶子节点）
    pub table_name: Option<String>,
}

/// 规范化路径字符串（用于 path_index 查找）
fn normalize_path_str(path: &str) -> Option<String> {
    let cp = CatalogPath::parse(path).ok()?;
    Some(cp.as_str().to_string())
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_parse_root() {
        let p = CatalogPath::parse("/").unwrap();
        assert!(p.is_root());
        assert_eq!(p.as_str(), "/");
        assert_eq!(p.segments().len(), 0);
        assert_eq!(p.name(), "");
        assert_eq!(p.parent(), None);
    }

    #[test]
    fn test_path_parse_simple() {
        let p = CatalogPath::parse("/sales").unwrap();
        assert!(!p.is_root());
        assert_eq!(p.as_str(), "/sales");
        assert_eq!(p.segments(), &["sales"]);
        assert_eq!(p.name(), "sales");
        assert_eq!(p.parent().unwrap().as_str(), "/");
    }

    #[test]
    fn test_path_parse_nested() {
        let p = CatalogPath::parse("/sales/orders").unwrap();
        assert_eq!(p.as_str(), "/sales/orders");
        assert_eq!(p.segments(), &["sales", "orders"]);
        assert_eq!(p.name(), "orders");
        assert_eq!(p.parent().unwrap().as_str(), "/sales");
    }

    #[test]
    fn test_path_parse_trailing_slash_normalized() {
        let p = CatalogPath::parse("/sales/orders/").unwrap();
        assert_eq!(p.as_str(), "/sales/orders");
    }

    #[test]
    fn test_path_parse_invalid() {
        assert!(CatalogPath::parse("").is_err());
        assert!(CatalogPath::parse("sales").is_err());
        assert!(CatalogPath::parse("//a").is_err());
        assert!(CatalogPath::parse("/a//b").is_err());
        assert!(CatalogPath::parse("/a\0b").is_err());
    }

    #[test]
    fn test_tree_new_only_root() {
        let tree = CatalogTree::new();
        assert_eq!(tree.len(), 1);
        assert!(tree.is_empty());
        assert_eq!(tree.root_id(), 0);
        let root = tree.get_node(0).unwrap();
        assert_eq!(root.kind, NodeKind::Root);
        assert!(root.children.is_empty());
    }

    #[test]
    fn test_create_dir_simple() {
        let mut tree = CatalogTree::new();
        let nid = tree.create_dir("/sales").unwrap();
        assert!(nid > 0);
        assert_eq!(tree.len(), 2);
        let node = tree.get_node(nid).unwrap();
        assert_eq!(node.kind, NodeKind::Directory);
        assert_eq!(node.name, "sales");
        assert_eq!(node.parent_id, Some(0));
        // 根节点 children 已包含
        let root = tree.get_node(0).unwrap();
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0], nid);
        // path_index 已建立
        assert_eq!(tree.get_node_by_path("/sales").unwrap().node_id, nid);
        assert_eq!(tree.get_path(nid).unwrap(), "/sales");
    }

    #[test]
    fn test_create_dir_nested() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/sales").unwrap();
        let orders_id = tree.create_dir("/sales/orders").unwrap();
        assert_eq!(tree.get_path(orders_id).unwrap(), "/sales/orders");
        let sales = tree.get_node_by_path("/sales").unwrap();
        assert_eq!(sales.children.len(), 1);
        assert_eq!(sales.children[0], orders_id);
    }

    #[test]
    fn test_create_dir_already_exists() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/sales").unwrap();
        let err = tree.create_dir("/sales").unwrap_err();
        assert!(matches!(err, CatalogTreeError::AlreadyExists(_)));
    }

    #[test]
    fn test_create_dir_parent_not_found() {
        let mut tree = CatalogTree::new();
        let err = tree.create_dir("/a/b").unwrap_err();
        assert!(matches!(err, CatalogTreeError::NotFound(_)));
    }

    #[test]
    fn test_create_dir_on_root_fails() {
        let mut tree = CatalogTree::new();
        let err = tree.create_dir("/").unwrap_err();
        assert!(matches!(err, CatalogTreeError::CannotOperateRoot));
    }

    #[test]
    fn test_mount_table_simple() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/sales").unwrap();
        let tid = tree.mount_table("/sales/orders", "orders").unwrap();
        let node = tree.get_node(tid).unwrap();
        assert_eq!(node.kind, NodeKind::Table);
        assert_eq!(node.name, "orders");
        assert_eq!(node.table_name.as_deref(), Some("orders"));
        assert_eq!(tree.get_path(tid).unwrap(), "/sales/orders");
        assert_eq!(
            tree.find_path_by_table_name("orders").unwrap(),
            "/sales/orders"
        );
    }

    #[test]
    fn test_mount_table_duplicate_table_name() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/a").unwrap();
        tree.create_dir("/b").unwrap();
        tree.mount_table("/a/t1", "t1").unwrap();
        let err = tree.mount_table("/b/t1", "t1").unwrap_err();
        assert!(matches!(err, CatalogTreeError::AlreadyExists(_)));
    }

    #[test]
    fn test_mount_view() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/reports").unwrap();
        let vid = tree.mount_view("/reports/summary", "v_summary").unwrap();
        let node = tree.get_node(vid).unwrap();
        assert_eq!(node.kind, NodeKind::View);
        assert_eq!(node.table_name.as_deref(), Some("v_summary"));
    }

    #[test]
    fn test_unmount_leaf() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/sales").unwrap();
        tree.mount_table("/sales/orders", "orders").unwrap();
        assert_eq!(tree.len(), 3);
        tree.unmount("/sales/orders").unwrap();
        assert_eq!(tree.len(), 2);
        assert!(tree.get_node_by_path("/sales/orders").is_none());
        assert!(tree.find_by_table_name("orders").is_none());
        // 父节点 children 已清理
        let sales = tree.get_node_by_path("/sales").unwrap();
        assert!(sales.children.is_empty());
    }

    #[test]
    fn test_unmount_directory_not_empty() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/sales").unwrap();
        tree.mount_table("/sales/orders", "orders").unwrap();
        let err = tree.unmount("/sales").unwrap_err();
        assert!(matches!(err, CatalogTreeError::DirectoryNotEmpty(_)));
    }

    #[test]
    fn test_unmount_empty_directory() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/sales").unwrap();
        tree.unmount("/sales").unwrap();
        assert_eq!(tree.len(), 1);
        assert!(tree.get_node_by_path("/sales").is_none());
    }

    #[test]
    fn test_unmount_root_fails() {
        let mut tree = CatalogTree::new();
        let err = tree.unmount("/").unwrap_err();
        assert!(matches!(err, CatalogTreeError::CannotOperateRoot));
    }

    #[test]
    fn test_list_children() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/sales").unwrap();
        tree.create_dir("/hr").unwrap();
        tree.mount_table("/sales/orders", "orders").unwrap();
        tree.mount_table("/sales/items", "items").unwrap();
        let children = tree.list_children("/").unwrap();
        assert_eq!(children.len(), 2);
        // 按名称排序：hr 在 sales 之前
        assert_eq!(children[0].name, "hr");
        assert_eq!(children[1].name, "sales");
        let sales_children = tree.list_children("/sales").unwrap();
        assert_eq!(sales_children.len(), 2);
        assert_eq!(sales_children[0].name, "items");
        assert_eq!(sales_children[1].name, "orders");
    }

    #[test]
    fn test_list_children_on_leaf_fails() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/sales").unwrap();
        tree.mount_table("/sales/orders", "orders").unwrap();
        let err = tree.list_children("/sales/orders").unwrap_err();
        assert!(matches!(err, CatalogTreeError::NodeTypeMismatch { .. }));
    }

    #[test]
    fn test_move_node_simple() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/a").unwrap();
        tree.create_dir("/b").unwrap();
        tree.mount_table("/a/t1", "t1").unwrap();
        // 移动 /a/t1 → /b/t1
        tree.move_node("/a/t1", "/b").unwrap();
        assert_eq!(tree.find_path_by_table_name("t1").unwrap(), "/b/t1");
        assert!(tree.get_node_by_path("/a/t1").is_none());
        assert!(tree.get_node_by_path("/b/t1").is_some());
        // 旧父节点 children 已清理
        let a = tree.get_node_by_path("/a").unwrap();
        assert!(a.children.is_empty());
        let b = tree.get_node_by_path("/b").unwrap();
        assert_eq!(b.children.len(), 1);
    }

    #[test]
    fn test_move_node_into_own_subtree_fails() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/a").unwrap();
        tree.create_dir("/a/b").unwrap();
        let err = tree.move_node("/a", "/a/b").unwrap_err();
        assert!(matches!(err, CatalogTreeError::InvalidPath(_)));
    }

    #[test]
    fn test_move_node_updates_subtree_paths() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/a").unwrap();
        tree.create_dir("/a/b").unwrap();
        tree.mount_table("/a/b/t1", "t1").unwrap();
        tree.create_dir("/c").unwrap();
        // 移动 /a → /c/a
        tree.move_node("/a", "/c").unwrap();
        assert!(tree.get_node_by_path("/a").is_none());
        assert!(tree.get_node_by_path("/c/a").is_some());
        assert!(tree.get_node_by_path("/c/a/b").is_some());
        assert_eq!(tree.find_path_by_table_name("t1").unwrap(), "/c/a/b/t1");
    }

    #[test]
    fn test_move_to_root_works() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/parent").unwrap();
        tree.create_dir("/parent/child").unwrap();
        // 将 child 提到根
        tree.move_node("/parent/child", "/").unwrap();
        assert!(tree.get_node_by_path("/child").is_some());
        assert!(tree.get_node_by_path("/parent/child").is_none());
    }

    #[test]
    fn test_tree_view_bfs() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/b").unwrap();
        tree.create_dir("/a").unwrap();
        tree.mount_table("/a/t1", "t1").unwrap();
        tree.mount_view("/b/v1", "v1").unwrap();
        let view = tree.tree_view();
        // BFS 顺序：根 → a → b → t1 → v1（同层全部输出后再进入下一层，同层按名称排序）
        assert_eq!(view.len(), 5);
        assert_eq!(view[0].path, "/");
        assert_eq!(view[0].depth, 0);
        assert_eq!(view[1].path, "/a");
        assert_eq!(view[1].depth, 1);
        assert_eq!(view[2].path, "/b");
        assert_eq!(view[2].depth, 1);
        assert_eq!(view[3].path, "/a/t1");
        assert_eq!(view[3].depth, 2);
        assert_eq!(view[3].kind, NodeKind::Table);
        assert_eq!(view[4].path, "/b/v1");
        assert_eq!(view[4].depth, 2);
        assert_eq!(view[4].kind, NodeKind::View);
    }

    #[test]
    fn test_node_kind_helpers() {
        assert!(NodeKind::Root.is_directory());
        assert!(NodeKind::Directory.is_directory());
        assert!(!NodeKind::Table.is_directory());
        assert!(!NodeKind::View.is_directory());
        assert!(NodeKind::Table.is_leaf());
        assert!(NodeKind::View.is_leaf());
        assert!(!NodeKind::Root.is_leaf());
        assert!(!NodeKind::Directory.is_leaf());
        assert_eq!(NodeKind::Root.as_str(), "root");
        assert_eq!(NodeKind::Directory.as_str(), "directory");
        assert_eq!(NodeKind::Table.as_str(), "table");
        assert_eq!(NodeKind::View.as_str(), "view");
    }

    #[test]
    fn test_get_path_root() {
        let tree = CatalogTree::new();
        assert_eq!(tree.get_path(0).unwrap(), "/");
    }

    #[test]
    fn test_find_by_table_name() {
        let mut tree = CatalogTree::new();
        tree.create_dir("/sales").unwrap();
        tree.mount_table("/sales/orders", "orders").unwrap();
        let node = tree.find_by_table_name("orders").unwrap();
        assert_eq!(node.kind, NodeKind::Table);
        assert_eq!(node.table_name.as_deref(), Some("orders"));
        assert!(tree.find_by_table_name("nonexistent").is_none());
    }

    #[test]
    fn test_complex_scenario_lifecycle() {
        // 模拟完整的数据资产组织生命周期
        let mut tree = CatalogTree::new();

        // 1. 创建业务域目录
        tree.create_dir("/sales").unwrap();
        tree.create_dir("/hr").unwrap();
        tree.create_dir("/finance").unwrap();

        // 2. 挂载表
        tree.mount_table("/sales/orders", "orders").unwrap();
        tree.mount_table("/sales/order_items", "order_items")
            .unwrap();
        tree.mount_table("/hr/employees", "employees").unwrap();
        tree.mount_table("/hr/departments", "departments").unwrap();
        tree.mount_table("/finance/invoices", "invoices").unwrap();

        // 3. 验证结构
        assert_eq!(tree.len(), 9); // 根 + 3 目录 + 5 表
        let sales_children = tree.list_children("/sales").unwrap();
        assert_eq!(sales_children.len(), 2);

        // 4. 重组：将 order_items 移到 hr（模拟业务调整）
        tree.move_node("/sales/order_items", "/hr").unwrap();
        assert_eq!(
            tree.find_path_by_table_name("order_items").unwrap(),
            "/hr/order_items"
        );

        // 5. 创建视图
        tree.mount_view("/sales/monthly_summary", "v_monthly")
            .unwrap();

        // 6. 卸载已删除的表
        tree.unmount("/finance/invoices").unwrap();
        assert!(tree.find_by_table_name("invoices").is_none());

        // 7. 验证最终结构
        let view = tree.tree_view();
        let table_count = view.iter().filter(|e| e.kind == NodeKind::Table).count();
        let view_count = view.iter().filter(|e| e.kind == NodeKind::View).count();
        let dir_count = view
            .iter()
            .filter(|e| e.kind == NodeKind::Directory || e.kind == NodeKind::Root)
            .count();
        assert_eq!(table_count, 4); // orders, employees, departments, order_items
        assert_eq!(view_count, 1); // v_monthly
        assert_eq!(dir_count, 4); // 根 + sales + hr + finance
    }
}
