# Using dx-scope as a Library

This guide explains how to use `dx-scope` as a Rust library in your own projects, enabling programmatic access to the analyze and doctor functionality without CLI dependencies.

## Overview

The `dx-scope` crate is designed with a library-first architecture, providing:

- **Abstraction traits** for user interaction and progress reporting
- **CLI-independent options types** for configuring operations
- **Pluggable implementations** for different environments (CLI, CI, testing)

## Installation

Add `dx-scope` to your `Cargo.toml`:

```toml
[dependencies]
dx-scope = { version = "2026.1", default-features = false }
tokio = { version = "1", features = ["full"] }
```

## Core Concepts

### User Interaction Trait

The `UserInteraction` trait abstracts user prompts, allowing different behaviors in different contexts:

```rust
use dx_scope::{UserInteraction, AutoApprove, DenyAll};

// AutoApprove - automatically accepts all prompts (CI/automated environments)
let auto = AutoApprove;
assert!(auto.confirm("Apply fix?", Some("This will modify files")));

// DenyAll - automatically rejects all prompts (dry-run mode)
let deny = DenyAll;
assert!(!deny.confirm("Apply fix?", None));
```

### Progress Reporting Trait

The `ProgressReporter` trait abstracts progress visualization:

```rust
use dx_scope::{ProgressReporter, NoOpProgress};

// NoOpProgress - silent operation (library/testing use)
let progress = NoOpProgress;
progress.start_group("build", 5);
progress.advance_action("compile", "Compiling source files");
progress.finish_group();
```

## Analyze Module

The analyze module detects known errors in command output or log files.

### Basic Usage

```rust
use dx_scope::{
    AnalyzeOptions, AnalyzeInput, AnalyzeStatus,
    AutoApprove, UserInteraction,
};
use dx_scope::analyze::process_lines;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create options
    let options = AnalyzeOptions {
        known_errors: BTreeMap::new(), // Load from config
        working_dir: PathBuf::from("."),
    };

    // Process input with auto-approve for fixes
    let input = tokio::io::BufReader::new(tokio::io::stdin());
    let interaction = AutoApprove;

    let status = process_lines(
        &options.known_errors,
        &options.working_dir,
        input,
        &interaction,
    ).await?;

    match status {
        AnalyzeStatus::NoKnownErrorsFound => println!("No errors detected"),
        AnalyzeStatus::KnownErrorFoundFixSucceeded => println!("Error found and fixed"),
        AnalyzeStatus::KnownErrorFoundUserDenied => println!("Fix was declined"),
        _ => println!("Error handling completed"),
    }

    Ok(())
}
```

### Analyzing Strings Directly

```rust
use dx_scope::{AnalyzeOptions, AutoApprove};
use dx_scope::analyze::process_lines;
use tokio::io::BufReader;
use std::io::Cursor;

async fn analyze_output(output: &str) -> anyhow::Result<()> {
    let options = AnalyzeOptions::default();
    let interaction = AutoApprove;

    // Convert string to async reader
    let cursor = Cursor::new(output.to_string());
    let reader = BufReader::new(cursor);

    let status = process_lines(
        &options.known_errors,
        &options.working_dir,
        reader,
        &interaction,
    ).await?;

    println!("Analysis result: {:?}", status);
    Ok(())
}
```

## Doctor Module

The doctor module runs health checks with automatic fixes.

### Options

```rust
use dx_scope::DoctorRunOptions;

// Default options (no fixes)
let options = DoctorRunOptions::default();

// Enable automatic fixes
let options = DoctorRunOptions::with_fixes();

// CI mode (checks only, no fixes)
let options = DoctorRunOptions::ci_mode();

// Run specific groups only
let options = DoctorRunOptions::for_groups(vec![
    "build".to_string(),
    "dependencies".to_string(),
]);

// Full customization
let options = DoctorRunOptions {
    only_groups: Some(vec!["build".to_string()]),
    run_fix: true,
    cache_dir: Some("/tmp/scope-cache".into()),
    no_cache: false,
    auto_publish_report: false,
};
```

## Configuration Loading

Load scope configuration programmatically:

```rust
use dx_scope::ConfigLoadOptions;
use std::path::PathBuf;

// Default discovery (searches ancestor directories for .scope)
let options = ConfigLoadOptions::default();

// Custom working directory
let options = ConfigLoadOptions::with_working_dir(
    PathBuf::from("/path/to/project")
);

// Additional config paths
let options = ConfigLoadOptions::with_extra_config(vec![
    PathBuf::from("/custom/config/path"),
]);

// Explicit paths only (no default discovery)
let options = ConfigLoadOptions::explicit_only(vec![
    PathBuf::from("/my/config"),
]);

// Full customization
let options = ConfigLoadOptions {
    extra_config: vec![PathBuf::from("/extra/config")],
    disable_default_config: false,
    working_dir: Some(PathBuf::from("/project")),
    run_id: Some("custom-run-id".to_string()),
};
```

## Custom UserInteraction Implementation

Implement custom user interaction for your specific needs:

```rust
use dx_scope::UserInteraction;

/// Interactive implementation that logs decisions
struct LoggingInteraction {
    log_file: std::path::PathBuf,
    auto_approve: bool,
}

impl UserInteraction for LoggingInteraction {
    fn confirm(&self, prompt: &str, help_text: Option<&str>) -> bool {
        // Log the prompt
        let decision = self.auto_approve;

        let log_entry = format!(
            "Prompt: {} | Help: {:?} | Decision: {}\n",
            prompt, help_text, decision
        );

        // In real code, write to log file
        println!("{}", log_entry);

        decision
    }

    fn notify(&self, message: &str) {
        println!("[NOTIFY] {}", message);
    }
}
```

