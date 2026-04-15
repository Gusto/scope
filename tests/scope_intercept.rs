use assert_fs::fixture::{FileWriteStr, PathChild};
use predicates::boolean::PredicateBooleanExt;
use predicates::prelude::predicate;

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
