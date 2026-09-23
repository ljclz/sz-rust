use std::sync::Arc;

use arc_swap::ArcSwap;

#[derive(Debug, Clone)]
pub struct RouteRule {
    pub method: String,
    pub path: String,
    pub handler_name: String,
}

#[derive(Debug, Clone)]
pub struct RouterTable {
    pub rules: Vec<RouteRule>,
    pub version: u64,
}

impl RouterTable {
    pub fn new(rules: Vec<RouteRule>) -> Self {
        Self { rules, version: 0 }
    }

    pub fn find(&self, method: &str, path: &str) -> Option<&RouteRule> {
        self.rules
            .iter()
            .find(|r| r.method == method && r.path == path)
    }
}

pub struct ArcSwapRouterTable {
    inner: ArcSwap<RouterTable>,
}

impl ArcSwapRouterTable {
    pub fn new(rules: Vec<RouteRule>) -> Self {
        Self {
            inner: ArcSwap::from_pointee(RouterTable::new(rules)),
        }
    }

    pub fn load(&self) -> arc_swap::Guard<Arc<RouterTable>> {
        self.inner.load()
    }

    pub fn store(&self, table: RouterTable) {
        self.inner.store(Arc::new(table));
    }

    pub fn hot_reload(&self, new_rules: Vec<RouteRule>) -> u64 {
        let old = self.load();
        let new_version = old.version + 1;
        let new_table = RouterTable {
            rules: new_rules,
            version: new_version,
        };
        self.store(new_table);
        new_version
    }

    pub fn version(&self) -> u64 {
        self.load().version
    }
}

impl Default for ArcSwapRouterTable {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(method: &str, path: &str) -> RouteRule {
        RouteRule {
            method: method.to_string(),
            path: path.to_string(),
            handler_name: format!("{method} {path}"),
        }
    }

    #[test]
    fn test_arc_swap_load_store() {
        let table = ArcSwapRouterTable::new(vec![make_rule("GET", "/users")]);

        let guard = table.load();
        assert_eq!(guard.version, 0);
        assert_eq!(guard.rules.len(), 1);
        assert!(guard.find("GET", "/users").is_some());
        assert!(guard.find("POST", "/users").is_none());
    }

    #[test]
    fn test_arc_swap_hot_reload() {
        let table = ArcSwapRouterTable::new(vec![make_rule("GET", "/users")]);
        assert_eq!(table.version(), 0);

        let new_version = table.hot_reload(vec![
            make_rule("GET", "/users"),
            make_rule("POST", "/users"),
            make_rule("DELETE", "/users/:id"),
        ]);
        assert_eq!(new_version, 1);
        assert_eq!(table.version(), 1);

        let guard = table.load();
        assert_eq!(guard.rules.len(), 3);
        assert!(guard.find("DELETE", "/users/:id").is_some());
    }

    #[test]
    fn test_arc_swap_concurrent_read_during_update() {
        use std::sync::Barrier;
        use std::thread;

        let table = Arc::new(ArcSwapRouterTable::new(vec![make_rule("GET", "/a")]));
        let barrier = Arc::new(Barrier::new(2));

        let reader_table = table.clone();
        let reader_barrier = barrier.clone();
        let reader = thread::spawn(move || {
            reader_barrier.wait();
            let guard = reader_table.load();
            assert!(guard.find("GET", "/a").is_some());
            guard.rules.len()
        });

        barrier.wait();
        table.hot_reload(vec![make_rule("GET", "/a"), make_rule("GET", "/b")]);

        let count = reader.join().unwrap();
        assert!(count >= 1, "reader should see a valid snapshot");
    }
}
