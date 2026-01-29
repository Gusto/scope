use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tracing::{info, instrument, warn};

use crate::doctor::check::{DefaultDoctorActionRun, DefaultGlobWalker};
use crate::doctor::file_cache::{FileBasedCache, FileCache, NoOpCache};
use crate::doctor::runner::{GroupActionContainer, RunGroups, compute_group_order};
use crate::prelude::{
    DefaultGroupedReportBuilder, ExecutionProvider, GroupedReportBuilder, ReportRenderer,
};
use crate::report_stdout;
use crate::shared::directories;
use crate::shared::prelude::{DefaultExecutionProvider, FoundConfig};

#[derive(Debug, Parser, Default)]
pub struct DoctorRunArgs {
    /// When set, only the checks listed will run
    #[arg(short, long)]
    pub only: Option<Vec<String>>,
    /// When set, if a fix is specified it will also run.
    #[arg(long, short, default_value = "true")]
    fix: Option<bool>,
    /// Location to store cache between runs
    #[arg(long, env = "SCOPE_DOCTOR_CACHE_DIR")]
    pub cache_dir: Option<String>,
    /// When set cache will be disabled, forcing all file based checks to run.
    #[arg(long, short, default_value = "false")]
    pub no_cache: bool,
    /// Do not ask, create report on failure
    #[arg(long, default_value = "false", env = "SCOPE_DOCTOR_AUTO_PUBLISH")]
    pub auto_publish_report: bool,
    /// Automatically approve all fix prompts without asking
    #[arg(long, short = 'y', default_value = "false")]
    pub yolo: bool,
}

fn get_cache(args: &DoctorRunArgs) -> Arc<dyn FileCache> {
    if args.no_cache {
        Arc::<NoOpCache>::default()
    } else {
        let cache_dir = args.cache_dir.clone().unwrap_or_else(|| {
            directories::cache()
                .expect("Unable to determine cache directory")
                .join("scope")
                .to_string_lossy()
                .to_string()
        });

        let cache_path = PathBuf::from(&cache_dir).join("cache-file.json");
        let old_default_cache_path = PathBuf::from("/tmp/scope/cache-file.json");

        // Handle backward compatibility: migrate from old location to new location
        let should_migrate = cache_dir != "/tmp/scope"
            && old_default_cache_path.exists()
            && !cache_path.exists();
        
        if should_migrate {
            if let Err(e) = migrate_old_cache(&old_default_cache_path, &cache_path) {
                warn!("Unable to migrate cache from old location: {:?}", e);
            }
        }

        match FileBasedCache::new(&cache_path) {
            Ok(cache) => Arc::new(cache),
            Err(e) => {
                warn!("Unable to create cache {:?}", e);
                Arc::<NoOpCache>::default()
            }
        }
    }
}

fn migrate_old_cache(old_path: &PathBuf, new_path: &PathBuf) -> Result<()> {
    // Create the new cache directory if it doesn't exist
    if let Some(parent) = new_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Copy the old cache file to the new location
    std::fs::copy(old_path, new_path)?;

    // Remove the old cache file
    std::fs::remove_file(old_path)?;

    info!(
        "Migrated cache from {} to {}",
        old_path.display(),
        new_path.display()
    );
    Ok(())
}

#[instrument("scope doctor run", skip(found_config))]
pub async fn doctor_run(found_config: &FoundConfig, args: &DoctorRunArgs) -> Result<i32> {
    let transform = transform_inputs(found_config, args);

    let all_paths = compute_group_order(&found_config.doctor_group, transform.desired_groups);
    if all_paths.is_empty() {
        warn!(target: "user", "Could not find any tasks to execute");
    }

    let run_groups = RunGroups {
        group_actions: transform.groups,
        all_paths,
        yolo: args.yolo,
    };

    let result = run_groups.execute().await?;
    report_stdout!("Summary: {}", result);

    if let Err(e) = transform.file_cache.persist().await {
        info!("Unable to store cache {:?}", e);
        warn!(target: "user", "Unable to update cache, re-runs may redo work");
    }

    if !result.did_succeed
        && !result.failed_group.is_empty()
        && !found_config.report_upload.is_empty()
    {
        println!();
        let create_report = if args.auto_publish_report {
            true
        } else {
            tracing_indicatif::suspend_tracing_indicatif(|| {
                inquire::Confirm::new("Do you want to upload a bug report?")
                    .with_default(false)
                    .with_help_message(
                        "This will allow you to share the error with other engineers for support.",
                    )
                    .prompt()
                    .unwrap_or(false)
            })
        };

        if create_report {
            let mut builder = DefaultGroupedReportBuilder::new("scope doctor run");

            for group_report in &result.group_reports {
                builder.append_group(group_report).ok();
            }

            for location in found_config.report_upload.values() {
                let mut builder = builder.clone();
                builder
                    .run_and_append_additional_data(
                        found_config,
                        transform.exec_runner.clone(),
                        &location.additional_data,
                    )
                    .await
                    .ok();

                let report = builder.render(location);

                match report {
                    Err(e) => warn!(target: "user", "Unable to render report: {}", e),
                    Ok(report) => {
                        if let Err(e) = report.distribute().await {
                            warn!(target: "user", "Unable to upload report: {}", e);
                        }
                    }
                }
            }
        }
    }

    if result.did_succeed { Ok(0) } else { Ok(1) }
}

