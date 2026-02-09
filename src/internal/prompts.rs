//! User interaction abstractions for library-first design.
//!
//! This module provides traits and implementations for user interaction,
//! allowing the library to be used both interactively (CLI) and programmatically.

/// Trait for user interaction (prompts, confirmations).
///
/// This trait abstracts away the interactive prompting mechanism, allowing
/// the library to be used in different contexts:
/// - CLI applications can use `InquireInteraction` (in the cli module)
/// - Library consumers can use `AutoApprove` or `DenyAll`
/// - Tests can use mock implementations
///
/// # Example
///
/// ```rust
/// use dx_scope::UserInteraction;
///
/// fn run_with_interaction<U: UserInteraction>(interaction: &U) {
///     if interaction.confirm("Apply fix?", Some("This will modify files")) {
///         // Apply the fix
///     }
/// }
/// ```
pub trait UserInteraction: Send + Sync {
    /// Prompt user for yes/no confirmation.
    ///
    /// # Arguments
    /// * `prompt` - The question to ask the user
    /// * `help_text` - Optional additional context or help text
    ///
    /// # Returns
    /// `true` if the user confirms, `false` otherwise
    fn confirm(&self, prompt: &str, help_text: Option<&str>) -> bool;

    /// Notify the user with a message (non-blocking).
    ///
    /// This is used for informational messages that don't require user input.
    fn notify(&self, message: &str);
}

/// Auto-approve all prompts.
///
/// This implementation automatically approves all confirmation prompts.
/// Useful for:
/// - Automated/CI environments where human interaction isn't available
/// - Testing scenarios where you want fixes to run automatically
/// - Library usage where the caller has pre-approved all operations
///
/// # Example
///
/// ```rust
/// use dx_scope::{UserInteraction, AutoApprove};
///
/// let interaction = AutoApprove;
/// assert!(interaction.confirm("Apply fix?", None)); // Always returns true
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoApprove;

impl UserInteraction for AutoApprove {
    fn confirm(&self, _prompt: &str, _help_text: Option<&str>) -> bool {
        true
    }

    fn notify(&self, _message: &str) {
        // No-op: auto-approve mode doesn't display notifications
    }
}

/// Deny all prompts.
///
/// This implementation automatically denies all confirmation prompts.
/// Useful for:
/// - Non-interactive environments where no changes should be made
/// - Testing scenarios where you want to verify denial handling
/// - Dry-run modes where operations should be skipped
///
/// # Example
///
/// ```rust
/// use dx_scope::{UserInteraction, DenyAll};
///
/// let interaction = DenyAll;
/// assert!(!interaction.confirm("Apply fix?", None)); // Always returns false
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAll;

impl UserInteraction for DenyAll {
    fn confirm(&self, _prompt: &str, _help_text: Option<&str>) -> bool {
        false
    }

    fn notify(&self, _message: &str) {
        // No-op: deny-all mode doesn't display notifications
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_approve_always_returns_true() {
        let interaction = AutoApprove;
        assert!(interaction.confirm("Test prompt?", None));
        assert!(interaction.confirm("Another prompt?", Some("With help text")));
    }

    #[test]
    fn test_deny_all_always_returns_false() {
        let interaction = DenyAll;
        assert!(!interaction.confirm("Test prompt?", None));
        assert!(!interaction.confirm("Another prompt?", Some("With help text")));
    }

    #[test]
    fn test_auto_approve_notify_does_not_panic() {
        let interaction = AutoApprove;
        interaction.notify("Test notification"); // Should not panic
    }

    #[test]
    fn test_deny_all_notify_does_not_panic() {
        let interaction = DenyAll;
        interaction.notify("Test notification"); // Should not panic
    }
}
