use super::error::AnalyzeError;
use crate::cli::InquireInteraction;
use crate::prelude::{
    CaptureError, CaptureOpts, DefaultExecutionProvider, ExecutionProvider, OutputDestination,
};
use crate::shared::analyze;
use crate::shared::prelude::FoundConfig;
use anyhow::Result;
use clap::{Args, Subcommand};
use std::env;
use std::io::Cursor;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{BufReader, Stdin};

#[derive(Debug, Args)]
pub struct AnalyzeArgs {
    #[clap(subcommand)]
    command: AnalyzeCommands,
}

#[derive(Debug, Subcommand)]
enum AnalyzeCommands {
    /// Reads a log file and detects errors in it
    #[clap(alias("log"))]
    Logs(AnalyzeLogsArgs),

    /// Runs a command and detects errors in the output
    #[clap()]
    Command(AnalyzeCommandArgs),
}

#[derive(Debug, Args)]
struct AnalyzeLogsArgs {
    /// Location that the logs should be searched, for stdin use '-'
    location: String,
}

#[derive(Debug, Args)]
struct AnalyzeCommandArgs {
    /// The command to run
    #[arg(last = true, required = true)]
    command: Vec<String>,
}

pub async fn analyze_root(found_config: &FoundConfig, args: &AnalyzeArgs) -> Result<i32> {
    match &args.command {
        AnalyzeCommands::Logs(args) => analyze_logs(found_config, args).await,
        AnalyzeCommands::Command(args) => analyze_command(found_config, args).await,
    }
}

async fn analyze_logs(found_config: &FoundConfig, args: &AnalyzeLogsArgs) -> Result<i32> {
    let interaction = InquireInteraction;
    let result = match args.location.as_str() {
        "-" => {
            analyze::process_lines(
                &found_config.known_error,
                &found_config.working_dir,
                read_from_stdin().await?,
                &interaction,
            )
            .await?
        }
        file_path => {
            analyze::process_lines(
                &found_config.known_error,
                &found_config.working_dir,
                read_from_file(file_path).await?,
                &interaction,
            )
            .await?
        }
    };

    analyze::report_result(&result);
    Ok(result.to_exit_code())
}

async fn analyze_command(found_config: &FoundConfig, args: &AnalyzeCommandArgs) -> Result<i32> {
    let exec_runner = DefaultExecutionProvider::default();
    let interaction = InquireInteraction;

    let command = args.command.clone();
    let path = env::var("PATH").unwrap_or_default();

    let capture_opts: CaptureOpts = CaptureOpts {
        working_dir: &found_config.working_dir,
        env_vars: Default::default(),
        path: &path,
        args: &command,
        output_dest: OutputDestination::StandardOutWithPrefix("analyzing".to_string()),
    };

    let result = analyze::process_lines(
        &found_config.known_error,
        &found_config.working_dir,
        read_from_command(&exec_runner, capture_opts).await?,
        &interaction,
    )
    .await?;

    analyze::report_result(&result);
    Ok(result.to_exit_code())
}

async fn read_from_command(
    exec_runner: &DefaultExecutionProvider,
    capture_opts: CaptureOpts<'_>,
) -> Result<BufReader<Cursor<String>>, CaptureError> {
    let output = exec_runner.run_command(capture_opts).await?;

    let cursor = Cursor::new(output.generate_user_output());

    Ok(BufReader::new(cursor))
}

async fn read_from_stdin() -> Result<BufReader<Stdin>, AnalyzeError> {
    Ok(BufReader::new(tokio::io::stdin()))
}

async fn read_from_file(file_name: &str) -> Result<BufReader<File>, AnalyzeError> {
    let file_path = PathBuf::from(file_name);
    if !file_path.exists() {
        return Err(AnalyzeError::FileNotFound {
            file_name: file_name.to_string(),
        });
    }
    Ok(BufReader::new(File::open(file_path).await?))
}