## Custom ProgressReporter Implementation

Implement custom progress reporting:

```rust
use dx_scope::ProgressReporter;

/// Progress reporter that writes to a callback
struct CallbackProgress<F: Fn(&str)> {
    callback: F,
}

impl<F: Fn(&str) + Send + Sync> ProgressReporter for CallbackProgress<F> {
    fn start_group(&self, name: &str, total_actions: usize) {
        (self.callback)(&format!("Starting {} ({} actions)", name, total_actions));
    }

    fn advance_action(&self, name: &str, description: &str) {
        (self.callback)(&format!("  {} - {}", name, description));
    }

    fn finish_group(&self) {
        (self.callback)("Group completed");
    }
}
```

## Error Handling

The library uses `anyhow::Result` for error handling:

```rust
use dx_scope::{AnalyzeOptions, AutoApprove};
use dx_scope::analyze::process_lines;
use anyhow::{Context, Result};

async fn run_analysis() -> Result<()> {
    let options = AnalyzeOptions::default();
    let interaction = AutoApprove;

    let input = tokio::fs::File::open("build.log")
        .await
        .context("Failed to open log file")?;

    let reader = tokio::io::BufReader::new(input);

    let status = process_lines(
        &options.known_errors,
        &options.working_dir,
        reader,
        &interaction,
    )
    .await
    .context("Analysis failed")?;

    Ok(())
}
```

## Testing with Mock Implementations

Use the provided implementations for testing:

```rust
#[cfg(test)]
mod tests {
    use dx_scope::{AutoApprove, DenyAll, NoOpProgress, UserInteraction};

    #[test]
    fn test_with_auto_approve() {
        let interaction = AutoApprove;

        // All prompts return true
        assert!(interaction.confirm("Any prompt?", None));
    }

    #[test]
    fn test_with_deny_all() {
        let interaction = DenyAll;

        // All prompts return false
        assert!(!interaction.confirm("Any prompt?", None));
    }

    #[tokio::test]
    async fn test_silent_progress() {
        use dx_scope::ProgressReporter;

        let progress = NoOpProgress;

        // These do nothing but don't panic
        progress.start_group("test", 5);
        progress.advance_action("action", "description");
        progress.finish_group();
    }
}
```

## Complete Example

Here's a complete example showing library usage:

```rust
use dx_scope::{
    AnalyzeOptions, AnalyzeStatus, AutoApprove, ConfigLoadOptions,
    DoctorRunOptions, NoOpProgress, UserInteraction, ProgressReporter,
};
use dx_scope::analyze::process_lines;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::io::BufReader;
use std::io::Cursor;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Load configuration
    let config_options = ConfigLoadOptions::with_working_dir(
        std::env::current_dir()?
    );

    // 2. Set up interaction mode
    let interaction = AutoApprove; // or DenyAll for dry-run
    let progress = NoOpProgress;   // silent progress

    // 3. Analyze some output
    let build_output = "error: missing dependency foo\ncompilation failed";
    let reader = BufReader::new(Cursor::new(build_output.to_string()));

    let analyze_options = AnalyzeOptions {
        known_errors: BTreeMap::new(), // would come from loaded config
        working_dir: std::env::current_dir()?,
    };

    let status = process_lines(
        &analyze_options.known_errors,
        &analyze_options.working_dir,
        reader,
        &interaction,
    ).await?;

    // 4. Handle the result
    let exit_code = status.to_exit_code();
    println!("Analysis completed with exit code: {}", exit_code);

    Ok(())
}
```

## Migration from CLI Usage

If you're migrating from CLI usage to library usage:

| CLI Flag | Library Equivalent |
|----------|-------------------|
| `--fix` | `DoctorRunOptions { run_fix: true, .. }` |
| `--no-cache` | `DoctorRunOptions { no_cache: true, .. }` |
| `--cache-dir PATH` | `DoctorRunOptions { cache_dir: Some(path), .. }` |
| `--only GROUP` | `DoctorRunOptions { only_groups: Some(vec![...]), .. }` |
| `-C DIR` | `ConfigLoadOptions { working_dir: Some(dir), .. }` |
| `--extra-config PATH` | `ConfigLoadOptions { extra_config: vec![path], .. }` |
| `--disable-default-config` | `ConfigLoadOptions { disable_default_config: true, .. }` |
| `-y` / `--yes` | Use `AutoApprove` implementation |

## Thread Safety

All provided types implement `Send + Sync`:

```rust
use dx_scope::{AutoApprove, DenyAll, NoOpProgress, InquireInteraction};

fn assert_send_sync<T: Send + Sync>() {}

// All these compile successfully
assert_send_sync::<AutoApprove>();
assert_send_sync::<DenyAll>();
assert_send_sync::<NoOpProgress>();
assert_send_sync::<InquireInteraction>();
```

## Feature Flags

The crate supports these feature configurations:

- Default features include CLI support via `inquire`
- For library-only usage without CLI dependencies, you can exclude features (when available)

## API Stability

The library API follows semantic versioning:

- Types in the crate root (`dx_scope::*`) are considered stable
- Types in `internal` module are stable for implementing traits
- Types in `prelude` are re-exports and follow the same stability as their source modules
