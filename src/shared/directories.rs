//! Cross-platform directory utilities for the scope application.
//!
//! This module provides functions for discovering standard directories
//! across different operating systems in a consistent way.
//!
//! You may be asking yourself, "Why didn't we use a crate like `dirs` or `directories`?"
//! The answer is that we want to use the XDG Base Directory Specification in Unix-like systems,
//! The author of dirs and directories has stated that they do not want to support this for MacOs
//! Therefore, we implement our own directory discovery logic.
//! https://github.com/dirs-dev/directories-rs/issues/47
//!
//! For info on the XDG Base Directory Specification, see:
//! https://wiki.archlinux.org/title/XDG_Base_Directory
//! https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest

use std::path::PathBuf;

/// Get the user's home directory.
///
/// # Returns
///
/// Returns `Some(PathBuf)` if the home directory can be determined,
/// `None` otherwise.
///
/// # Examples
///
/// ```
/// # use dx_scope::shared::directories::home;
/// if let Some(home) = home() {
///     println!("Home directory: {}", home.display());
/// }
/// ```
pub fn home() -> Option<PathBuf> {
    std::env::home_dir()
}

/// Get the user's configuration directory.
///
/// Returns the configuration directory following the XDG Base Directory Specification:
/// - `$XDG_CONFIG_HOME` if set, otherwise `~/.config`
///
/// # Returns
///
/// Returns `Some(PathBuf)` if the configuration directory can be determined,
/// `None` otherwise.
///
/// # Examples
///
/// ```
/// # use dx_scope::shared::directories::config;
/// if let Some(config_dir) = config() {
///     println!("Config directory: {}", config_dir.display());
/// }
/// ```
pub fn config() -> Option<PathBuf> {
    if let Some(home) = home() {
        // Use XDG Base Directory Specification
        // Check for XDG_CONFIG_HOME first, fallback to ~/.config
        if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
            Some(PathBuf::from(xdg_config))
        } else {
            Some(home.join(".config"))
        }
    } else {
        None
    }
}

/// Get the user's cache directory.
///
/// Returns the cache directory following the XDG Base Directory Specification:
/// - `$XDG_CACHE_HOME` if set, otherwise `~/.cache`
///
/// # Returns
///
/// Returns `Some(PathBuf)` if the cache directory can be determined,
/// `None` otherwise.
///
/// # Examples
///
/// ```
/// # use dx_scope::shared::directories::cache;
/// if let Some(cache_dir) = cache() {
///     println!("Cache directory: {}", cache_dir.display());
/// }
/// ```
pub fn cache() -> Option<PathBuf> {
    if let Some(home) = home() {
        // Use XDG Base Directory Specification
        // Check for XDG_CACHE_HOME first, fallback to ~/.cache
        if let Ok(xdg_cache) = std::env::var("XDG_CACHE_HOME") {
            Some(PathBuf::from(xdg_cache))
        } else {
            Some(home.join(".cache"))
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Mutex to serialize tests that modify environment variables.
    // Rust tests run in parallel by default, so we need to prevent concurrent
    // modification of shared environment variables.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // Guard struct to ensure environment variables are restored even on panic
    struct EnvGuard<'a> {
        var_name: String,
        original_value: Option<String>,
        _lock: std::sync::MutexGuard<'a, ()>,
    }

    impl<'a> EnvGuard<'a> {
        fn new(var_name: &str) -> Self {
            let lock = ENV_MUTEX.lock().unwrap();
            let original_value = env::var(var_name).ok();
            Self {
                var_name: var_name.to_string(),
                original_value,
                _lock: lock,
            }
        }

        fn set(&self, value: &str) {
            // SAFETY: This is test-only code. Access is serialized by ENV_MUTEX.
            unsafe { env::set_var(&self.var_name, value) };
        }

        fn remove(&self) {
            // SAFETY: This is test-only code. Access is serialized by ENV_MUTEX.
            unsafe { env::remove_var(&self.var_name) };
        }
    }

    impl Drop for EnvGuard<'_> {
        fn drop(&mut self) {
            // SAFETY: This is test-only code. Access is serialized by ENV_MUTEX.
            match &self.original_value {
                Some(value) => unsafe { env::set_var(&self.var_name, value) },
                None => unsafe { env::remove_var(&self.var_name) },
            }
        }
    }

    mod home {
        use super::*;

        #[test]
        fn consistency() {
            // Test that multiple calls return the same result
            let home1 = home();
            let home2 = home();
            assert_eq!(home1, home2, "home should return consistent results");
        }

        #[test]
        fn uses_home_env_var() {
            let home_var = env::var("HOME").expect("HOME environment variable should be set");
            let home_dir = home().expect("home() should return a value when HOME is set");

            assert_eq!(
                home_dir,
                PathBuf::from(home_var),
                "home() should return the path from HOME environment variable"
            );
        }
    }

    mod config {
        use super::*;

        #[test]
        fn xdg_config_home_unset() {
            let _guard = EnvGuard::new("XDG_CONFIG_HOME");
            _guard.remove();

            let config_dir = config().unwrap();
            let path_str = config_dir.to_string_lossy();
            assert!(
                path_str.ends_with("/.config"),
                "Config directory should end with '/.config', got: {path_str}"
            );
        }

        #[test]
        fn xdg_config_home_set() {
            let _guard = EnvGuard::new("XDG_CONFIG_HOME");
            let test_xdg_path = "/tmp/test_xdg_config";
            _guard.set(test_xdg_path);

            let config_dir = config().unwrap();
            assert_eq!(
                config_dir,
                PathBuf::from(test_xdg_path),
                "Should use XDG_CONFIG_HOME when set"
            );
        }

        #[test]
        fn consistency() {
            // Test that multiple calls return the same result
            let config1 = config();
            let config2 = config();
            assert_eq!(config1, config2, "config should return consistent results");
        }
    }

    mod cache {
        use super::*;

        #[test]
        fn xdg_cache_home_unset() {
            let _guard = EnvGuard::new("XDG_CACHE_HOME");
            _guard.remove();

            let cache_dir = cache().unwrap();
            let path_str = cache_dir.to_string_lossy();
            assert!(
                path_str.ends_with("/.cache"),
                "Cache directory should end with '/.cache', got: {path_str}"
            );
        }

        #[test]
        fn xdg_cache_home_set() {
            let _guard = EnvGuard::new("XDG_CACHE_HOME");
            let test_xdg_path = "/tmp/test_xdg_cache";
            _guard.set(test_xdg_path);

            let cache_dir = cache().unwrap();
            assert_eq!(
                cache_dir,
                PathBuf::from(test_xdg_path),
                "Should use XDG_CACHE_HOME when set"
            );
        }

        #[test]
        fn consistency() {
            let _guard = EnvGuard::new("XDG_CACHE_HOME");
            _guard.remove();

            // Test that multiple calls return the same result
            let cache1 = cache();
            let cache2 = cache();
            assert_eq!(cache1, cache2, "cache should return consistent results");
        }
    }
}
