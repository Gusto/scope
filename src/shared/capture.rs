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
use std::sync::LazyLock;
use thiserror::Error;
use tokio::io;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::RwLock;
use tracing::{Instrument, Level, debug, error, info, instrument};
use which::which_in;

// Child commands inherit the terminal. A Scope confirmation must wait for
// running commands to release it, while independent commands may run together.
pub(crate) static TERMINAL_ACCESS: LazyLock<RwLock<()>> = LazyLock::new(RwLock::default);

pub(crate) fn with_terminal_prompt<R>(prompt: impl FnOnce() -> R) -> R {
    // The CLI uses a multi-thread runtime. Keep the old prompt behavior for
    // library callers using a current-thread runtime.
    if tokio::runtime::Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        tokio::task::block_in_place(|| {
            let _terminal_access = TERMINAL_ACCESS.blocking_write();
            prompt()
        })
    } else {
        prompt()
    }
}

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
        let _terminal_access = TERMINAL_ACCESS.read().await;
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
            .kill_on_drop(true)
            .spawn()?;

        // Both pipes must be read while the child is running. JoinSet aborts the
        // readers if the capture is cancelled; kill_on_drop stops the child.
        let mut readers = tokio::task::JoinSet::new();

        // capture stdout
        let stdout = child.stdout.take().expect("stdout to be available");
        let stdout_stream = StreamCapture {
            reader: stdout,
            writer: crate::shared::prelude::STDOUT_WRITER.clone(),
            level: Level::INFO,
            dest: opts.output_dest.clone(),
        };
        readers
            .spawn(async move { (true, stdout_stream.capture_output().await) }.in_current_span());

        // capture stderr
        let stderr = child.stderr.take().expect("stderr to be available");
        let stderr_stream = StreamCapture {
            reader: stderr,
            writer: crate::shared::prelude::STDERR_WRITER.clone(),
            level: Level::ERROR,
            dest: opts.output_dest.clone(),
        };
        readers
            .spawn(async move { (false, stderr_stream.capture_output().await) }.in_current_span());

        // wait for app to exit
        let command_result = child.wait().await;
        let mut captured_stdout = Vec::new();
        let mut captured_stderr = Vec::new();
        while let Some(read_result) = readers.join_next().await {
            match read_result {
                Ok((true, Ok(lines))) => captured_stdout = lines,
                Ok((false, Ok(lines))) => captured_stderr = lines,
                _ => {}
            }
        }
        let end_time = Utc::now();
        debug!("join result {:?}", command_result);

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

        output.sort_by_key(|(l_time, _)| *l_time);

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

        output.sort_by_key(|(l_time, _)| *l_time);

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

#[cfg(test)]
mod terminal_tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn child_waits_for_terminal_and_drains_large_output() {
        let prompt = TERMINAL_ACCESS.write().await;
        let child = tokio::spawn(async {
            let workdir = std::env::temp_dir();
            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                "yes output | head -n 20000".to_string(),
            ];
            OutputCapture::capture_output(CaptureOpts {
                working_dir: &workdir,
                env_vars: BTreeMap::new(),
                path: "/usr/bin:/bin",
                args: &args,
                output_dest: OutputDisplay::Silent,
            })
            .await
            .unwrap()
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!child.is_finished());
        drop(prompt);
        let output = tokio::time::timeout(std::time::Duration::from_secs(5), child)
            .await
            .expect("child should finish after prompt releases terminal")
            .unwrap();
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(output.get_stdout().lines().count(), 20000);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_capture_stops_child_command() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        let marker = directory.path().join("marker");
        let workdir = directory.path().to_path_buf();
        let ready_value = ready.to_string_lossy().into_owned();
        let marker_value = marker.to_string_lossy().into_owned();
        let capture = tokio::spawn(async move {
            let args = vec![
                "sh".to_string(),
                "-c".to_string(),
                "touch \"$READY\"; sleep 1; touch \"$MARKER\"".to_string(),
            ];
            OutputCapture::capture_output(CaptureOpts {
                working_dir: &workdir,
                env_vars: BTreeMap::from([
                    ("READY".to_string(), ready_value),
                    ("MARKER".to_string(), marker_value),
                ]),
                path: "/usr/bin:/bin",
                args: &args,
                output_dest: OutputDisplay::Silent,
            })
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !ready.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("child should start");
        capture.abort();
        assert!(capture.await.unwrap_err().is_cancelled());
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert!(!marker.exists());
    }
}
