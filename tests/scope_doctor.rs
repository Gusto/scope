use assert_fs::fixture::{FileWriteStr, PathChild};
use predicates::boolean::PredicateBooleanExt;
use predicates::prelude::predicate;

#[allow(dead_code)]
mod common;
use common::*;

#[test]
fn test_run_check_fix_then_recheck_succeeds() {
    let helper = ScopeTestHelper::new(
        "test_run_check_fix_then_recheck_succeeds",
        "simple-check-fix",
    );

    let results = helper.doctor_run(None);
    results.success().stdout(predicate::str::contains(
        "Check initially failed, fix was successful, group: \"path-exists\", name: \"file-exists\"",
    ));

    helper.clean_work_dir();
}

#[test]
fn test_run_check_fix_then_recheck_fails_shows_output() {
    let helper = ScopeTestHelper::new(
        "test_run_check_fix_then_recheck_fails_shows_output",
        "simple-check-fail",
    );

    let results = helper.doctor_run(None);
    results.failure()
        .stdout(predicate::str::contains(
        "Check initially failed, fix ran, verification failed, group: \"path-exists\", name: \"file-exists\"",
    ))
        .stdout(predicate::str::contains("file-mod.txt"))
        .stdout(predicate::str::contains("path-exists/file-exists:  /"))
        .stdout(predicate::str::contains("path-exists/file-exists:  found file /"))
        .stdout(predicate::str::contains("Summary: 0 groups succeeded, 1 groups failed"));

    helper.clean_work_dir();
}

#[test]
fn test_doctor_list() {
    let test_helper = ScopeTestHelper::new("test_doctor_list", "simple-check-fix");
    let result = test_helper.run_command(&["doctor", "list"]);

    result
        .success()
        .stdout(predicate::str::contains("Included"))
        .stdout(predicate::str::contains("ScopeDoctorGroup/path-exists"))
        .stdout(predicate::str::contains("Check if file exists"))
        .stdout(predicate::str::contains(".scope/group.yaml"));

    test_helper.clean_work_dir();
}

#[test]
fn test_doctor_list_includes_when_required_groups() {
    let test_helper = ScopeTestHelper::new(
        "test_doctor_list_includes_when_required_groups",
        "list-with-when-required",
    );
    let result = test_helper
        .run_command(&["doctor", "list"])
        .success()
        .stdout(predicate::str::contains(
            "ScopeDoctorGroup/when-required-group",
        ))
        .stdout(predicate::str::contains("ScopeDoctorGroup/default-group"));

    let output = result.get_output().stdout.clone();
    let stdout = String::from_utf8(output).expect("stdout should be utf8");

    let default_line = stdout
        .lines()
        .find(|line| line.contains("ScopeDoctorGroup/default-group"))
        .expect("default-group line should be present");
    assert!(
        default_line.contains("Yes"),
        "expected default-group to be marked Included: Yes, got: {default_line}"
    );

    let when_required_line = stdout
        .lines()
        .find(|line| line.contains("ScopeDoctorGroup/when-required-group"))
        .expect("when-required-group line should be present");
    assert!(
        when_required_line.contains("No"),
        "expected when-required-group to be marked Included: No, got: {when_required_line}"
    );

    test_helper.clean_work_dir();
}

#[test]
fn test_able_to_limit_run() {
    let test_helper = ScopeTestHelper::new("test_able_to_limit_run", "two-groups");
    let result = test_helper.doctor_run(Some(&["--only=group-one"]));

    result.success().stdout(predicate::str::contains(
        "Check initially failed, fix was successful, group: \"group-one\", name: \"file-exists\"",
    ));

    let result = test_helper.doctor_run(Some(&["--only=group-two"]));

    result.success().stdout(predicate::str::contains(
        "Check initially failed, fix was successful, group: \"group-two\", name: \"file-exists\"",
    ));

    test_helper.clean_work_dir();
}

#[test]
fn test_nonexistant_file() {
    let test_helper = ScopeTestHelper::new("test_nonexistant_file", "paths");
    let result = test_helper.doctor_run(None);

    result.success().stdout(predicate::str::contains(
        "Check initially failed, fix was successful, group: \"path-checks\", name: \"does-not-exist\"",
    ));

    test_helper.clean_work_dir();
}

#[test]
fn test_cache_invalidation() {
    let test_helper = ScopeTestHelper::new("test_cache_invalidation", "file-cache-check");

    test_helper
        .work_dir
        .child("foo/requirements.txt")
        .write_str("initial cache")
        .unwrap();

    let result = test_helper.doctor_run(None);

    result
        .success()
        .stdout(predicate::str::contains(
            "Check initially failed, fix was successful, group: \"setup\", name: \"install\"",
        ))
        .stdout(predicate::str::contains("Failed to write updated cache to disk").not());

    let result = test_helper.doctor_run(None);

    result
        .success()
        .stdout(predicate::str::contains(
            "Check was successful, group: \"setup\", name: \"install\"",
        ))
        .stdout(predicate::str::contains("Failed to write updated cache to disk").not());

    test_helper
        .work_dir
        .child("foo/requirements.txt")
        .write_str("cache buster")
        .unwrap();

    let result = test_helper.doctor_run(Some(&["--only=setup"]));

    result
        .success()
        .stdout(predicate::str::contains(
            "Check initially failed, fix was successful, group: \"setup\", name: \"install\"",
        ))
        .stdout(predicate::str::contains("Failed to write updated cache to disk").not());
}

