//! Public API for the doctor module.
//!
//! This module provides the main library entry points for programmatic usage
//! of the doctor functionality without CLI dependencies.
//!
//! # Examples
//!
//! ## Run All Checks with Auto-Fix
//!
//! ```rust,no_run
//! use dx_scope::{DoctorRunOptions, FoundConfig};
//! use dx_scope::doctor::run;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Load configuration
//!     let working_dir = std::env::current_dir()?;
//!     let config = FoundConfig::empty(working_dir);
//!
//!     // Configure options with auto-fix enabled
//!     let options = DoctorRunOptions::with_fixes();
//!
//!     let result = run(&config, options).await?;
//!
//!     println!("Success: {}", result.did_succeed);
//!     println!("Passed: {}", result.succeeded_groups.len());
//!     println!("Failed: {}", result.failed_group.len());
//!
//!     Ok(())
//! }
//! ```

use crate::doctor::check::{DefaultDoctorActionRun, DefaultGlobWalker};
use crate::doctor::file_cache::{FileBasedCache, FileCache, NoOpCache};
use crate::doctor::options::DoctorRunOptions;
use crate::doctor::runner::{GroupActionContainer, PathRunResult, RunGroups, compute_group_order};
use crate::shared::directories;
use crate::shared::prelude::{DefaultExecutionProvider, ExecutionProvider, FoundConfig};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tracing::{info, warn};

/// Run doctor health checks.
///
/// This is the main library entry point for running doctor checks programmatically.
/// It runs health checks defined in the configuration and optionally applies fixes.
///
/// # Arguments
///
/// * `config` - Loaded scope configuration
/// * `options` - Doctor run options (groups, fix mode, cache settings)
///
/// # Returns
///
/// Returns `PathRunResult` containing:
/// - `did_succeed`: Overall success/failure
/// - `succeeded_groups`: Names of groups that passed
/// - `failed_group`: Names of groups that failed
/// - `skipped_group`: Names of groups that were skipped
/// - `group_reports`: Detailed reports for each group
///
/// # Examples
///
/// ```rust
/// use dx_scope::{DoctorRunOptions, FoundConfig};
/// use dx_scope::doctor::run;
///
/// let working_dir = std::env::current_dir().unwrap();
/// let config = FoundConfig::empty(working_dir);
/// let options = DoctorRunOptions::with_fixes();
/// // Call: run(&config, options).await
/// ```
///
/// # Note on Interaction
///
/// When `options.run_fix` is true, fixes will be applied automatically without prompting.
/// For non-interactive/CI environments, this is the recommended mode.
pub async fn run(
    config: &FoundConfig,
    options: DoctorRunOptions,
) -> Result<PathRunResult>
{
    info!("Starting doctor run");

    // Get cache implementation
    let file_cache: Arc<dyn FileCache> = if options.no_cache {
        Arc::<NoOpCache>::default()
    } else {
        let cache_dir_path = options.cache_dir.clone().unwrap_or_else(|| {
            directories::cache()
                .expect("Unable to determine cache directory")
                .join("scope")
        });

        let cache_path = cache_dir_path.join("cache-file.json");

        match FileBasedCache::new(&cache_path) {
            Ok(cache) => Arc::new(cache),
            Err(e) => {
                warn!("Unable to create cache {:?}", e);
                Arc::<NoOpCache>::default()
            }
        }
    };

    // Get execution provider
    let exec_runner: Arc<dyn ExecutionProvider> = Arc::new(DefaultExecutionProvider::default());
    let glob_walker = Arc::new(DefaultGlobWalker::default());

    // Build group containers and desired groups set
    let mut groups = BTreeMap::new();
    let mut desired_groups = BTreeSet::new();
    let run_fix = options.run_fix;

    for group in config.doctor_group.values() {
        let should_group_run = match &options.only_groups {
            None => group.run_by_default,
            Some(names) => names.contains(&group.metadata.name().to_string()),
        };

        let mut action_runs = Vec::new();

        for action in &group.actions {
            let run = DefaultDoctorActionRun {
                model: group.clone(),
                action: action.clone(),
                working_dir: config.working_dir.clone(),
                file_cache: file_cache.clone(),
                run_fix,
                exec_runner: exec_runner.clone(),
                glob_walker: glob_walker.clone(),
                known_errors: config.known_error.clone(),
            };

            action_runs.push(run);
        }

        let container = GroupActionContainer::new(
            group.clone(),
            action_runs,
            exec_runner.clone(),
            config.working_dir.clone(),
            config.bin_path.clone(),
        );

        let group_name = container.group_name().to_string();
        groups.insert(group_name.clone(), container);

        if should_group_run {
            desired_groups.insert(group_name);
        }
    }

    // Compute group order
    let all_paths = compute_group_order(&config.doctor_group, desired_groups);

    if all_paths.is_empty() {
        warn!("Could not find any tasks to execute");
    }

    // Execute groups
    let run_groups = RunGroups {
        group_actions: groups,
        all_paths,
        yolo: options.run_fix, // Auto-approve fixes when run_fix is enabled
    };
    let result = run_groups.execute().await?;

    // Persist cache
    if let Err(e) = file_cache.persist().await {
        info!("Unable to store cache {:?}", e);
        warn!("Unable to update cache, re-runs may redo work");
    }

    info!(
        "Doctor run completed: {} succeeded, {} failed, {} skipped",
        result.succeeded_groups.len(),
        result.failed_group.len(),
        result.skipped_group.len()
    );

    Ok(result)
}

/// List available doctor checks.
///
/// Returns information about all available doctor checks and groups
/// defined in the configuration.
///
/// # Arguments
///
/// * `config` - Loaded scope configuration
///
/// # Returns
///
/// Returns a vector of doctor groups with their checks.
///
/// # Examples
///
/// ```rust
/// use dx_scope::FoundConfig;
/// use dx_scope::doctor::list;
///
/// let working_dir = std::env::current_dir().unwrap();
/// let config = FoundConfig::empty(working_dir);
/// // Call: list(&config).await
/// // Then iterate: for group in groups { ... }
/// ```
pub async fn list(config: &FoundConfig) -> Result<Vec<crate::shared::prelude::DoctorGroup>> {
    let order = super::commands::generate_doctor_list(config);
    Ok(order.clone())
}
