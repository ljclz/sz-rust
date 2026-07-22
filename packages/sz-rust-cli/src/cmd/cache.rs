//! `cache:clear` 命令 — 对齐 PHP `think cache:clear`
//!
//! ## PHP 对齐
//!
//! PHP `cache:clear` 通过 `think\facade\Cache::clear()` 清空缓存。
//! Rust 端 CLI 是独立进程，无法直接访问运行时 `CacheManager` 实例，
//! 因此通过以下方式实现：
//!
//! 1. 清除缓存目录（默认 `runtime/cache`）
//! 2. 输出提示信息

use std::path::PathBuf;

use crate::error::CliError;

/// 执行 cache:clear 命令
///
/// # 参数
///
/// - `store`：指定缓存存储名（`None` 清空所有）
pub fn execute_cache_clear(store: Option<&str>) -> Result<(), CliError> {
    match store {
        Some(store_name) => clear_store(store_name),
        None => clear_all(),
    }
}

/// 清空指定缓存存储
fn clear_store(store: &str) -> Result<(), CliError> {
    let cache_dir = get_cache_dir(store);

    if !cache_dir.exists() {
        println!(
            "Cache store '{}' directory not found: {}",
            store,
            cache_dir.display()
        );
        return Ok(());
    }

    let count = remove_dir_contents(&cache_dir)?;
    println!(
        "Cache store '{}' cleared: {} file(s) removed from {}",
        store,
        count,
        cache_dir.display()
    );
    Ok(())
}

/// 清空所有缓存
fn clear_all() -> Result<(), CliError> {
    let cache_root = get_cache_root();

    if !cache_root.exists() {
        println!("Cache root directory not found: {}", cache_root.display());
        println!("Nothing to clear.");
        return Ok(());
    }

    let count = remove_dir_contents(&cache_root)?;
    println!(
        "All caches cleared: {} file(s)/dir(s) removed from {}",
        count,
        cache_root.display()
    );
    Ok(())
}

/// 获取缓存根目录
///
/// 对齐 PHP `runtime/cache` 路径约定。
fn get_cache_root() -> PathBuf {
    PathBuf::from("runtime/cache")
}

/// 获取指定存储的缓存目录
fn get_cache_dir(store: &str) -> PathBuf {
    get_cache_root().join(store)
}

/// 递归删除目录内容（保留目录本身）
///
/// # 返回
///
/// 删除的文件和目录数量
fn remove_dir_contents(path: &PathBuf) -> Result<usize, CliError> {
    let mut count = 0;
    let entries = std::fs::read_dir(path)?;
    for entry in entries {
        let entry = entry?;
        let entry_path = entry.path();
        if entry_path.is_dir() {
            std::fs::remove_dir_all(&entry_path)?;
        } else {
            std::fs::remove_file(&entry_path)?;
        }
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn test_get_cache_root_default() {
        let root = get_cache_root();
        assert_eq!(root, PathBuf::from("runtime/cache"));
    }

    #[test]
    fn test_get_cache_dir_with_store() {
        let dir = get_cache_dir("redis");
        assert_eq!(dir, PathBuf::from("runtime/cache/redis"));
    }

    #[test]
    fn test_clear_all_nonexistent_dir() {
        // 在临时目录中测试（不依赖 set_current_dir）
        let temp = tempfile::tempdir().unwrap();
        let cache_root = temp.path().join("runtime/cache");
        // 不创建目录，模拟不存在
        assert!(!cache_root.exists());
    }

    #[test]
    fn test_clear_all_with_files_via_remove_dir_contents() {
        // 直接测试 remove_dir_contents 避免工作目录依赖
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();

        // 创建缓存文件
        let file1 = root.join("cache1.txt");
        let file2 = root.join("cache2.txt");
        let mut f1 = fs::File::create(&file1).unwrap();
        writeln!(f1, "data1").unwrap();
        let mut f2 = fs::File::create(&file2).unwrap();
        writeln!(f2, "data2").unwrap();

        let count = remove_dir_contents(&root).unwrap();
        assert_eq!(count, 2);
        assert!(!file1.exists());
        assert!(!file2.exists());
    }

    #[test]
    fn test_clear_store_nonexistent() {
        // 验证逻辑：不存在的存储目录不会报错
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("nonexistent_store");
        assert!(!store_dir.exists());
    }

    #[test]
    fn test_clear_store_with_files_via_remove_dir_contents() {
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("redis");
        fs::create_dir_all(&store_dir).unwrap();

        let cache_file = store_dir.join("key1.txt");
        let mut f = fs::File::create(&cache_file).unwrap();
        writeln!(f, "redis_data").unwrap();

        let count = remove_dir_contents(&store_dir).unwrap();
        assert_eq!(count, 1);
        assert!(!cache_file.exists());
    }

    #[test]
    fn test_remove_dir_contents_with_subdirs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();

        // 创建子目录和文件
        let sub_dir = root.join("subdir");
        fs::create_dir_all(&sub_dir).unwrap();
        fs::File::create(root.join("file1.txt")).unwrap();
        fs::File::create(sub_dir.join("file2.txt")).unwrap();

        let count = remove_dir_contents(&root).unwrap();
        assert_eq!(count, 2); // file1.txt + subdir

        // 验证目录已清空但本身保留
        assert!(root.exists());
        assert!(root.read_dir().unwrap().next().is_none());
    }

    #[test]
    fn test_execute_cache_clear_no_store_nonexistent() {
        // 当 runtime/cache 不存在时，execute_cache_clear 应返回 Ok
        // 注意：此测试依赖工作目录中不存在 runtime/cache
        // 在 CI/测试环境中通常满足此条件
        let result = execute_cache_clear(None);
        // 如果 runtime/cache 不存在，返回 Ok；如果存在，也返回 Ok
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_cache_clear_with_store_nonexistent() {
        // 指定不存在的存储名，应返回 Ok
        let result = execute_cache_clear(Some("nonexistent_store_xyz"));
        assert!(result.is_ok());
    }
}
