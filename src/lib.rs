//! # dev-scope Library
//!
//! This library provides the core functionality for the `scope` tool,
//! designed for library-first usage with a thin CLI wrapper.
//!
//! ## Key Features
//!
//! - **Analyze**: Detect known errors in command output and log files
//! - **Doctor**: Run health checks with automatic fixes
//! - **Config**: Load and manage scope configuration files
//!
//! ## Library Usage
//!
//! The library can be used programmatically without CLI dependencies:
//!
//! ```rust
//! use dx_scope::{
//!     AnalyzeOptions, AnalyzeInput, AutoApprove,
//!     DoctorRunOptions, FoundConfig,
//! };
//!
//! // Create configuration
//! let working_dir = std::env::current_dir().unwrap();
//! let config = FoundConfig::empty(working_dir);
//!
//! // Configure analyze options
//! let analyze_options = AnalyzeOptions::default();
//! let input = AnalyzeInput::from_lines(vec!["test".to_string()]);
//! // Use dx_scope::analyze::process_input(&analyze_options, input, &AutoApprove).await
//!
//! // Configure doctor options for CI mode
//! let doctor_options = DoctorRunOptions::ci_mode();
//! // Use dx_scope::doctor::run(&config, doctor_options).await
//! ```
//!
//! ## Modules
//!
//! - [`analyze`] - Log and output analysis for known errors
//! - [`doctor`] - Health checks and automatic fixes
//! - [`internal`] - Abstraction traits (UserInteraction, ProgressReporter)
//! - [`shared`] - Shared utilities and configuration loading
//! - [`models`] - Data model definitions
//!
//! ## CLI Module
//!
//! The `cli` module contains CLI-specific implementations and is not exported
//! as part of the public library API. However, `InquireInteraction` is re-exported
//! at the crate root for convenience when building CLI applications.

pub mod analyze;
pub mod doctor;
pub mod internal;
pub mod lint;
pub mod models;
pub mod report;
pub mod shared;

// CLI module is internal - not part of public library API
// Only InquireInteraction is re-exported for CLI usage
pub(crate) mod cli;

// Re-export key types at crate root for convenience
// Analyze module
pub use analyze::{AnalyzeInput, AnalyzeOptions, AnalyzeStatus};

// Doctor module
pub use doctor::{DoctorRunOptions, PathRunResult};

// Config module
pub use shared::config::ConfigLoadOptions;
pub use shared::prelude::FoundConfig;

// Internal abstractions (for library implementors)
pub use internal::progress::{NoOpProgress, ProgressReporter};
pub use internal::prompts::{AutoApprove, DenyAll, UserInteraction};

// CLI implementations
pub use cli::InquireInteraction;

// Capture module (for CLI tools that intercept commands)
pub use shared::prelude::{
    CaptureOpts, DefaultExecutionProvider, OutputCapture, OutputDestination,
};

// Logging module (for CLI tools)
pub use shared::prelude::LoggingOpts;

// Config loading (for CLI tools)
pub use shared::prelude::ConfigOptions;

// Report builders (for CLI tools)
pub use shared::prelude::{DefaultGroupedReportBuilder, DefaultUnstructuredReportBuilder};

// Report traits (for CLI tools that need to render reports)
pub use shared::prelude::{ReportRenderer, UnstructuredReportBuilder};

// Model traits (for accessing metadata on config models)
pub use models::HelpMetadata;

// CLI argument types (for CLI binaries)
pub use analyze::prelude::{AnalyzeArgs, analyze_root};
pub use doctor::prelude::{DoctorArgs, doctor_root};
pub use lint::cli::LintArgs;
pub use lint::commands::lint_root;
pub use report::prelude::{ReportArgs, report_root};

// Shared utilities (for CLI binaries)
pub use shared::{print_details, CONFIG_FILE_PATH_ENV, RUN_ID_ENV_VAR};

/// Prelude module for convenient glob imports.
///
/// **DEPRECATED**: This module will be removed in a future version.
/// For new code, use explicit imports from the crate root or specific modules instead.
///
/// # Migration
///
/// Instead of:
/// ```rust
/// # #[allow(deprecated)]
/// use dx_scope::prelude::*;
/// ```
///
/// Use explicit imports:
/// ```rust
/// use dx_scope::{DoctorRunOptions, AnalyzeOptions, FoundConfig};
/// use dx_scope::doctor;
/// use dx_scope::analyze;
/// ```
#[deprecated(
    since = "2026.1.13",
    note = "Use explicit imports from crate root or specific modules instead of prelude"
)]
pub mod prelude {
    pub use crate::analyze::prelude::*;
    pub use crate::doctor::prelude::*;
    pub use crate::lint::prelude::*;
    pub use crate::models::prelude::*;
    pub use crate::report::prelude::*;
    pub use crate::shared::prelude::*;
}

/// Preferred way to output data to users. This macro will write the output to tracing for debugging
/// and to stdout using the global stdout writer. Because we use the stdout writer, the calls
/// will all be async.
#[macro_export]
macro_rules! report_stdout {
    ($($arg:tt)*) => {
        tracing::info!(target="stdout", $($arg)*);
        writeln!($crate::prelude::STDOUT_WRITER.write().await, $($arg)*).ok()
    };
}
