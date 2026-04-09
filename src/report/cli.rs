use crate::prelude::{
    DefaultExecutionProvider, DefaultUnstructuredReportBuilder, ReportRenderer,
    UnstructuredReportBuilder,
};
use crate::shared::prelude::{CaptureOpts, FoundConfig, OutputCapture, OutputDisplay};
use anyhow::Result;
use clap::Args;
use std::sync::Arc;
use tracing::{instrument, warn};

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Where the report will be generated, if not set a location will be determined at runtime.
    #[arg(long, short = 'o')]
    report_location: Option<String>,

    /// The command that should be run and reported on
    #[arg(last = true, required = true)]
    command: Vec<String>,
}

#[instrument("scope report", skip_all)]
pub async fn report_root(found_config: &FoundConfig, args: &ReportArgs) -> Result<i32> {
    let capture = OutputCapture::capture_output(CaptureOpts {
        working_dir: &found_config.working_dir,
        args: &args.command,
        output_dest: OutputDisplay::Silent,
        path: &found_config.bin_path,
        env_vars: Default::default(),
    })
    .await?;
    let exit_code = capture.exit_code.unwrap_or(-1);

    let entrypoint = args.command.join(" ");
    let exec_runner = Arc::new(DefaultExecutionProvider::default());

    let builder = DefaultUnstructuredReportBuilder::new(&entrypoint, &capture);

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

    Ok(exit_code)
}