#[test]
fn test_templated_file_paths() {
    let test_helper = ScopeTestHelper::new("test_templated_file_paths", "templated-check-path");
    let result = test_helper.doctor_run(None);

    result.success().stdout(predicate::str::contains(
        "INFO Check initially failed, fix was successful, group: \"templated\", name: \"hushlogin\"",
    ));
}

#[test]
fn test_no_tasks_found() {
    let test_helper = ScopeTestHelper::new("test_no_tasks_found", "empty");
    let result = test_helper.doctor_run(Some(&["--only=bogus-group"]));

    result.success().stdout(predicate::str::contains(
        "Could not find any tasks to execute",
    ));
}

#[test]
fn test_no_tasks_warning_suppressed_when_everything_runnable_was_skipped() {
    // Regression test for a mutation-testing survivor: `all_paths.is_empty() &&
    // skipped_groups.is_empty()` must stay an `&&` — skipping every group that would otherwise
    // have run is not the same as finding nothing to do, and shouldn't be reported as such.
    let test_helper = ScopeTestHelper::new(
        "test_no_tasks_warning_suppressed_when_everything_runnable_was_skipped",
        "two-groups",
    );
    let result = test_helper.doctor_run(Some(&["--skip=group-one", "--skip=group-two"]));

    result
        .success()
        .stdout(predicate::str::contains("Could not find any tasks to execute").not())
        .stdout(predicate::str::contains(
            "Group skipped, group: \"group-one\"",
        ))
        .stdout(predicate::str::contains(
            "Group skipped, group: \"group-two\"",
        ))
        .stdout(predicate::str::contains(
            "Summary: 0 groups succeeded, 2 groups skipped",
        ));

    test_helper.clean_work_dir();
}

#[test]
fn test_skip_unknown_group_name_warns() {
    let test_helper = ScopeTestHelper::new("test_skip_unknown_group_name_warns", "two-groups");
    let result = test_helper.doctor_run(Some(&["--skip=does-not-exist"]));

    result.success().stdout(predicate::str::contains(
        "--skip does-not-exist does not match any known group, ignoring",
    ));

    test_helper.clean_work_dir();
}

#[test]
fn test_skip_only_unknown_group_name_warns() {
    let test_helper = ScopeTestHelper::new("test_skip_only_unknown_group_name_warns", "two-groups");
    let result = test_helper.doctor_run(Some(&["--skip-only=does-not-exist"]));

    result.success().stdout(predicate::str::contains(
        "--skip-only does-not-exist does not match any known group, ignoring",
    ));

    test_helper.clean_work_dir();
}

#[test]
fn test_sub_command_works() {
    let test_helper = ScopeTestHelper::new("test_sub_command_works", "command-paths");
    let result = test_helper.doctor_run(None);

    result.success().stdout(predicate::str::contains(
        "Check initially failed, fix was successful, group: \"fail-then-fix\", name: \"task\"",
    ));
    test_helper.clean_work_dir();
}

#[test]
fn test_group_skip_boolean_true() {
    let test_helper = ScopeTestHelper::new("test_group_skip_boolean_true", "group-skip-boolean");
    let result = test_helper.doctor_run(None);

    result
        .success()
        .stdout(predicate::str::contains(
            "Group skipped, group: \"group-skip-boolean\"",
        ))
        .stdout(predicate::str::contains("This check should not run").not())
        .stdout(predicate::str::contains("This fix should not run").not())
        .stdout(predicate::str::contains(
            "Summary: 0 groups succeeded, 1 groups skipped",
        ));

    test_helper.clean_work_dir();
}

#[test]
fn test_group_skip_boolean_false() {
    let test_helper =
        ScopeTestHelper::new("test_group_skip_boolean_false", "group-skip-boolean-false");
    let result = test_helper.doctor_run(None);

    result
        .success()
        .stdout(predicate::str::contains("Group skipped").not())
        .stdout(predicate::str::contains(
            "Check was successful, group: \"group-skip-boolean-false\", name: \"should-run\"",
        ))
        .stdout(predicate::str::contains("Summary: 1 groups succeeded"));

    test_helper.clean_work_dir();
}

#[test]
fn test_group_skip_default() {
    let test_helper = ScopeTestHelper::new("test_group_skip_default", "group-skip-default");
    let result = test_helper.doctor_run(None);

    result
        .success()
        .stdout(predicate::str::contains("Group skipped").not())
        .stdout(predicate::str::contains(
            "Check was successful, group: \"group-skip-default\", name: \"should-run\"",
        ))
        .stdout(predicate::str::contains("Summary: 1 groups succeeded"));

    test_helper.clean_work_dir();
}

