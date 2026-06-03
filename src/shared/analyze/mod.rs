use crate::models::HelpMetadata;
use crate::prelude::{
    CaptureOpts, DefaultExecutionProvider, DoctorFix, ExecutionProvider, KnownError, OutputCapture,
    OutputDisplay, generate_env_vars,
};
use anyhow::Result;
use inquire::InquireError;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncRead};
use tracing::{debug, error, info, warn};

mod status;
pub use crate::shared::analyze::status::{AnalyzeStatus, report_result};

pub async fn process_lines<T>(
    known_errors: &BTreeMap<String, KnownError>,
    working_dir: &PathBuf,
    input: T,
    yolo: bool,
) -> Result<AnalyzeStatus>
where
    T: AsyncRead,
    T: AsyncBufReadExt,
    T: Unpin,
{
    let mut result = AnalyzeStatus::NoKnownErrorsFound;
    let mut known_errors = known_errors.clone();
    let mut line_number = 0;

    let mut lines = input.lines();

    while let Some(line) = lines.next_line().await? {
        let mut known_errors_to_remove = Vec::new();
        for (name, ke) in &known_errors {
            debug!("Checking known error {}", ke.name());
            if ke.regexes.iter().any(|r| r.is_match(&line)) {
                warn!(target: "always", "Known error '{}' found on line {}", ke.name(), line_number);
                info!(target: "always", "\t==> {}", ke.help_text);

                result = match &ke.fix {
                    Some(fix) => {
                        info!(target: "always", "found a fix!");

                        tracing_indicatif::suspend_tracing_indicatif(|| {
                            let exec_path = ke.metadata.exec_path();
                            prompt_and_run_fix(working_dir, exec_path, fix, yolo)
                        })
                        .await?
                    }
                    None => AnalyzeStatus::KnownErrorFoundNoFixFound,
                };

                known_errors_to_remove.push(name.clone());
            }
        }

        for name in known_errors_to_remove {
            known_errors.remove(&name);
        }

        line_number += 1;

        if known_errors.is_empty() {
            info!(target: "always", "All known errors detected, ignoring rest of output.");
            break;
        }
    }

    Ok(result)
}

async fn prompt_and_run_fix(
    working_dir: &PathBuf,
    exec_path: String,
    fix: &DoctorFix,
    yolo: bool,
) -> Result<AnalyzeStatus> {
    let fix_prompt = &fix.prompt.as_ref();
    let prompt_text = fix_prompt
        .map(|p| p.text.clone())
        .unwrap_or("Would you like to run it?".to_string());
    let extra_context = &fix_prompt.map(|p| p.extra_context.clone()).flatten();

    let user_accepted = if yolo {
        info!(target: "always", "{} Yes (auto-approved)", prompt_text);
        if let Some(ctx) = extra_context {
            info!(target: "always", "[{}]", ctx);
        }
        true
    } else {
        let prompt = {
            let base_prompt = inquire::Confirm::new(&prompt_text).with_default(false);
            match extra_context {
                Some(help_text) => base_prompt.with_help_message(help_text),
                None => base_prompt,
            }
        };

        match prompt.prompt() {
            Ok(accepted) => Ok::<bool, anyhow::Error>(accepted),
            Err(InquireError::NotTTY) => {
                warn!(target: "user", "Prompting user for fix, but input device is not a TTY. Skipping fix.");
                Ok(false)
            }
            Err(e) => {
                error!(target: "user", "Error prompting user for fix: {}", e);
                Err(e.into())
            }
        }?
    };

    if user_accepted {
        // failure indicates an issue with us actually executing it,
        // not the success/failure of the command itself.
        let outputs = run_fix(working_dir, &exec_path, fix).await?;
        let max_exit_code = outputs
            .iter()
            .map(|c| c.exit_code.unwrap_or(-1))
            .max()
            .unwrap();

        match max_exit_code {
            0 => Ok(AnalyzeStatus::KnownErrorFoundFixSucceeded),
            _ => {
                if let Some(help_text) = &fix.help_text {
                    error!(target: "user", "Fix Help: {}", help_text);
                }
                if let Some(help_url) = &fix.help_url {
                    error!(target: "user", "For more help, please visit {}", help_url);
                }

                Ok(AnalyzeStatus::KnownErrorFoundFixFailed)
            }
        }
    } else {
        Ok(AnalyzeStatus::KnownErrorFoundUserDenied)
    }
}

async fn run_fix(
    working_dir: &PathBuf,
    exec_path: &str,
    fix: &DoctorFix,
) -> Result<Vec<OutputCapture>> {
    let exec_runner = DefaultExecutionProvider::default();

    let commands = fix.command.as_ref().expect("Expected a command");

    let mut outputs = Vec::<OutputCapture>::new();
    for cmd in commands {
        let capture_opts = CaptureOpts {
            working_dir,
            args: &[cmd.text().to_string()],
            output_dest: OutputDisplay::VisibleWithPrefix("fixing".to_string()),
            path: exec_path,
            env_vars: generate_env_vars(),
        };
        let output = exec_runner.run_command(capture_opts).await?;
        let exit_code = output.exit_code.expect("Expected an exit code");
        outputs.push(output);
        if exit_code != 0 {
            break;
        }
    }

    Ok(outputs)
}
