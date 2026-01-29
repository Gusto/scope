//! Progress reporting abstractions for library-first design.
//!
//! This module provides traits and implementations for progress reporting,
//! allowing the library to be used both with visual progress indicators (CLI)
//! and silently (library/testing).

/// Trait for progress reporting during long-running operations.
///
/// This trait abstracts away the progress visualization mechanism, allowing
/// the library to be used in different contexts:
/// - CLI applications can use `IndicatifProgress` (in the cli module)
/// - Library consumers can use `NoOpProgress`
/// - Tests can use mock implementations to verify progress calls
///
/// # Example
///
/// ```rust
/// use dx_scope::ProgressReporter;
///
/// fn run_checks<P: ProgressReporter>(progress: &P, groups: &[&str]) {
///     for group in groups {
///         progress.start_group(group, 3);
///         // Run actions...
///         progress.advance_action("action1", "Checking dependencies");
///         progress.finish_group();
///     }
/// }
/// ```
pub trait ProgressReporter: Send + Sync {
    /// Start a new group of actions.
    ///
    /// # Arguments
    /// * `name` - The name of the group being processed
    /// * `total_actions` - The total number of actions in this group
    fn start_group(&self, name: &str, total_actions: usize);

    /// Advance to the next action within the current group.
    ///
    /// # Arguments
    /// * `name` - The name of the action
    /// * `description` - A description of what the action does
    fn advance_action(&self, name: &str, description: &str);

    /// Finish the current group.
    fn finish_group(&self);
}

/// No-op progress reporter for library use.
///
/// This implementation does nothing for all progress methods.
/// Useful for:
/// - Library usage where visual progress isn't needed
/// - Testing scenarios where progress output should be suppressed
/// - Automated/CI environments
///
/// # Example
///
/// ```rust
/// use dx_scope::{ProgressReporter, NoOpProgress};
///
/// let progress = NoOpProgress;
/// progress.start_group("test", 5); // Does nothing
/// progress.advance_action("action", "description"); // Does nothing
/// progress.finish_group(); // Does nothing
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpProgress;

impl ProgressReporter for NoOpProgress {
    fn start_group(&self, _name: &str, _total_actions: usize) {
        // No-op
    }

    fn advance_action(&self, _name: &str, _description: &str) {
        // No-op
    }

    fn finish_group(&self) {
        // No-op
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_progress_does_not_panic() {
        let progress = NoOpProgress;
        progress.start_group("test-group", 10);
        progress.advance_action("action1", "Test action");
        progress.advance_action("action2", "Another action");
        progress.finish_group();
        // Should complete without panicking
    }

    #[test]
    fn test_noop_progress_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NoOpProgress>();
    }
}