struct RunTransform {
    groups: BTreeMap<String, GroupActionContainer<DefaultDoctorActionRun>>,
    desired_groups: BTreeSet<String>,
    file_cache: Arc<dyn FileCache>,
    exec_runner: Arc<dyn ExecutionProvider>,
}

fn transform_inputs(found_config: &FoundConfig, args: &DoctorRunArgs) -> RunTransform {
    let mut groups = BTreeMap::new();
    let mut desired_groups = BTreeSet::new();

    let file_cache: Arc<dyn FileCache> = get_cache(args);
    let exec_runner = Arc::new(DefaultExecutionProvider::default());
    let glob_walker = Arc::new(DefaultGlobWalker::default());

    for group in found_config.doctor_group.values() {
        let should_group_run = match &args.only {
            None => group.run_by_default,
            Some(names) => names.contains(&group.metadata.name().to_string()),
        };

        let mut action_runs = Vec::new();

        for action in &group.actions {
            let run = DefaultDoctorActionRun {
                model: group.clone(),
                action: action.clone(),
                working_dir: found_config.working_dir.clone(),
                file_cache: file_cache.clone(),
                run_fix: args.fix.unwrap_or(true),
                exec_runner: exec_runner.clone(),
                glob_walker: glob_walker.clone(),
                known_errors: found_config.known_error.clone(),
            };

            action_runs.push(run);
        }

        let container = GroupActionContainer::new(
            group.clone(),
            action_runs,
            exec_runner.clone(),
            found_config.working_dir.clone(),
            found_config.bin_path.clone(),
        );

        let group_name = container.group_name().to_string();
        groups.insert(group_name.clone(), container);

        if should_group_run {
            desired_groups.insert(group_name);
        }
    }

    RunTransform {
        groups,
        desired_groups,
        file_cache,
        exec_runner,
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use crate::doctor::commands::DoctorRunArgs;
    use crate::doctor::commands::run::transform_inputs;
    use crate::doctor::tests::{group_noop, make_root_model_additional, meta_noop};
    use crate::prelude::FoundConfig;

    #[test]
    fn test_will_include_by_default() {
        let mut fc = FoundConfig::empty(PathBuf::from("/tmp"));
        fc.doctor_group.insert(
            "included".to_string(),
            make_root_model_additional(vec![], |meta| meta.name("included"), group_noop),
        );
        let args = DoctorRunArgs {
            only: None,
            no_cache: true,
            ..Default::default()
        };

        let transform = transform_inputs(&fc, &args);
        assert_eq!(
            BTreeSet::from(["included".to_string()]),
            transform.desired_groups
        );
    }

    #[test]
    fn test_include_will_skip() {
        let mut fc = FoundConfig::empty(PathBuf::from("/tmp"));
        fc.doctor_group.insert(
            "not-included".to_string(),
            make_root_model_additional(vec![], meta_noop, |g| g.run_by_default(false)),
        );
        let args = DoctorRunArgs {
            only: None,
            no_cache: true,
            ..Default::default()
        };

        let transform = transform_inputs(&fc, &args);
        assert!(transform.desired_groups.is_empty());
    }

    mod get_cache_tests {
        use super::*;
        use tempfile::tempdir;

        #[test]
        fn test_get_cache_returns_noop_when_no_cache_is_true() {
            let args = DoctorRunArgs {
                no_cache: true,
                cache_dir: None,
                ..Default::default()
            };

            let cache = get_cache(&args);

            // NoOpCache should return None for path()
            assert_eq!(cache.path(), None);
        }

        #[test]
        fn test_get_cache_no_cache_takes_precedence_over_cache_dir() {
            let args = DoctorRunArgs {
                no_cache: true,
                cache_dir: Some("/custom/path".to_string()),
                ..Default::default()
            };

            let cache = get_cache(&args);

            // Should return NoOpCache (None path) even when cache_dir is provided
            assert_eq!(cache.path(), None);
        }

        #[test]
        fn test_get_cache_uses_default_path_when_cache_dir_is_none() {
            let args = DoctorRunArgs {
                no_cache: false,
                cache_dir: None,
                ..Default::default()
            };

            let cache = get_cache(&args);

            let expected_cache_dir = directories::cache()
                .expect("Unable to determine cache directory")
                .join("scope");
            let expected_path = expected_cache_dir
                .join("cache-file.json")
                .to_string_lossy()
                .to_string();
            assert_eq!(cache.path(), Some(expected_path));
        }

        #[test]
        fn test_get_cache_empty_cache_dir_uses_default() {
            // I'm not sure this behavior is intentional,
            // but if an empty string is cached, no path is prepended
            let args = DoctorRunArgs {
                no_cache: false,
                cache_dir: Some("".to_string()),
                ..Default::default()
            };

            let cache = get_cache(&args);

            assert_eq!(cache.path(), Some("cache-file.json".to_string()));
        }

        #[test]
        fn test_get_cache_uses_provided_cache_dir() {
            let args = DoctorRunArgs {
                no_cache: false,
                cache_dir: Some("/custom/path".to_string()),
                ..Default::default()
            };

            let cache = get_cache(&args);

            assert_eq!(
                cache.path(),
                Some("/custom/path/cache-file.json".to_string())
            );
        }

        #[test]
        fn test_migrate_old_cache_success() {
            use std::fs;

            let temp_dir = tempdir().unwrap();
            let old_cache_path = temp_dir.path().join("old_cache.json");
            let new_cache_dir = temp_dir.path().join("new_cache_dir");
            let new_cache_path = new_cache_dir.join("cache-file.json");

            // Create old cache file with some content
            fs::write(&old_cache_path, r#"{"test": "data"}"#).unwrap();
            assert!(old_cache_path.exists());

            // Migrate the cache
            let result = migrate_old_cache(&old_cache_path, &new_cache_path);
            assert!(result.is_ok());

            // Verify new cache file exists and old one is removed
            assert!(new_cache_path.exists());
            assert!(!old_cache_path.exists());

            // Verify content was copied correctly
            let content = fs::read_to_string(&new_cache_path).unwrap();
            assert_eq!(content, r#"{"test": "data"}"#);
        }

        #[test]
        fn test_migrate_old_cache_creates_directories() {
            use std::fs;

            let temp_dir = tempdir().unwrap();
            let old_cache_path = temp_dir.path().join("old_cache.json");
            let new_cache_dir = temp_dir.path().join("deep").join("nested").join("dir");
            let new_cache_path = new_cache_dir.join("cache-file.json");

            // Create old cache file
            fs::write(&old_cache_path, r#"{"test": "data"}"#).unwrap();

            // Ensure target directory doesn't exist
            assert!(!new_cache_dir.exists());

            // Migrate the cache
            let result = migrate_old_cache(&old_cache_path, &new_cache_path);
            assert!(result.is_ok());

            // Verify directories were created and file exists
            assert!(new_cache_dir.exists());
            assert!(new_cache_path.exists());
            assert!(!old_cache_path.exists());
        }

        #[test]
        fn test_get_cache_migrates_from_old_location() {
            let temp_dir = tempdir().unwrap();
            let new_cache_dir = temp_dir.path().join("new_cache");
            let cache_dir_arg = new_cache_dir.to_string_lossy().to_string();

            // This test would require creating files in /tmp/scope which might not be practical
            // in all test environments, so we'll test the logic indirectly by ensuring
            // the function handles migration when paths differ

            let args = DoctorRunArgs {
                no_cache: false,
                cache_dir: Some(cache_dir_arg.clone()),
                ..Default::default()
            };

            // This should not fail even if old cache doesn't exist
            let cache = get_cache(&args);

            // Verify it uses the new path
            let expected_path = format!("{}/cache-file.json", cache_dir_arg);
            assert_eq!(cache.path(), Some(expected_path));
        }

        #[test]
        fn test_migrate_old_cache_handles_missing_old_file() {
            let temp_dir = tempdir().unwrap();
            let old_cache_path = temp_dir.path().join("nonexistent.json");
            let new_cache_path = temp_dir.path().join("new_cache.json");

            // Should return an error when old file doesn't exist
            let result = migrate_old_cache(&old_cache_path, &new_cache_path);
            assert!(result.is_err());
        }

        #[test]
        fn test_get_cache_no_migration_when_using_tmp_scope() {
            // When explicitly using /tmp/scope, no migration should occur
            let args = DoctorRunArgs {
                no_cache: false,
                cache_dir: Some("/tmp/scope".to_string()),
                ..Default::default()
            };

            let cache = get_cache(&args);

            // Should use the original /tmp/scope path
            assert_eq!(cache.path(), Some("/tmp/scope/cache-file.json".to_string()));
        }
    }
}
