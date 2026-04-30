use clap::Parser;
use dev_scope::prelude::*;
use dev_scope::shared::analyze;
use dev_scope::shared::analyze::AnalyzeStatus;
use human_panic::setup_panic;
use std::env;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::BufReader;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{Level, enabled, error, info, warn};

/// A wrapper CLI that can be used to capture output from a program, check if there are known errors
/// and let the user know.
///
/// `scope-intercept` will execute `/usr/bin/env -S [utility] [args...]` capture the output from
/// STDOUT and STDERR. After the program exits, the exit code will be checked, and if it's non-zero
/// the output will be parsed for known errors.
#[derive(Parser)]
#[clap(author, version, about)]
struct Cli {
    #[clap(flatten)]
    logging: LoggingOpts,

    /// Add additional "successful" exit codes. A sub-command that exists 0 will always be considered
    /// a success.
    #[arg(short, long)]
    successful_exit: Vec<i32>,

    #[clap(flatten)]
    config_options: ConfigOptions,

    /// Automatically approve all fix prompts without asking
    #[arg(long, short = 'y', default_value = "false")]
    yolo: bool,

    /// Command to execute withing scope-intercept.
    #[arg(required = true)]
    utility: String,

    /// Arguments to be passed to the utility
    args: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    setup_panic!();
    dotenvy::dotenv().ok();
    let exe_path = std::env::current_exe().unwrap();
    let env_path = exe_path.parent().unwrap().join("../etc/scope.env");
    dotenvy::from_path(env_path).ok();
    let opts = Cli::parse();

    let configured_logger = opts
        .logging
        .with_new_default(tracing::level_filters::LevelFilter::WARN)
        .configure_logging(&opts.config_options.get_run_id(), "intercept")
        .await;

    let exit_code = run_command(opts).await.unwrap_or_else(|e| {
        error!(target: "user", "Fatal error {:?}", e);
        1
    });

    if exit_code != 0 || enabled!(Level::DEBUG) {
        info!(target: "user", "More detailed logs at {}", configured_logger.log_location);
    }

    drop(configured_logger);
    std::process::exit(exit_code);
}

async fn run_command(opts: Cli) -> anyhow::Result<i32> {
    let yolo = opts.yolo;
    let mut command = vec![opts.utility];
    command.extend(opts.args);
    let current_dir = std::env::current_dir()?;
    let path = env::var("PATH").unwrap_or_default();

    // SIGINT/SIGTERM/SIGHUP are delivered to every member of our foreground
    // process group, including the child, so its own cleanup traps run. We
    // just need to keep ourselves alive long enough for the child to finish
    // shutting down — otherwise our captured stdout/stderr pipes close and
    // the child dies with SIGPIPE before its trap can complete.
    let interrupted = Arc::new(AtomicBool::new(false));
    for kind in [
        SignalKind::interrupt(),
        SignalKind::terminate(),
        SignalKind::hangup(),
    ] {
        let mut stream = signal(kind)?;
        let interrupted = interrupted.clone();
        tokio::spawn(async move {
            while stream.recv().await.is_some() {
                interrupted.store(true, Ordering::SeqCst);
            }
        });
    }

    let capture = OutputCapture::capture_output(CaptureOpts {
        working_dir: &current_dir,
        args: &command,
        output_dest: OutputDisplay::Visible,
        path: &path,
        env_vars: Default::default(),
    })
    .await?;

    let mut accepted_exit_codes = vec![0];
    accepted_exit_codes.extend(opts.successful_exit);

    let exit_code = capture.exit_code.unwrap_or(-1);
    if accepted_exit_codes.contains(&exit_code) {
        return Ok(exit_code);
    }

    // The user explicitly asked to quit — don't surprise them with
    // known-error prompts, retry, or bug-report offers. Just propagate the
    // child's exit code (typically 130 for SIGINT, 143 for SIGTERM).
    if interrupted.load(Ordering::SeqCst) {
        return Ok(exit_code);
    }

    error!(target: "user", "Command failed, checking for a known error");
    let found_config = opts.config_options.load_config().await.unwrap_or_else(|e| {
        error!(target: "user", "Unable to load configs from disk: {:?}", e);
        FoundConfig::empty(env::current_dir().unwrap())
    });

    let analyze_status = analyze::process_lines(
        &found_config.known_error,
        &found_config.working_dir,
        BufReader::new(Cursor::new(capture.generate_user_output())),
        yolo,
    )
    .await?;

    analyze::report_result(&analyze_status);

    let (capture, exit_code) =
        if matches!(analyze_status, AnalyzeStatus::KnownErrorFoundFixSucceeded) {
            info!(target: "always", "Fix succeeded, retrying command");
            let retry_capture = OutputCapture::capture_output(CaptureOpts {
                working_dir: &current_dir,
                args: &command,
                output_dest: OutputDisplay::Visible,
                path: &path,
                env_vars: Default::default(),
            })
            .await?;

            let retry_exit_code = retry_capture.exit_code.unwrap_or(-1);
            if accepted_exit_codes.contains(&retry_exit_code) {
                return Ok(retry_exit_code);
            }

            (retry_capture, retry_exit_code)
        } else {
            (capture, exit_code)
        };

    if !found_config.report_upload.is_empty() {
        offer_bug_report(&found_config, &command, &capture).await?;
    }
    Ok(exit_code)
}

async fn offer_bug_report(
    found_config: &FoundConfig,
    command: &[String],
    capture: &OutputCapture,
) -> anyhow::Result<()> {
    let ans = inquire::Confirm::new("Do you want to upload a bug report?")
        .with_default(false)
        .with_help_message(
            "This will allow you to share the error with other engineers for support.",
        )
        .prompt();

    if let Ok(true) = ans {
        let entrypoint = command.join(" ");
        let exec_runner = Arc::new(DefaultExecutionProvider::default());

        let builder = DefaultUnstructuredReportBuilder::new(&entrypoint, capture);

        for location in found_config.report_upload.values() {
            let mut builder = builder.clone();
            builder
                .run_and_append_additional_data(
                    found_config,
                    exec_runner.clone(),
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
    Ok(())
}
