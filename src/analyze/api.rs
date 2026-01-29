//! Public API for the analyze module.
//!
//! This module provides the main library entry points for programmatic usage
//! of the analyze functionality without CLI dependencies.
//!
//! # Examples
//!
//! ## Analyze Text for Known Errors
//!
//! ```rust,no_run
//! use dx_scope::{
//!     AnalyzeOptions, AnalyzeInput, AutoApprove, FoundConfig,
//! };
//! use dx_scope::analyze::process_input;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Load configuration
//!     let working_dir = std::env::current_dir()?;
//!     let config = FoundConfig::empty(working_dir);
//!
//!     // Prepare input
//!     let log_content = std::fs::read_to_string("error.log")?;
//!     let lines: Vec<String> = log_content.lines().map(|s| s.to_string()).collect();
//!     let input = AnalyzeInput::from_lines(lines);
//!
//!     // Configure options
//!     let options = AnalyzeOptions::new(
//!         config.known_error.clone(),
//!         config.working_dir.clone(),
//!     );
//!
//!     // Run analysis with auto-approve for fixes
//!     let interaction = AutoApprove;
//!     let status = process_input(&options, input, &interaction).await?;
//!
//!     match status {
//!         dx_scope::AnalyzeStatus::NoKnownErrorsFound => {
//!             println!("No errors detected");
//!         }
//!         dx_scope::AnalyzeStatus::KnownErrorFoundFixSucceeded => {
//!             println!("Error found and fixed!");
//!         }
//!         _ => println!("Error handling completed: {:?}", status),
//!     }
//!
//!     Ok(())
//! }
//! ```

use crate::analyze::options::{AnalyzeInput, AnalyzeOptions};
use crate::internal::prompts::UserInteraction;
use crate::shared::analyze::{AnalyzeStatus, process_lines as process_lines_internal};
use anyhow::Result;
use std::io::Cursor;
use tokio::io::BufReader;
use tracing::{debug, info};

/// Process input for known errors and optionally run fixes.
///
/// This is the main library entry point for analyzing text/logs programmatically.
/// It scans the input for known error patterns and can automatically apply fixes.
///
/// # Arguments
///
/// * `options` - Analyze options containing known errors and working directory
/// * `input` - Input source (file, stdin, or in-memory lines)
/// * `interaction` - Implementation of `UserInteraction` for fix prompts (use `AutoApprove` or `DenyAll`)
///
/// # Returns
///
/// Returns `AnalyzeStatus` indicating the outcome:
/// - `NoKnownErrorsFound` - No matches found
/// - `KnownErrorFoundNoFixFound` - Error matched but no fix available
/// - `KnownErrorFoundUserDenied` - User declined to run the fix
/// - `KnownErrorFoundFixFailed` - Fix was attempted but failed
/// - `KnownErrorFoundFixSucceeded` - Fix was successfully applied
///
/// # Examples
///
/// ## Analyze In-Memory Text
///
/// ```rust
/// use dx_scope::{AnalyzeOptions, AnalyzeInput, AutoApprove};
/// use dx_scope::analyze::process_input;
///
/// let lines = vec![
///     "Building project...".to_string(),
///     "error: dependency not found".to_string(),
/// ];
/// let input = AnalyzeInput::from_lines(lines);
/// let options = AnalyzeOptions::default();
/// // Call: process_input(&options, input, &AutoApprove).await
/// ```
///
/// ## Analyze a File
///
/// ```rust
/// use dx_scope::{AnalyzeOptions, AnalyzeInput, DenyAll};
/// use dx_scope::analyze::process_input;
///
/// let options = AnalyzeOptions::default();
/// let input = AnalyzeInput::from_file("/var/log/build.log");
/// // Call: process_input(&options, input, &DenyAll).await
/// ```
pub async fn process_input<U>(
    options: &AnalyzeOptions,
    input: AnalyzeInput,
    interaction: &U,
) -> Result<AnalyzeStatus>
where
    U: UserInteraction,
{
    debug!("Starting analyze with input type: {:?}", input);

    match input {
        AnalyzeInput::File(path) => {
            info!("Analyzing file: {:?}", path);
            let file = tokio::fs::File::open(&path).await?;
            let reader = BufReader::new(file);
            process_lines_internal(
                &options.known_errors,
                &options.working_dir,
                reader,
                interaction,
            )
            .await
        }
        AnalyzeInput::Stdin => {
            info!("Analyzing stdin");
            let stdin = tokio::io::stdin();
            let reader = BufReader::new(stdin);
            process_lines_internal(
                &options.known_errors,
                &options.working_dir,
                reader,
                interaction,
            )
            .await
        }
        AnalyzeInput::Lines(lines) => {
            info!("Analyzing {} lines from memory", lines.len());
            let text = lines.join("\n");
            let cursor = Cursor::new(text);
            let reader = BufReader::new(cursor);
            process_lines_internal(
                &options.known_errors,
                &options.working_dir,
                reader,
                interaction,
            )
            .await
        }
    }
}

/// Analyze text content directly for known errors.
///
/// Convenience function for analyzing a string without creating an `AnalyzeInput`.
///
/// # Arguments
///
/// * `options` - Analyze options containing known errors and working directory
/// * `text` - Text content to analyze
/// * `interaction` - Implementation of `UserInteraction` for fix prompts
///
/// # Examples
///
/// ```rust
/// use dx_scope::{AnalyzeOptions, DenyAll};
/// use dx_scope::analyze::process_text;
///
/// let log_output = "error: compilation failed\nSome other output";
/// let options = AnalyzeOptions::default();
/// // Call: process_text(&options, log_output, &DenyAll).await
/// ```
pub async fn process_text<U>(
    options: &AnalyzeOptions,
    text: &str,
    interaction: &U,
) -> Result<AnalyzeStatus>
where
    U: UserInteraction,
{
    let lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    let input = AnalyzeInput::from_lines(lines);
    process_input(options, input, interaction).await
}
