use assert_cmd::Command;
use assert_cmd::assert::Assert;
use assert_fs::TempDir;
use assert_fs::prelude::PathCopy;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn setup_working_dir(dir_name: &str) -> TempDir {
    let file_path = PathBuf::from(format!(
        "{}/tests/test-cases/{}",
        env!("CARGO_MANIFEST_DIR"),
        dir_name
    ));
    // println!("Creating work dir {:?}", file_path);

    let temp = TempDir::new().unwrap();
    temp.copy_from(file_path, &["*", "**/*"]).unwrap();

    temp
}

pub struct ScopeTestHelper<'a> {
    pub work_dir: TempDir,
    name: &'a str,
    counter: AtomicUsize,
}

impl<'a> ScopeTestHelper<'a> {
    pub fn new(name: &'a str, test_dir: &'a str) -> Self {
        Self {
            work_dir: setup_working_dir(test_dir),
            name,
            counter: AtomicUsize::new(0),
        }
    }

    pub fn run_command(&self, args: &[&str]) -> Assert {
        let mut cmd = Command::cargo_bin("scope").unwrap();
        cmd.current_dir(self.work_dir.path())
            .env(
                "SCOPE_RUN_ID",
                format!(
                    "{}-{}",
                    self.name,
                    self.counter.fetch_add(1, Ordering::Relaxed)
                ),
            )
            .env("SCOPE_OUTPUT_PROGRESS", "plain")
            .env("NO_COLOR", "1")
            .args(args)
            .assert()
    }

    /// Execute `doctor run` (with optional args) with a cache-dir that's relative to the working dir.
    pub fn doctor_run(&self, args: Option<&[&str]>) -> Assert {
        let cache_args = format!("--cache-dir={}/.cache", self.work_dir.to_str().unwrap());

        let mut run_command = vec!["doctor", "run", &cache_args];

        if let Some(extra) = args {
            for entry in extra {
                run_command.push(entry)
            }
        }

        self.run_command(&run_command)
    }

    pub fn analyze_command(&self, command: &str) -> Assert {
        let run_command = vec!["analyze", "command", "--", command];

        self.run_command(&run_command)
    }

    pub fn intercept_command(&self, args: &[&str]) -> Assert {
        let mut cmd = Command::cargo_bin("scope-intercept").unwrap();
        cmd.current_dir(self.work_dir.path())
            .env(
                "SCOPE_RUN_ID",
                format!(
                    "{}-{}",
                    self.name,
                    self.counter.fetch_add(1, Ordering::Relaxed)
                ),
            )
            .env("NO_COLOR", "1")
            .args(args)
            .assert()
    }

    pub fn clean_work_dir(self) {
        self.work_dir.close().unwrap();
    }
}
