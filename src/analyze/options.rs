//! CLI-independent options types for the analyze module.
//!
//! This module provides options types that can be used to configure
//! analyze operations without depending on CLI argument parsing (clap).
//!
//! # Overview
//!
//! The analyze functionality detects known errors in command output or log files
//! and can optionally run fixes for detected errors.
//!
//! # Examples
//!
//! ## Basic Configuration
//!
//! ```rust
//! use dx_scope::AnalyzeOptions;
//! use std::collections::BTreeMap;
//! use std::path::PathBuf;
//!
//! let options = AnalyzeOptions {
//!     known_errors: BTreeMap::new(),
//!     working_dir: PathBuf::from("/path/to/project"),
//! };
//! assert!(options.known_errors.is_empty());
//! ```
//!
//! ## Different Input Sources
//!
//! ```rust,no_run
//! use dx_scope::analyze::options::AnalyzeInput;
//!
//! // From a file
//! let input = AnalyzeInput::from_file("/var/log/build.log");
//!
//! // From stdin
//! let input = AnalyzeInput::Stdin;
//!
//! // From in-memory lines (useful for testing or library usage)
//! let input = AnalyzeInput::from_lines(vec![
//!     "Building project...".to_string(),
//!     "error: missing dependency".to_string(),
//! ]);
//! ```

use crate::shared::prelude::KnownError;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Options for running analyze operations.
///
/// This struct contains all the configuration needed to run analysis,
/// without any CLI-specific dependencies like clap.
///
/// # Example
///
/// ```rust
/// use dx_scope::AnalyzeOptions;
/// use std::collections::BTreeMap;
/// use std::path::PathBuf;
///
/// let options = AnalyzeOptions {
///     known_errors: BTreeMap::new(),
///     working_dir: PathBuf::from("/path/to/project"),
/// };
/// assert!(options.known_errors.is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct AnalyzeOptions {
    /// Map of known errors to detect and potentially fix
    pub known_errors: BTreeMap<String, KnownError>,
    /// Working directory for running fix commands
    pub working_dir: PathBuf,
}

impl AnalyzeOptions {
    /// Create new analyze options with the given parameters.
    pub fn new(known_errors: BTreeMap<String, KnownError>, working_dir: PathBuf) -> Self {
        Self {
            known_errors,
            working_dir,
        }
    }
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            known_errors: BTreeMap::new(),
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

/// Specifies the input source for analysis.
///
/// The analyze functionality can process input from various sources.
/// This enum allows callers to specify where the input should come from.
#[derive(Debug, Clone)]
pub enum AnalyzeInput {
    /// Read from a file at the given path
    File(PathBuf),
    /// Read from standard input
    Stdin,
    /// Process the given lines directly (useful for library usage)
    Lines(Vec<String>),
}

impl AnalyzeInput {
    /// Create input from a file path.
    pub fn from_file(path: impl Into<PathBuf>) -> Self {
        Self::File(path.into())
    }

    /// Create input from a vector of strings.
    pub fn from_lines(lines: Vec<String>) -> Self {
        Self::Lines(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_options_default() {
        let options = AnalyzeOptions::default();
        assert!(options.known_errors.is_empty());
    }

    #[test]
    fn test_analyze_input_from_file() {
        let input = AnalyzeInput::from_file("/path/to/file");
        match input {
            AnalyzeInput::File(path) => assert_eq!(path, PathBuf::from("/path/to/file")),
            _ => panic!("Expected File variant"),
        }
    }

    #[test]
    fn test_analyze_input_from_lines() {
        let lines = vec!["line1".to_string(), "line2".to_string()];
        let input = AnalyzeInput::from_lines(lines.clone());
        match input {
            AnalyzeInput::Lines(l) => assert_eq!(l, lines),
            _ => panic!("Expected Lines variant"),
        }
    }
}
