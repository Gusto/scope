mod api;
mod cli;
mod error;
pub mod options;

pub mod prelude {
    pub use super::cli::{AnalyzeArgs, analyze_root};
}

// Re-export key types for library usage
pub use crate::shared::analyze::{AnalyzeStatus, process_lines, report_result};
pub use options::{AnalyzeInput, AnalyzeOptions};

// Public API functions
pub use api::{process_input, process_text};
