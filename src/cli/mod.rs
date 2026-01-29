//! CLI-specific implementations for interactive usage.
//!
//! This module provides implementations of the library traits that are
//! suitable for command-line interface usage, including:
//!
//! - Interactive prompts using the `inquire` crate
//! - Visual progress reporting using `tracing-indicatif`
//!
//! # When to Use
//!
//! Use the implementations in this module when building CLI applications
//! that need interactive user prompts. For library usage, automated
//! environments, or testing, use the implementations in [`crate::internal`].
//!
//! # Examples
//!
//! ## Interactive CLI Application
//!
//! ```rust
//! use dx_scope::InquireInteraction;
//! use dx_scope::internal::prompts::UserInteraction;
//!
//! let interaction = InquireInteraction;
//!
//! // This will show an interactive prompt in the terminal
//! // In non-TTY environments (like this doc test), it returns false
//! let result = interaction.confirm("Apply this fix?", Some("This will modify files"));
//! // result is false in doc test environment (no TTY)
//! ```
//!
//! # TTY Detection
//!
//! `InquireInteraction` automatically detects when stdin is not a TTY
//! (e.g., when running in a pipe or CI environment) and returns `false`
//! instead of crashing. For explicit control in non-interactive environments,
//! use [`AutoApprove`](crate::AutoApprove) or [`DenyAll`](crate::DenyAll).


use crate::internal::prompts::UserInteraction;
use inquire::InquireError;
use tracing::warn;

/// CLI user interaction using the `inquire` crate.
///
/// This implementation provides interactive prompts suitable for terminal usage.
/// It handles TTY detection and gracefully falls back to denial when running
/// in non-interactive environments.
///
/// # Example
///
/// ```rust
/// use dx_scope::InquireInteraction;
/// use dx_scope::internal::prompts::UserInteraction;
///
/// let interaction = InquireInteraction;
/// // In non-TTY environments, confirm() returns false gracefully
/// let confirmed = interaction.confirm("Apply fix?", Some("This will modify files"));
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct InquireInteraction;

impl UserInteraction for InquireInteraction {
    fn confirm(&self, prompt: &str, help_text: Option<&str>) -> bool {
        tracing_indicatif::suspend_tracing_indicatif(|| {
            let base_prompt = inquire::Confirm::new(prompt).with_default(false);
            let prompt = match help_text {
                Some(text) => base_prompt.with_help_message(text),
                None => base_prompt,
            };

            match prompt.prompt() {
                Ok(result) => result,
                Err(InquireError::NotTTY) => {
                    warn!(target: "user", "Prompting user, but input device is not a TTY. Skipping.");
                    false
                }
                Err(_) => false,
            }
        })
    }

    fn notify(&self, message: &str) {
        println!("{}", message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inquire_interaction_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<InquireInteraction>();
    }
}