#[test]
fn test_group_skip_command_success() {
    let test_helper = ScopeTestHelper::new("test_group_skip_command_success", "group-skip-command");
    let result = test_helper.doctor_run(None);

    result
        .success()
        .stdout(predicate::str::contains(
            "Group skipped, group: \"group-skip-command\"",
        ))
        .stdout(predicate::str::contains("This check should not run").not())
        .stdout(predicate::str::contains("This fix should not run").not())
        .stdout(predicate::str::contains(
            "Summary: 0 groups succeeded, 1 groups skipped",
        ));

    test_helper.clean_work_dir();
}

#[test]
fn test_group_skip_command_fail() {
    let test_helper =
        ScopeTestHelper::new("test_group_skip_command_fail", "group-skip-command-fail");
    let result = test_helper.doctor_run(None);

    result
        .success()
        .stdout(predicate::str::contains("Group skipped").not())
        .stdout(predicate::str::contains(
            "Check was successful, group: \"group-skip-command-fail\", name: \"should-run\"",
        ))
        .stdout(predicate::str::contains("Summary: 1 groups succeeded"));

    test_helper.clean_work_dir();
}

#[test]
fn test_group_skip_subsequent_groups_run() {
    let test_helper = ScopeTestHelper::new(
        "test_group_skip_subsequent_groups_run",
        "group-skip-allows-later-groups-to-run",
    );
    let result = test_helper.doctor_run(None);

    result
        .success()
        .stdout(predicate::str::contains(
            "Group skipped, group: \"group-skipped\"",
        ))
        .stdout(predicate::str::contains(
            "Check was successful, group: \"group-runs\", name: \"should-run\"",
        ))
        .stdout(predicate::str::contains(
            "Summary: 2 groups succeeded, 1 groups skipped",
        ));

    test_helper.clean_work_dir();
}

#[test]
fn test_group_skip_boolean_trims_dependency() {
    let test_helper = ScopeTestHelper::new(
        "test_group_skip_boolean_trims_dependency",
        "group-skip-boolean-trims-dependency",
    );
    let result = test_helper.doctor_run(None);

    result
        .success()
        .stdout(predicate::str::contains(
            "Group skipped, group: \"skip-parent\"",
        ))
        .stdout(predicate::str::contains("should not run").not())
        .stdout(predicate::str::contains("group: \"needs-dep\"").not())
        .stdout(predicate::str::contains(
            "Summary: 0 groups succeeded, 1 groups skipped",
        ));

    test_helper.clean_work_dir();
}

#[test]
fn test_skip_flag_trims_exclusive_dependencies_but_keeps_shared_dependency() {
    let test_helper = ScopeTestHelper::new(
        "test_skip_flag_trims_exclusive_dependencies_but_keeps_shared_dependency",
        "skip-with-deps",
    );
    let result = test_helper.doctor_run(Some(&["--skip=a"]));

    result
        .success()
        .stdout(predicate::str::contains("Group skipped, group: \"a\""))
        .stdout(predicate::str::contains("group: \"b\"").not())
        .stdout(predicate::str::contains("group: \"c\"").not())
        .stdout(predicate::str::contains(
            "Check was successful, group: \"d\", name: \"check-d\"",
        ))
        .stdout(predicate::str::contains(
            "Check was successful, group: \"shared\", name: \"check-shared\"",
        ))
        .stdout(predicate::str::contains(
            "Summary: 2 groups succeeded, 1 groups skipped",
        ));

    test_helper.clean_work_dir();
}

#[test]
fn test_skip_only_flag_keeps_dependencies() {
    let test_helper =
        ScopeTestHelper::new("test_skip_only_flag_keeps_dependencies", "skip-with-deps");
    let result = test_helper.doctor_run(Some(&["--skip-only=a"]));

    result
        .success()
        .stdout(predicate::str::contains("Group skipped, group: \"a\""))
        .stdout(predicate::str::contains("group: \"a\", name: \"check-a\"").not())
        .stdout(predicate::str::contains(
            "Check was successful, group: \"b\", name: \"check-b\"",
        ))
        .stdout(predicate::str::contains(
            "Check was successful, group: \"c\", name: \"check-c\"",
        ))
        .stdout(predicate::str::contains(
            "Check was successful, group: \"d\", name: \"check-d\"",
        ))
        .stdout(predicate::str::contains(
            "Check was successful, group: \"shared\", name: \"check-shared\"",
        ))
        .stdout(predicate::str::contains(
            "Summary: 4 groups succeeded, 1 groups skipped",
        ));

    test_helper.clean_work_dir();
}

#[test]
fn test_yolo_flag_auto_approves_fix_prompts() {
    let test_helper = ScopeTestHelper::new(
        "test_yolo_flag_auto_approves_fix_prompts",
        "fix-with-prompt",
    );

    // Without --yolo, the fix would be skipped because user can't confirm in non-interactive mode.
    // With --yolo, the fix should run automatically and succeed.
    let result = test_helper.doctor_run(Some(&["--yolo"]));

    result
        .success()
        .stdout(predicate::str::contains(
            "Check initially failed, fix was successful, group: \"prompt-test\", name: \"needs-approval\"",
        ));

    test_helper.clean_work_dir();
}
