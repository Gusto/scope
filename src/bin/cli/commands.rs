//! Command routing and utility functions for the CLI.
//!
//! This module contains all the command routing logic and helper functions
//! used by the binary, keeping the binary itself thin.

use anyhow::Result;
use clap::CommandFactory;
use colored::Colorize;
use crate::{Cli, Command, VersionArgs};
use dx_scope::{
    analyze_root, doctor_root, lint_root, print_details, report_root,
    CaptureOpts, FoundConfig, OutputCapture, OutputDestination,
    CONFIG_FILE_PATH_ENV, RUN_ID_ENV_VAR, report_stdout,
};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::BTreeMap;
use std::ffi::OsString;
use tracing::{debug, info, instrument};

lazy_static! {
    static ref SCOPE_SUBCOMMAND_REGEX: Regex = Regex::new("^scope-.*").unwrap();
}

/// Route a command to its appropriate handler.
pub async fn handle_command(found_config: &FoundConfig, command: &Command) -> Result<i32> {
    match command {
        Command::Doctor(args) => doctor_root(found_config, args).await,
        Command::Report(args) => report_root(found_config, args).await,
        Command::List => show_config(found_config).await.map(|_| 0),
        Command::Version(args) => print_version(args).await,
        Command::ExternalSubCommand(args) => exec_sub_command(found_config, args).await,
        Command::Analyze(args) => analyze_root(found_config, args).await,
        Command::Lint(args) => lint_root(found_config, args).await,
    }
}

/// Execute an external subcommand.
#[instrument("scope external-command", skip_all)]
async fn exec_sub_command(found_config: &FoundConfig, args: &[String]) -> Result<i32> {
    let mut args = args.to_owned();
    let command = match args.first() {
        None => return Err(anyhow::anyhow!("Sub command not provided")),
        Some(cmd) => {
            format!("scope-{cmd}")
        }
    };
    let _ = std::mem::replace(&mut args[0], command);

    debug!("Executing {:?}", args);

    let config_file_path = found_config.write_raw_config_to_disk()?;
    let capture = OutputCapture::capture_output(CaptureOpts {
        working_dir: &found_config.working_dir,
        args: &args,
        output_dest: OutputDestination::StandardOut,
        path: &found_config.bin_path,
        env_vars: BTreeMap::from([
            (
                CONFIG_FILE_PATH_ENV.to_string(),
                config_file_path.display().to_string(),
            ),
            (RUN_ID_ENV_VAR.to_string(), found_config.run_id.clone()),
        ]),
    })
    .await?;

    capture
        .exit_code
        .ok_or_else(|| anyhow::anyhow!("Unable to exec {}", args.join(" ")))
}

/// Show configuration and available commands.
#[instrument("scope list", skip_all)]
async fn show_config(found_config: &FoundConfig) -> Result<()> {
    info!(target: "user", "Found Resources");
    print_details(&found_config.working_dir, &found_config.raw_config).await;

    info!(target: "user", "");
    info!(target: "user", "Commands");
    print_commands(found_config).await;
    Ok(())
}

/// Print available commands (both built-in and external).
async fn print_commands(found_config: &FoundConfig) {
    if let Ok(commands) = which::which_re_in(
        SCOPE_SUBCOMMAND_REGEX.clone(),
        Some(OsString::from(&found_config.bin_path)),
    ) {
        let mut command_map = BTreeMap::new();
        for command in commands {
            let command_name = command.file_name().unwrap().to_str().unwrap().to_string();
            let command_name = command_name.replace("scope-", "");
            command_map.entry(command_name.clone()).or_insert_with(|| {
                format!("External sub-command, run `scope {command_name}` for help")
            });
        }
        for command in Cli::command().get_subcommands() {
            command_map
                .entry(command.get_name().to_string())
                .or_insert_with(|| command.get_about().unwrap_or_default().to_string());
        }

        let mut command_names: Vec<_> = command_map.keys().collect();
        command_names.sort();

        report_stdout!(
            "  {:20}{:60}",
            "Name".white().bold(),
            "Description".white().bold()
        );
        for command_name in command_names {
            let description = command_map.get(command_name.as_str()).unwrap();
            report_stdout!("- {:20}{:60}", command_name, description);
        }
    }
}

/// Print version information.
#[instrument("scope version", skip_all)]
async fn print_version(args: &VersionArgs) -> Result<i32> {
    if args.short {
        report_stdout!("scope {}", env!("CARGO_PKG_VERSION"));
    } else {
        report_stdout!(
            "{}: {:60}",
            "Version".white().bold(),
            env!("CARGO_PKG_VERSION")
        );
        report_stdout!(
            "{}: {:60}",
            "Build Timestamp".white().bold(),
            env!("VERGEN_BUILD_TIMESTAMP")
        );
        report_stdout!(
            "{}: {:60}",
            "Describe".white().bold(),
            env!("VERGEN_GIT_DESCRIBE")
        );
        report_stdout!(
            "{}: {:60}",
            "Commit SHA".white().bold(),
            env!("VERGEN_GIT_SHA")
        );
        report_stdout!(
            "{}: {:60}",
            "Commit Date".white().bold(),
            env!("VERGEN_GIT_COMMIT_DATE")
        );
    }

    Ok(0)
}
