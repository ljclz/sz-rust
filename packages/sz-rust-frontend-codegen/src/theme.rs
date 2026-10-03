// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//! 主题系统 — 继承链 + 运行时切换 + 循环检测（spec §5.3）
//!
//! v1.7.0 新增模块，通过 Cargo feature `plugin-theme` 控制。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

/// 默认继承深度上限（spec §6.3 规则 1）
pub const DEFAULT_MAX_DEPTH: usize = 10;

/// 主题标识（唯一）
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ThemeId(pub String);

impl ThemeId {
    /// 创建主题标识
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 获取标识字符串
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ThemeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 主题定义
#[derive(Debug, Clone)]
pub struct Theme {
    /// 主题标识
    pub id: ThemeId,
    /// 父主题标识（None 表示根主题）
    pub parent: Option<ThemeId>,
    /// 模板覆盖（模板名 → 模板内容）
    pub template_overrides: HashMap<String, String>,
    /// 样式覆盖
    pub style_overrides: HashMap<String, String>,
}

impl Theme {
    /// 创建根主题（无父主题）
    pub fn root(id: impl Into<String>) -> Self {
        Self {
            id: ThemeId::new(id),
            parent: None,
            template_overrides: HashMap::new(),
            style_overrides: HashMap::new(),
        }
    }

    /// 创建子主题
    pub fn child(id: impl Into<String>, parent: impl Into<String>) -> Self {
        Self {
            id: ThemeId::new(id),
            parent: Some(ThemeId::new(parent)),
            template_overrides: HashMap::new(),
            style_overrides: HashMap::new(),
        }
    }

    /// 添加模板覆盖
    pub fn with_template(mut self, name: impl Into<String>, content: impl Into<String>) -> Self {
        self.template_overrides.insert(name.into(), content.into());
        self
    }

    /// 添加样式覆盖
    pub fn with_style(mut self, name: impl Into<String>, content: impl Into<String>) -> Self {
        self.style_overrides.insert(name.into(), content.into());
        self
    }
}

/// 主题错误
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    /// 主题未找到
    #[error("主题未找到: {0}")]
    NotFound(String),
    /// 继承链循环
    #[error("主题继承循环: {0}")]
    InheritanceCycle(String),
    /// 继承深度超限（spec §6.3 规则 1，默认 ≤ 10）
    #[error("继承深度超限: {0} > {1}")]
    DepthExceeded(usize, usize),
}

/// 主题注册中心
pub struct ThemeRegistry {
    /// 主题表（ThemeId → Theme）
    themes: RwLock<HashMap<ThemeId, Arc<Theme>>>,
    /// 继承深度上限（默认 10）
    max_depth: usize,
}

impl ThemeRegistry {
    /// 创建主题注册中心
    pub fn new(max_depth: usize) -> Self {
        Self {
            themes: RwLock::new(HashMap::new()),
            max_depth,
        }
    }

    /// 使用默认深度上限创建
    pub fn with_default_depth() -> Self {
        Self::new(DEFAULT_MAX_DEPTH)
    }

    /// 注册主题，检测继承循环与深度
    pub fn register(&self, theme: Theme) -> Result<(), ThemeError> {
        let mut themes = self.themes.write();

        if let Some(ref parent_id) = theme.parent {
            if !themes.contains_key(parent_id) {
                return Err(ThemeError::NotFound(parent_id.to_string()));
            }
        }

        let theme_id = theme.id.clone();
        let theme_arc = Arc::new(theme);

        if self.would_create_cycle(&themes, &theme_id, &theme_arc.parent) {
            return Err(ThemeError::InheritanceCycle(theme_id.to_string()));
        }

        let depth = self.compute_depth(&themes, &theme_arc);
        if depth > self.max_depth {
            return Err(ThemeError::DepthExceeded(depth, self.max_depth));
        }

        themes.insert(theme_id, theme_arc);
        Ok(())
    }

    /// 解析主题继承链，返回从根到当前主题的有序列表
    pub fn resolve_chain(&self, id: &ThemeId) -> Result<Vec<Arc<Theme>>, ThemeError> {
        let themes = self.themes.read();
        let mut chain = Vec::new();
        let mut current = id.clone();
        let mut visited = std::collections::HashSet::new();

        loop {
            if !visited.insert(current.clone()) {
                return Err(ThemeError::InheritanceCycle(current.to_string()));
            }

            let theme = themes
                .get(&current)
                .ok_or_else(|| ThemeError::NotFound(current.to_string()))?;

            chain.push(theme.clone());

            match &theme.parent {
                Some(parent_id) => current = parent_id.clone(),
                None => break,
            }
        }

        chain.reverse();
        Ok(chain)
    }

