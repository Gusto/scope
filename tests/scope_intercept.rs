use assert_fs::fixture::{FileWriteStr, PathChild};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::{Pid, setsid};
use predicates::boolean::PredicateBooleanExt;
use predicates::prelude::predicate;
use std::os::unix::process::CommandExt;
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

#[allow(dead_code)]
mod common;
use common::*;

#[test]
fn test_intercept_fix_succeeds_retry_succeeds() {
    let helper = ScopeTestHelper::new(
        "test_intercept_fix_succeeds_retry_succeeds",
        "intercept-known-error-with-fix",
    );

    helper
        .intercept_command(&["--yolo", "cat", "status.txt"])
        .success()
        .stdout(predicate::str::contains(
            "Known error 'missing-status-file' found",
        ))
        .stdout(predicate::str::contains("Fix succeeded, retrying command"))
        .stdout(predicate::str::contains("ready"));

    helper.clean_work_dir();
}

#[test]
fn test_intercept_fix_succeeds_retry_succeeds_via_script() {
    let helper = ScopeTestHelper::new(
        "test_intercept_fix_succeeds_retry_succeeds_via_script",
        "intercept-known-error-with-fix",
    );

    // Mirrors the shebang use case: `#!/path/to/scope-intercept bash`
    helper
        .work_dir
        .child("setup.sh")
        .write_str("#!/bin/bash\nset -e\necho 'Running setup...'\ncat status.txt\necho 'Done!'\n")
        .unwrap();

    helper
        .intercept_command(&["--yolo", "bash", "setup.sh"])
        .success()
        .stdout(predicate::str::contains("Running setup..."))
        .stdout(predicate::str::contains("Fix succeeded, retrying command"))
        .stdout(predicate::str::contains("Done!"));

    helper.clean_work_dir();
}

// Fix resolves the first failure (status.txt) but the command also requires
// other.txt, which the fix does not create. Retry still fails.
#[test]
fn test_intercept_fix_succeeds_retry_fails() {
    let helper = ScopeTestHelper::new(
        "test_intercept_fix_succeeds_retry_fails",
        "intercept-known-error-fix-retry-fails",
    );

    helper
        .work_dir
        .child("check.sh")
        .write_str("#!/bin/bash\nset -e\ncat status.txt\ncat other.txt\n")
        .unwrap();

    helper
        .intercept_command(&["--yolo", "bash", "check.sh"])
        .failure()
        .stdout(predicate::str::contains(
            "Known error 'missing-status-file' found",
        ))
        .stdout(predicate::str::contains("Fix succeeded, retrying command"))
        .stdout(predicate::str::contains("ready"));

    helper.clean_work_dir();
}

#[test]
fn test_intercept_known_error_no_fix() {
    let helper = ScopeTestHelper::new(
        "test_intercept_known_error_no_fix",
        "intercept-known-error-no-fix",
    );

    helper
        .intercept_command(&["--", "bash", "-c", "echo 'something went wrong'; exit 1"])
        .failure()
        .stdout(predicate::str::contains(
            "Known error 'something-broke' found",
        ))
        .stdout(predicate::str::contains(
            "This is a known issue. Check the wiki for manual steps.",
        ))
        .stdout(predicate::str::contains("No automatic fix available"))
        .stdout(predicate::str::contains("Fix succeeded").not());

    helper.clean_work_dir();
}

// Without --yolo, assert_cmd pipes stdin (no TTY), so inquire returns NotTTY
// which maps to KnownErrorFoundUserDenied — same as the user answering "No".
#[test]
fn test_intercept_no_tty_skips_fix() {
    let helper = ScopeTestHelper::new(
        "test_intercept_no_tty_skips_fix",
        "intercept-known-error-with-fix",
    );

    helper
        .intercept_command(&["cat", "status.txt"])
        .failure()
        .stdout(predicate::str::contains(
            "Known error 'missing-status-file' found",
        ))
        .stdout(predicate::str::contains("User denied fix"))
        .stdout(predicate::str::contains("Fix succeeded").not());

    helper.clean_work_dir();
}

