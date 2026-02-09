//! Scope CLI binary - thin wrapper around the dx-scope library.
//!
//! This binary provides the command-line interface for scope. Most logic
//! lives in the library or cli module, keeping this file minimal.

mod cli;

use clap::{Parser, Subcommand};
use dx_scope::{
    AnalyzeArgs, ConfigOptions, DoctorArgs, LintArgs, LoggingOpts, ReportArgs,
};
use human_panic::setup_panic;
use tracing::{Level, enabled, error, info};

/// scope
///
/// Scope is a tool to enable teams to manage local machine
/// checks. An example would be a team that supports other
/// engineers may want to verify that the engineer asking
/// for support's machine is setup correctly.
#[derive(Parser)]
#[clap(author, version, about)]
pub(crate) struct Cli {
    #[clap(flatten)]
    pub logging: LoggingOpts,

    #[clap(flatten)]
    pub config: ConfigOptions,

    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub(crate) struct VersionArgs {
    #[arg(long, action)]
    pub short: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Run checks that will "checkup" your machine.
    #[clap(alias("d"))]
    Doctor(DoctorArgs),
    /// Generate a bug report based from a command that was ran
    #[clap(alias("r"))]
    Report(ReportArgs),
    /// Analyze for known errors.
    #[clap(alias("a"))]
    Analyze(AnalyzeArgs),
    /// Validate inputs, providing recommendations about configuration
    Lint(LintArgs),
    /// List the found config files, and resources detected
    #[clap(alias("l"))]
    List,
    /// Print version info and exit
    #[clap(alias("v"))]
    Version(VersionArgs),
    #[command(external_subcommand)]
    #[allow(clippy::enum_variant_names)]
    ExternalSubCommand(Vec<String>),
}

#[tokio::main]
async fn main() {
    setup_panic!();
    
    // Load environment files
    dotenvy::dotenv().ok();
    let exe_path = std::env::current_exe().unwrap();
    let env_path = exe_path.parent().unwrap().join("../etc/scope.env");
    dotenvy::from_path(env_path).ok();
    
    // Parse CLI arguments
    let opts = Cli::parse();

    // Setup logging
    let configured_logger = opts
        .logging
        .configure_logging(&opts.config.get_run_id(), "root")
        .await;
    
    // Run command
    let error_code = run_command(opts).await;

    // Show log location on error or debug
    if error_code != 0 || enabled!(Level::DEBUG) {
        info!(target: "user", "More detailed logs at {}", configured_logger.log_location);
    }

    drop(configured_logger);
    std::process::exit(error_code);
}

async fn run_command(opts: Cli) -> i32 {
    // Load configuration
    let config = match opts.config.load_config().await {
        Ok(c) => c,
        Err(e) => {
            error!(target: "user", "Failed to load configuration: {}", e);
            return 2;
        }
    };

    // Route to command handler
    cli::commands::handle_command(&config, &opts.command)
        .await
        .unwrap_or_else(|e| {
            error!(target: "user", "Critical Error. {}", e);
            1
        })
}
