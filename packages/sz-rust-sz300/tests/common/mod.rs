use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

static ENV_LOCK: AtomicBool = AtomicBool::new(false);

thread_local! {
    static LOCK_DEPTH: Cell<i32> = Cell::new(0);
}

fn acquire_lock() {
    let already_locked = LOCK_DEPTH.with(|d| {
        let depth = d.get();
        d.set(depth + 1);
        depth > 0
    });
    if !already_locked {
        while ENV_LOCK
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
    }
}

fn release_lock() {
    let should_release = LOCK_DEPTH.with(|d| {
        let depth = d.get() - 1;
        d.set(depth);
        depth == 0
    });
    if should_release {
        ENV_LOCK.store(false, Ordering::Release);
    }
}

pub struct EnvGuard {
    vars: Vec<String>,
}

impl EnvGuard {
    pub fn set(name: &str, value: &str) -> Self {
        acquire_lock();
        std::env::set_var(name, value);
        Self {
            vars: vec![name.to_string()],
        }
    }

    pub fn set_many(pairs: &[(&str, &str)]) -> Self {
        acquire_lock();
        let names: Vec<String> = pairs.iter().map(|(k, _)| k.to_string()).collect();
        for (k, v) in pairs {
            std::env::set_var(k, v);
        }
        Self { vars: names }
    }

    pub fn clean(names: &[&str]) -> Self {
        acquire_lock();
        for name in names {
            std::env::remove_var(name);
        }
        Self { vars: vec![] }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for name in &self.vars {
            std::env::remove_var(name);
        }
        release_lock();
    }
}
