use super::redact::Redactor;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use colored::Colorize;
use derive_builder::Builder;
use mockall::automock;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use thiserror::Error;
use tokio::io;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::RwLock;
use tracing::{Level, debug, error, info, instrument};
use which::which_in;

#[derive(Debug, Default)]
struct RwLockOutput {
    output: RwLock<Vec<(DateTime<Utc>, String)>>,
}

impl RwLockOutput {
    async fn add_line(&self, line: &str) {
        let mut stdout = self.output.write().await;
        stdout.push((Utc::now(), line.to_string()));
    }
}

#[derive(Clone, Default, Builder, Debug)]
#[builder(setter(into))]
pub struct OutputCapture {
    #[builder(default)]
    pub working_dir: PathBuf,
    #[builder(default)]
    stdout: Vec<(DateTime<Utc>, String)>,
    #[builder(default)]
    stderr: Vec<(DateTime<Utc>, String)>,
    #[builder(default)]
    pub exit_code: Option<i32>,
    #[builder(default)]
    pub start_time: DateTime<Utc>,
    #[builder(default)]
    pub end_time: DateTime<Utc>,
    #[builder(default)]
    pub command: String,
}

#[derive(Clone, Debug)]
pub enum OutputDisplay {
    Visible,
    VisibleWithPrefix(String),
    Silent,
}

struct StreamCapture<R: io::AsyncRead + Unpin> {
    reader: R,
    writer: Arc<RwLock<Box<dyn std::io::Write + Send + Sync>>>,
    level: Level,
    dest: OutputDisplay,
}

impl<R: io::AsyncRead + Unpin> StreamCapture<R> {
    async fn capture_output(self) -> Result<Vec<(DateTime<Utc>, String)>, anyhow::Error> {
        let captured = RwLockOutput::default();

        let mut reader = BufReader::new(self.reader).lines();
        while let Some(line) = reader.next_line().await? {
            captured.add_line(&line).await;

            // Always trace — telemetry is not optional
            match self.level {
                Level::ERROR => error!("{}", line),
                _ => info!("{}", line),
            }

            // Optionally display to user
            match &self.dest {
                OutputDisplay::Visible => {
                    writeln!(self.writer.write().await, "{line}\r").ok();
                }
                OutputDisplay::VisibleWithPrefix(prefix) => {
                    writeln!(
                        self.writer.write().await,
                        "{}:  {}\r",
                        prefix.dimmed(),
                        line
                    )
                    .ok();
                }
                OutputDisplay::Silent => {}
            };
        }

        Ok::<_, anyhow::Error>(captured.output.into_inner())
    }
}

#[derive(Error, Debug)]
pub enum CaptureError {
    #[error("Unable to process file. {error:?}")]
    IoError {
        #[from]
        error: std::io::Error,
    },
    #[error("File {name} was not executable or it did not exist.")]
    MissingShExec { name: String },
    #[error("Unable to parse UTF-8 output. {error:?}")]
    FromUtf8Error {
        #[from]
        error: std::string::FromUtf8Error,
    },
}

#[automock]
#[async_trait]
pub trait ExecutionProvider: Send + Sync {
    async fn run_command<'a>(&self, opts: CaptureOpts<'a>) -> Result<OutputCapture, CaptureError>;

    async fn run_for_output(&self, path: &str, workdir: &Path, command: &str) -> String {
        let args: Vec<String> = command.split(' ').map(|x| x.to_string()).collect();
        let result = self
            .run_command(CaptureOpts {
                working_dir: workdir,
                args: &args,
                output_dest: OutputDisplay::Silent,
                path,
                env_vars: Default::default(),
            })
            .await;

        match result {
            Ok(capture) => capture.generate_user_output(),
            Err(error) => error.to_string(),
        }
    }
}

#[derive(Default, Debug)]
pub struct DefaultExecutionProvider {}

#[async_trait]
impl ExecutionProvider for DefaultExecutionProvider {
    async fn run_command<'a>(&self, opts: CaptureOpts<'a>) -> Result<OutputCapture, CaptureError> {
        OutputCapture::capture_output(opts).await
    }
}

pub struct CaptureOpts<'a> {
    pub working_dir: &'a Path,
    pub env_vars: BTreeMap<String, String>,
    pub path: &'a str,
    pub args: &'a [String],
    pub output_dest: OutputDisplay,
}

