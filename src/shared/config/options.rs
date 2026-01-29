//! CLI-independent configuration loading options.
//!
//! This module provides options types that can be used to configure
//! configuration loading without depending on CLI argument parsing (clap).
//!
//! # Overview
//!
//! Scope configuration is loaded from YAML files in `.scope` directories.
//! By default, scope searches:
//!
//! 1. Ancestor directories from the working directory (`.scope/`)
//! 2. User home directory (`~/.scope/`)
//! 3. System config directory
//!
//! # Examples
//!
//! ## Default Discovery
//!
//! ```rust
//! use dx_scope::shared::config::ConfigLoadOptions;
//!
//! // Uses default discovery from current directory
//! let options = ConfigLoadOptions::default();
//! ```
//!
//! ## Custom Working Directory
//!
//! ```rust
//! use dx_scope::shared::config::ConfigLoadOptions;
//! use std::path::PathBuf;
//!
//! // Search from a specific directory
//! let options = ConfigLoadOptions::with_working_dir(
//!     PathBuf::from("/path/to/my/project")
//! );
//! ```
//!
//! ## Additional Config Paths
//!
//! ```rust
//! use dx_scope::shared::config::ConfigLoadOptions;
//! use std::path::PathBuf;
//!
//! // Add extra config directories to the search
//! let options = ConfigLoadOptions::with_extra_config(vec![
//!     PathBuf::from("/shared/team/config"),
//!     PathBuf::from("/company/global/scope"),
//! ]);
//! ```
//!
//! ## Explicit Paths Only
//!
//! ```rust
//! use dx_scope::shared::config::ConfigLoadOptions;
//! use std::path::PathBuf;
//!
//! // Disable default discovery, use only specified paths
//! let options = ConfigLoadOptions::explicit_only(vec![
//!     PathBuf::from("/my/config/only"),
//! ]);
//! ```
//!
//! # Configuration Precedence
//!
//! When the same configuration item appears in multiple files, the first
//! occurrence wins. Files are processed in order:
//!
//! 1. Most specific directory (closest to working dir)
//! 2. Parent directories (ascending)
//! 3. Home directory
//! 4. System config
//! 5. Extra config paths (in order specified)

use std::path::PathBuf;

/// Options for loading scope configuration.
///
/// This struct contains all the configuration needed to find and load
/// scope configuration files, without any CLI-specific dependencies like clap.
///
/// # Example
///
/// ```rust,no_run
/// use dx_scope::shared::config::ConfigLoadOptions;
/// use std::path::PathBuf;
///
/// // Use default configuration discovery
/// let options = ConfigLoadOptions::default();
///
/// // Load from specific directories
/// let options = ConfigLoadOptions {
///     extra_config: vec![PathBuf::from("/custom/config/path")],
///     ..Default::default()
/// };
///
/// // Disable default config discovery (only use explicit paths)
/// let options = ConfigLoadOptions {
///     disable_default_config: true,
///     extra_config: vec![PathBuf::from("/my/config")],
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct ConfigLoadOptions {
    /// Additional paths to search for configuration files.
    /// These are added to the default discovery paths.
    pub extra_config: Vec<PathBuf>,

    /// When true, skip default config discovery (ancestor .scope directories,
    /// home directory, system config). Only explicitly specified paths are used.
    pub disable_default_config: bool,

    /// Override the working directory for config discovery and command execution.
    /// If None, uses the current working directory.
    pub working_dir: Option<PathBuf>,

    /// Custom run ID for this execution.
    /// If None, a unique ID will be generated.
    pub run_id: Option<String>,
}

impl ConfigLoadOptions {
    /// Create options that only use explicitly specified config paths.
    pub fn explicit_only(paths: Vec<PathBuf>) -> Self {
        Self {
            extra_config: paths,
            disable_default_config: true,
            ..Default::default()
        }
    }

    /// Create options with a custom working directory.
    pub fn with_working_dir(working_dir: PathBuf) -> Self {
        Self {
            working_dir: Some(working_dir),
            ..Default::default()
        }
    }

    /// Create options with additional config paths.
    pub fn with_extra_config(paths: Vec<PathBuf>) -> Self {
        Self {
            extra_config: paths,
            ..Default::default()
        }
    }

    /// Get the working directory, falling back to current directory.
    pub fn get_working_dir(&self) -> std::io::Result<PathBuf> {
        match &self.working_dir {
            Some(dir) => Ok(dir.clone()),
            None => std::env::current_dir(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_options() {
        let options = ConfigLoadOptions::default();
        assert!(options.extra_config.is_empty());
        assert!(!options.disable_default_config);
        assert!(options.working_dir.is_none());
        assert!(options.run_id.is_none());
    }

    #[test]
    fn test_explicit_only() {
        let paths = vec![PathBuf::from("/path1"), PathBuf::from("/path2")];
        let options = ConfigLoadOptions::explicit_only(paths.clone());
        assert_eq!(options.extra_config, paths);
        assert!(options.disable_default_config);
    }

    #[test]
    fn test_with_working_dir() {
        let dir = PathBuf::from("/custom/dir");
        let options = ConfigLoadOptions::with_working_dir(dir.clone());
        assert_eq!(options.working_dir, Some(dir));
    }

    #[test]
    fn test_with_extra_config() {
        let paths = vec![PathBuf::from("/extra/config")];
        let options = ConfigLoadOptions::with_extra_config(paths.clone());
        assert_eq!(options.extra_config, paths);
        assert!(!options.disable_default_config);
    }

    #[test]
    fn test_get_working_dir_with_override() {
        let dir = PathBuf::from("/override/dir");
        let options = ConfigLoadOptions {
            working_dir: Some(dir.clone()),
            ..Default::default()
        };
        assert_eq!(options.get_working_dir().unwrap(), dir);
    }

    #[test]
    fn test_get_working_dir_without_override() {
        let options = ConfigLoadOptions::default();
        // Should return the current directory
        let result = options.get_working_dir();
        assert!(result.is_ok());
    }
}
