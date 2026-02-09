//! Internal abstractions for library-first design.
//!
//! This module contains traits and implementations that allow the scope library
//! to be used both as a CLI tool and as a programmatic library. These abstractions
//! decouple the core logic from specific UI implementations.
//!
//! # Overview
//!
//! The internal module provides:
//! - [`prompts`] - User interaction abstractions (confirmations, notifications)
//! - [`progress`] - Progress reporting abstractions
//!
//! # Choosing an Implementation
//!
//! | Use Case | UserInteraction | ProgressReporter |
//! |----------|-----------------|------------------|
//! | CLI/Interactive | `InquireInteraction` | (use tracing-indicatif) |
//! | CI/Automated | `AutoApprove` | `NoOpProgress` |
//! | Dry-run/Testing | `DenyAll` | `NoOpProgress` |
//! | Custom | Implement trait | Implement trait |
//!
//! # Examples
//!
//! ## Automated Environment (CI)
//!
//! ```rust
//! use dx_scope::internal::prompts::{UserInteraction, AutoApprove};
//! use dx_scope::internal::progress::{ProgressReporter, NoOpProgress};
//!
//! let interaction = AutoApprove;
//! let progress = NoOpProgress;
//!
//! // All prompts will be automatically approved
//! assert!(interaction.confirm("Apply fix?", None));
//!
//! // Progress calls are silent
//! progress.start_group("build", 5);
//! progress.finish_group();
//! ```
//!
//! ## Dry-Run Mode
//!
//! ```rust
//! use dx_scope::internal::prompts::{UserInteraction, DenyAll};
//!
//! let interaction = DenyAll;
//!
//! // All prompts will be denied - no changes made
//! assert!(!interaction.confirm("Apply fix?", None));
//! ```
//!
//! ## Custom Implementation
//!
//! ```rust
//! use dx_scope::internal::prompts::UserInteraction;
//!
//! struct AlwaysAskUser;
//!
//! impl UserInteraction for AlwaysAskUser {
//!     fn confirm(&self, prompt: &str, _help: Option<&str>) -> bool {
//!         // Custom logic - maybe read from a config file
//!         println!("Would prompt: {}", prompt);
//!         false
//!     }
//!
//!     fn notify(&self, message: &str) {
//!         println!("[INFO] {}", message);
//!     }
//! }
//! ```
//!
//! # Thread Safety
//!
//! All provided implementations are `Send + Sync`, making them safe to use
//! across async tasks and threads.

pub mod progress;
pub mod prompts;

// Re-export commonly used types at the module level
pub use progress::{NoOpProgress, ProgressReporter};
pub use prompts::{AutoApprove, DenyAll, UserInteraction};