impl<'a> CaptureOpts<'a> {
    fn command(&self) -> String {
        self.args.join(" ")
    }
}

impl OutputCapture {
    #[instrument(skip_all)]
    pub async fn capture_output(opts: CaptureOpts<'_>) -> Result<Self, CaptureError> {
        check_pre_exec(&opts)?;
        let args = opts.args.to_vec();

        debug!("Executing PATH={} {:?}", &opts.path, &args);

        let start_time = Utc::now();
        let mut command = tokio::process::Command::new("/usr/bin/env");
        let mut child = command
            .arg("-S")
            .args(args)
            .env("PATH", opts.path)
            .envs(&opts.env_vars)
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .current_dir(opts.working_dir)
            .spawn()?;

        // capture stdout
        let stdout = child.stdout.take().expect("stdout to be available");
        let stdout_stream = StreamCapture {
            reader: stdout,
            writer: crate::shared::prelude::STDOUT_WRITER.clone(),
            level: Level::INFO,
            dest: opts.output_dest.clone(),
        };
        let stdout = stdout_stream.capture_output();

        // capture stderr
        let stderr = child.stderr.take().expect("stderr to be available");
        let stderr_stream = StreamCapture {
            reader: stderr,
            writer: crate::shared::prelude::STDERR_WRITER.clone(),
            level: Level::ERROR,
            dest: opts.output_dest.clone(),
        };
        let stderr = stderr_stream.capture_output();

        // wait for app to exit
        let (command_result, wait_stdout, wait_stderr) = tokio::join!(child.wait(), stdout, stderr);
        let end_time = Utc::now();
        debug!("join result {:?}", command_result);

        let captured_stdout = wait_stdout.unwrap_or_default();
        let captured_stderr = wait_stderr.unwrap_or_default();

        Ok(Self {
            working_dir: opts.working_dir.to_path_buf(),
            stdout: captured_stdout,
            stderr: captured_stderr,
            exit_code: command_result.ok().and_then(|x| x.code()),
            start_time,
            end_time,
            command: opts.command(),
        })
    }

    pub fn generate_output(&self) -> String {
        let stdout: Vec<_> = self
            .stdout
            .iter()
            .map(|(time, line)| {
                let offset: Duration = *time - self.start_time;
                (*time, format!("{offset} OUT: {line}"))
            })
            .collect();

        let stderr: Vec<_> = self
            .stderr
            .iter()
            .map(|(time, line)| {
                let offset: Duration = *time - self.start_time;
                (*time, format!("{offset} ERR: {line}"))
            })
            .collect();

        let mut output = Vec::new();
        output.extend(stdout);
        output.extend(stderr);

        output.sort_by(|(l_time, _), (r_time, _)| l_time.cmp(r_time));

        let text: String = output
            .iter()
            .map(|(_, line)| line.clone())
            .collect::<Vec<_>>()
            .join("\n");

        Redactor::new().redact_text(&text).to_string()
    }

    pub fn generate_user_output(&self) -> String {
        let mut output = Vec::new();
        output.extend(self.stdout.iter());
        output.extend(self.stderr.iter());

        output.sort_by(|(l_time, _), (r_time, _)| l_time.cmp(r_time));

        output
            .iter()
            .map(|(_, line)| line.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn get_stdout(&self) -> String {
        self.stdout
            .iter()
            .map(|(_, line)| line.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn get_stderr(&self) -> String {
        self.stderr
            .iter()
            .map(|(_, line)| line.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn check_pre_exec(opts: &CaptureOpts) -> Result<(), CaptureError> {
    let command = opts.command();
    let found_binary = match command.split(' ').collect::<Vec<_>>().first() {
        None => return Err(CaptureError::MissingShExec { name: command }),
        Some(path) => which_in(path, Some(OsString::from(opts.path)), opts.working_dir),
    };

    let path = match found_binary {
        Ok(path) => path,
        Err(e) => {
            debug!("Unable to find binary {:?}", e);
            return Err(CaptureError::MissingShExec { name: command });
        }
    };

    if !path.exists() {
        return Err(CaptureError::MissingShExec {
            name: path.display().to_string(),
        });
    }
    let metadata = std::fs::metadata(&path)?;
    let permissions = metadata.permissions().mode();
    if permissions & 0x700 == 0 {
        return Err(CaptureError::MissingShExec {
            name: path.display().to_string(),
        });
    }

    Ok(())
}
