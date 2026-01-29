//! CLI-independent options types for the doctor module.
//!
//! This module provides options types that can be used to configure
//! doctor operations without depending on CLI argument parsing (clap).
//!
//! # Overview
//!
//! The doctor functionality runs health checks on your development environment
//! and can automatically apply fixes when issues are detected.
//!
//! # Examples
//!
//! ## Common Configurations
//!
//! ```rust
//! use dx_scope::doctor::options::DoctorRunOptions;
//!
//! // CI mode - run checks without fixes
//! let ci_options = DoctorRunOptions::ci_mode();
//!
//! // Interactive mode - prompt for fixes
//! let interactive_options = DoctorRunOptions::with_fixes();
//!
//! // Run specific groups only
//! let targeted_options = DoctorRunOptions::for_groups(vec![
//!     "rust".to_string(),
//!     "docker".to_string(),
//! ]);
//! ```
//!
//! ## Full Customization
//!
//! ```rust
//! use dx_scope::doctor::options::DoctorRunOptions;
//! use std::path::PathBuf;
//!
//! let options = DoctorRunOptions {
//!     only_groups: Some(vec!["build".to_string()]),
//!     run_fix: true,
//!     cache_dir: Some(PathBuf::from("/tmp/my-cache")),
//!     no_cache: false,
//!     auto_publish_report: true,
//! };
//! ```
//!
//! # Caching
//!
//! The doctor module supports caching to avoid re-running checks when
//! source files haven't changed. Control caching with:
//!
//! - `cache_dir`: Custom cache location (default: system cache directory)
//! - `no_cache`: Disable caching entirely (useful for debugging)

use std::path::PathBuf;

/// Options for running doctor operations.
///
/// This struct contains all the configuration needed to run doctor checks,
/// without any CLI-specific dependencies like clap.
///
/// # Example
///
/// ```rust,no_run
/// use dx_scope::DoctorRunOptions;
///
/// // Run all default groups with fixes enabled
/// let options = DoctorRunOptions::default();
///
/// // Run only specific groups
/// let options = DoctorRunOptions {
///     only_groups: Some(vec!["build".to_string(), "test".to_string()]),
///     ..Default::default()
/// };
///
/// // Run in CI mode (no interactive fixes)
/// let options = DoctorRunOptions::ci_mode();
/// ```
#[derive(Debug, Clone, Default)]
pub struct DoctorRunOptions {
    /// Run only these groups (None = run all default groups)
    pub only_groups: Option<Vec<String>>,
    /// Whether to run fixes when checks fail
    pub run_fix: bool,
    /// Custom cache directory path
    pub cache_dir: Option<PathBuf>,
    /// Disable caching
    pub no_cache: bool,
    /// Automatically publish report on failure
    pub auto_publish_report: bool,
}

impl DoctorRunOptions {
    /// Create new options with default values but fixes enabled.
    pub fn with_fixes() -> Self {
        Self {
            run_fix: true,
            ..Default::default()
        }
    }

    /// Create new options for CI/non-interactive mode (no fixes).
    pub fn ci_mode() -> Self {
        Self {
            run_fix: false,
            ..Default::default()
        }
    }

    /// Create options to run specific groups only.
    pub fn for_groups(groups: Vec<String>) -> Self {
        Self {
            only_groups: Some(groups),
            run_fix: true,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_options() {
        let options = DoctorRunOptions::default();
        assert!(options.only_groups.is_none());
        assert!(!options.run_fix);
        assert!(options.cache_dir.is_none());
        assert!(!options.no_cache);
        assert!(!options.auto_publish_report);
    }

    #[test]
    fn test_with_fixes() {
        let options = DoctorRunOptions::with_fixes();
        assert!(options.run_fix);
    }

    #[test]
    fn test_ci_mode() {
        let options = DoctorRunOptions::ci_mode();
        assert!(!options.run_fix);
    }

    #[test]
    fn test_for_groups() {
        let groups = vec!["group1".to_string(), "group2".to_string()];
        let options = DoctorRunOptions::for_groups(groups.clone());
        assert_eq!(options.only_groups, Some(groups));
        assert!(options.run_fix);
    }
}