    /// 查询模板（子主题覆盖优先，未覆盖继承父主题）
    pub fn lookup_template(&self, theme: &ThemeId, template_name: &str) -> Option<String> {
        let chain = self.resolve_chain(theme).ok()?;
        for theme in chain.iter().rev() {
            if let Some(content) = theme.template_overrides.get(template_name) {
                return Some(content.clone());
            }
        }
        None
    }

    /// 查询样式（子主题覆盖优先，未覆盖继承父主题）
    pub fn lookup_style(&self, theme: &ThemeId, style_name: &str) -> Option<String> {
        let chain = self.resolve_chain(theme).ok()?;
        for theme in chain.iter().rev() {
            if let Some(content) = theme.style_overrides.get(style_name) {
                return Some(content.clone());
            }
        }
        None
    }

    /// 获取主题
    pub fn get(&self, id: &ThemeId) -> Option<Arc<Theme>> {
        self.themes.read().get(id).cloned()
    }

    /// 已注册主题数
    pub fn len(&self) -> usize {
        self.themes.read().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.themes.read().is_empty()
    }

    fn would_create_cycle(
        &self,
        themes: &HashMap<ThemeId, Arc<Theme>>,
        new_id: &ThemeId,
        new_parent: &Option<ThemeId>,
    ) -> bool {
        let Some(parent_id) = new_parent else {
            return false;
        };

        let mut current = parent_id.clone();
        let mut visited = std::collections::HashSet::new();

        loop {
            if current == *new_id {
                return true;
            }

            if !visited.insert(current.clone()) {
                return true;
            }

            match themes.get(&current) {
                Some(theme) => match &theme.parent {
                    Some(parent_id) => current = parent_id.clone(),
                    None => return false,
                },
                None => return false,
            }
        }
    }

    fn compute_depth(&self, themes: &HashMap<ThemeId, Arc<Theme>>, theme: &Arc<Theme>) -> usize {
        let mut depth = 0;
        let mut current = theme.parent.clone();

        while let Some(parent_id) = current {
            depth += 1;
            match themes.get(&parent_id) {
                Some(parent_theme) => current = parent_theme.parent.clone(),
                None => break,
            }
        }
        depth
    }
}

/// 运行时主题切换器（新请求用新主题，进行中请求用原主题）
pub struct ThemeSwitcher {
    /// 当前主题
    current: Arc<RwLock<ThemeId>>,
    /// 主题注册中心
    registry: Arc<ThemeRegistry>,
}

impl ThemeSwitcher {
    /// 创建主题切换器
    pub fn new(registry: Arc<ThemeRegistry>, initial: ThemeId) -> Result<Self, ThemeError> {
        if registry.get(&initial).is_none() {
            return Err(ThemeError::NotFound(initial.to_string()));
        }
        Ok(Self {
            current: Arc::new(RwLock::new(initial)),
            registry,
        })
    }

    /// 切换主题（原子切换，进行中请求持有旧引用不受影响）
    pub fn switch(&self, new_theme: ThemeId) -> Result<(), ThemeError> {
        if self.registry.get(&new_theme).is_none() {
            return Err(ThemeError::NotFound(new_theme.to_string()));
        }
        let mut current = self.current.write();
        *current = new_theme;
        Ok(())
    }

    /// 获取当前主题快照
    pub fn current(&self) -> ThemeId {
        self.current.read().clone()
    }

