//! Simple example of using dx-scope's doctor functionality as a library.
//!
//! This example demonstrates how to programmatically run health checks
//! without using the CLI.

use dx_scope::doctor;
use dx_scope::DoctorRunOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load configuration from the current directory
    println!("Loading scope configuration...");
    
    // Note: For library usage, you can either:
    // 1. Create an empty config and populate it programmatically
    // 2. Load from disk using your own config loading logic
    let working_dir = std::env::current_dir()?;
    let config = dx_scope::FoundConfig::empty(working_dir);

    println!("Found {} doctor groups", config.doctor_group.len());
    println!();

    // Option 1: Run all checks without fixes (CI mode)
    println!("=== Running checks in CI mode (no fixes) ===");
    let ci_options = DoctorRunOptions::ci_mode();
    let result = doctor::run(&config, ci_options).await?;

    println!("✓ Succeeded: {}", result.succeeded_groups.len());
    println!("✗ Failed:    {}", result.failed_group.len());
    println!("⊘ Skipped:   {}", result.skipped_group.len());
    println!("Overall success: {}", result.did_succeed);
    println!();

    // Option 2: Run specific groups with auto-fix enabled
    println!("=== Running specific groups with auto-fix ===");
    let targeted_options = DoctorRunOptions::for_groups(vec![
        "example-group".to_string(),
    ]);

    match doctor::run(&config, targeted_options).await {
        Ok(result) => {
            println!("Targeted run completed:");
            println!("  Succeeded: {:?}", result.succeeded_groups);
            println!("  Failed:    {:?}", result.failed_group);
        }
        Err(e) => {
            println!("Run failed: {}", e);
        }
    }

    println!();
    println!("=== Listing available checks ===");
    let groups = doctor::list(&config).await?;
    for group in groups {
        println!("Group: {}", group.metadata.name());
        println!("  Description: {}", group.metadata.description());
        println!("  Actions: {}", group.actions.len());
    }

    Ok(())
}