#[test]
fn test_intercept_succeeds_first_try() {
    let helper = ScopeTestHelper::new(
        "test_intercept_succeeds_first_try",
        "intercept-known-error-with-fix",
    );

    helper
        .work_dir
        .child("status.txt")
        .write_str("ready\n")
        .unwrap();

    helper
        .intercept_command(&["cat", "status.txt"])
        .success()
        .stdout(predicate::str::contains("ready"))
        .stdout(predicate::str::contains("Command failed").not());

    helper.clean_work_dir();
}

// Exit code 42 is non-standard; intercept must preserve it.
#[test]
fn test_intercept_no_known_errors_match() {
    let helper = ScopeTestHelper::new(
        "test_intercept_no_known_errors_match",
        "intercept-known-error-with-fix",
    );

    helper
        .intercept_command(&["--", "bash", "-c", "echo 'totally unexpected'; exit 42"])
        .failure()
        .code(42)
        .stdout(predicate::str::contains("No known errors found"))
        .stdout(predicate::str::contains("Fix succeeded").not());

    helper.clean_work_dir();
}

// When scope-intercept wraps a long-running interactive process (e.g.
// used as a shebang on a server start script), Ctrl+C must let the
// child run its own SIGINT trap and exit cleanly. Before this fix,
// scope-intercept terminated immediately on SIGINT, closing the
// captured stdout/stderr pipes and killing the child with SIGPIPE
// before its trap could complete.
#[test]
fn test_intercept_forwards_sigint_and_waits_for_child_cleanup() {
    let helper = ScopeTestHelper::new(
        "test_intercept_forwards_sigint_and_waits_for_child_cleanup",
        "empty",
    );

    let work_dir = helper.work_dir.path().to_path_buf();
    let cleanup_marker = work_dir.join("cleanup.done");
    let ready_marker = work_dir.join("ready");

    helper
        .work_dir
        .child("trap.sh")
        .write_str(
            "#!/bin/bash\n\
             trap 'echo trapped > cleanup.done; exit 130' INT\n\
             touch ready\n\
             # sleep in short increments so the trap fires promptly\n\
             for _ in $(seq 1 30); do sleep 1; done\n",
        )
        .unwrap();

    let intercept_bin = assert_cmd::cargo::cargo_bin("scope-intercept");
    let mut command = StdCommand::new(intercept_bin);
    command
        .current_dir(&work_dir)
        .env("NO_COLOR", "1")
        .args(["--", "bash", "trap.sh"]);
    // Put scope-intercept in its own session/process group so killpg below
    // hits both it and the wrapped bash, mirroring how a terminal delivers
    // Ctrl+C to the foreground process group.
    //
    // SAFETY: pre_exec runs the closure between fork and exec, so it must
    // call only async-signal-safe functions. setsid(2) is on the POSIX
    // async-signal-safe list and touches no shared state in the parent.
    unsafe {
        command.pre_exec(|| setsid().map(|_| ()).map_err(std::io::Error::from));
    }
    let mut child = command.spawn().expect("failed to spawn scope-intercept");

    // Poll for the ready marker. There's no good cross-process "file
    // appeared" primitive without bringing in inotify/kqueue, so we
    // sleep between checks to avoid burning a core.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready_marker.exists() {
        if Instant::now() > deadline {
            child.kill().ok();
            panic!(
                "trap.sh never wrote ready marker; scope-intercept may not be running the script"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let pgid = Pid::from_raw(child.id() as i32);
    killpg(pgid, Signal::SIGINT).expect("killpg(pgid, SIGINT) failed");

    // If scope-intercept doesn't shut down within 10s, the bug is back:
    // it's hanging instead of letting the child run its trap and
    // propagating the exit.
    let status = match child
        .wait_timeout(Duration::from_secs(10))
        .expect("wait_timeout failed")
    {
        Some(status) => status,
        None => {
            child.kill().ok();
            panic!("scope-intercept did not exit within 10s after SIGINT");
        }
    };

    assert!(
        cleanup_marker.exists(),
        "child SIGINT trap did not run — cleanup.done was never created"
    );
    let body = std::fs::read_to_string(&cleanup_marker).unwrap();
    assert_eq!(body.trim(), "trapped");

    assert_eq!(
        status.code(),
        Some(130),
        "scope-intercept should propagate the child's exit code (130 for SIGINT), got {status:?}"
    );

    helper.clean_work_dir();
}