    /// 获取注册中心引用
    pub fn registry(&self) -> &ThemeRegistry {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_root_theme() {
        let registry = ThemeRegistry::with_default_depth();
        let theme = Theme::root("base");
        assert!(registry.register(theme).is_ok());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_register_child_theme() {
        let registry = ThemeRegistry::with_default_depth();
        registry.register(Theme::root("base")).unwrap();
        registry.register(Theme::child("dark", "base")).unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_register_missing_parent() {
        let registry = ThemeRegistry::with_default_depth();
        let result = registry.register(Theme::child("dark", "nonexistent"));
        assert!(matches!(result, Err(ThemeError::NotFound(_))));
    }

    #[test]
    fn test_inheritance_cycle_detection() {
        let registry = ThemeRegistry::with_default_depth();
        registry.register(Theme::root("a")).unwrap();
        registry.register(Theme::child("b", "a")).unwrap();

        let result = registry.register(Theme {
            id: ThemeId::new("a"),
            parent: Some(ThemeId::new("b")),
            template_overrides: HashMap::new(),
            style_overrides: HashMap::new(),
        });
        assert!(matches!(result, Err(ThemeError::InheritanceCycle(_))));
    }

    #[test]
    fn test_resolve_chain() {
        let registry = ThemeRegistry::with_default_depth();
        registry.register(Theme::root("base")).unwrap();
        registry.register(Theme::child("mid", "base")).unwrap();
        registry.register(Theme::child("leaf", "mid")).unwrap();

        let chain = registry.resolve_chain(&ThemeId::new("leaf")).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].id, ThemeId::new("base"));
        assert_eq!(chain[1].id, ThemeId::new("mid"));
        assert_eq!(chain[2].id, ThemeId::new("leaf"));
    }

    #[test]
    fn test_resolve_chain_root() {
        let registry = ThemeRegistry::with_default_depth();
        registry.register(Theme::root("base")).unwrap();

        let chain = registry.resolve_chain(&ThemeId::new("base")).unwrap();
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn test_resolve_chain_not_found() {
        let registry = ThemeRegistry::with_default_depth();
        let result = registry.resolve_chain(&ThemeId::new("nonexistent"));
        assert!(matches!(result, Err(ThemeError::NotFound(_))));
    }

    #[test]
    fn test_lookup_template_child_override() {
        let registry = ThemeRegistry::with_default_depth();
        registry
            .register(Theme::root("base").with_template("header", "base header"))
            .unwrap();
        registry
            .register(Theme::child("dark", "base").with_template("header", "dark header"))
            .unwrap();

        let result = registry.lookup_template(&ThemeId::new("dark"), "header");
        assert_eq!(result.unwrap(), "dark header");
    }

    #[test]
    fn test_lookup_template_inherit_from_parent() {
        let registry = ThemeRegistry::with_default_depth();
        registry
            .register(Theme::root("base").with_template("footer", "base footer"))
            .unwrap();
        registry.register(Theme::child("dark", "base")).unwrap();

        let result = registry.lookup_template(&ThemeId::new("dark"), "footer");
        assert_eq!(result.unwrap(), "base footer");
    }

    #[test]
    fn test_lookup_template_not_found() {
        let registry = ThemeRegistry::with_default_depth();
        registry.register(Theme::root("base")).unwrap();

        let result = registry.lookup_template(&ThemeId::new("base"), "nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_lookup_style_inherit() {
        let registry = ThemeRegistry::with_default_depth();
        registry
            .register(Theme::root("base").with_style("color", "black"))
            .unwrap();
        registry
            .register(Theme::child("dark", "base").with_style("color", "white"))
            .unwrap();

        assert_eq!(
            registry
                .lookup_style(&ThemeId::new("dark"), "color")
                .unwrap(),
            "white"
        );
        assert_eq!(
            registry
                .lookup_style(&ThemeId::new("base"), "color")
                .unwrap(),
            "black"
        );
    }

    #[test]
    fn test_depth_exceeded() {
        let registry = ThemeRegistry::new(2);
        registry.register(Theme::root("l0")).unwrap();
        registry.register(Theme::child("l1", "l0")).unwrap();
        registry.register(Theme::child("l2", "l1")).unwrap();

        let result = registry.register(Theme::child("l3", "l2"));
        assert!(matches!(result, Err(ThemeError::DepthExceeded(_, _))));
    }

    #[test]
    fn test_theme_switcher() {
        let registry = Arc::new(ThemeRegistry::with_default_depth());
        registry.register(Theme::root("light")).unwrap();
        registry.register(Theme::root("dark")).unwrap();

        let switcher = ThemeSwitcher::new(registry.clone(), ThemeId::new("light")).unwrap();
        assert_eq!(switcher.current(), ThemeId::new("light"));

        switcher.switch(ThemeId::new("dark")).unwrap();
        assert_eq!(switcher.current(), ThemeId::new("dark"));
    }

    #[test]
    fn test_theme_switcher_invalid_initial() {
        let registry = Arc::new(ThemeRegistry::with_default_depth());
        let result = ThemeSwitcher::new(registry, ThemeId::new("nonexistent"));
        assert!(result.is_err());
    }

    #[test]
    fn test_theme_switcher_switch_to_nonexistent() {
        let registry = Arc::new(ThemeRegistry::with_default_depth());
        registry.register(Theme::root("base")).unwrap();

        let switcher = ThemeSwitcher::new(registry, ThemeId::new("base")).unwrap();
        let result = switcher.switch(ThemeId::new("nonexistent"));
        assert!(result.is_err());
    }

    #[test]
    fn test_multi_level_inheritance_lookup() {
        let registry = ThemeRegistry::with_default_depth();
        registry
            .register(Theme::root("base").with_template("t", "base-t"))
            .unwrap();
        registry
            .register(Theme::child("mid", "base").with_template("t", "mid-t"))
            .unwrap();
        registry.register(Theme::child("leaf", "mid")).unwrap();

        assert_eq!(
            registry
                .lookup_template(&ThemeId::new("leaf"), "t")
                .unwrap(),
            "mid-t"
        );
    }
}
