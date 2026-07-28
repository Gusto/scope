#[allow(dead_code)]
mod common;

use assert_fs::fixture::{FileWriteStr, PathChild};
use common::*;
use predicates::prelude::predicate;

#[test]
fn test_will_find_child_configs() {
    let helper = ScopeTestHelper::new("test_will_find_child_configs", "nested-config");

    let results = helper.run_command(&["list"]);
    results
        .success()
        .stdout(predicate::str::contains("ScopeKnownError/disk-full"))
        .stdout(predicate::str::contains("The disk is full of files"))
        .stdout(predicate::str::contains(".scope/shared/disk-full.yaml"));

    helper.clean_work_dir();
}

#[test]
fn test_will_list_sub_command() {
    let test_helper = ScopeTestHelper::new("test_will_list_sub_command", "command-paths");
    let result = test_helper.run_command(&["list"]);

    result
        .success()
        .stdout(predicate::str::contains("external"))
        .stdout(predicate::str::contains(
            "External sub-command, run `scope external` for help",
        ));

    test_helper.clean_work_dir();
}

#[test]
fn test_external_sub_command_works() {
    let test_helper = ScopeTestHelper::new("test_sub_command_works", "command-paths");
    let result = test_helper.run_command(&["external"]);

    result
        .success()
        .stdout(predicate::str::contains("in external"));
    test_helper.clean_work_dir();
}

#[test]
fn test_extra_field_will_show_warn() {
    let test_helper = ScopeTestHelper::new("test_extra_field_will_show_warn", "empty");
    let example_file = "apiVersion: scope.github.com/v1alpha
kind: ScopeDoctorGroup
metadata:
  name: fail-then-fix
  description: Run dep install
spec:
  extra: string
  actions: []
";
    test_helper
        .work_dir
        .child(".scope/bad-format.yml")
        .write_str(example_file)
        .unwrap();

    let result = test_helper.run_command(&["list"]);

    result.success().stdout(predicate::str::contains(
        "Resource 'ScopeDoctorGroup/fail-then-fix' didn't match the schema for ScopeDoctorGroup",
    ));
    test_helper.clean_work_dir();
}

#[test]
fn test_known_error_with_reserved_capture_name_fails_load() {
    let test_helper = ScopeTestHelper::new(
        "test_known_error_with_reserved_capture_name_fails_load",
        "empty",
    );
    let example_file = "apiVersion: scope.github.com/v1alpha
kind: ScopeKnownError
metadata:
  name: bad-reserved
  description: uses a reserved capture group name
spec:
  pattern: \"boom: (?<captures>.*)\"
  help: \"should never load\"
";
    test_helper
        .work_dir
        .child(".scope/bad-reserved.yaml")
        .write_str(example_file)
        .unwrap();

    let result = test_helper.run_command(&["list"]);

    result
        .failure()
        .stdout(predicate::str::contains("bad-reserved.yaml"))
        .stdout(predicate::str::contains("reserved"));
    test_helper.clean_work_dir();
}

#[test]
fn test_known_error_with_broken_fix_template_fails_load() {
    let test_helper = ScopeTestHelper::new(
        "test_known_error_with_broken_fix_template_fails_load",
        "empty",
    );
    let example_file = "apiVersion: scope.github.com/v1alpha
kind: ScopeKnownError
metadata:
  name: bad-fix-template
  description: has a broken fix template
spec:
  pattern: \"boom: (.*)\"
  help: \"should never load\"
  fix:
    commands:
      - \"echo {{ unclosed\"
";
    test_helper
        .work_dir
        .child(".scope/bad-fix-template.yaml")
        .write_str(example_file)
        .unwrap();

    let result = test_helper.run_command(&["list"]);

    result
        .failure()
        .stdout(predicate::str::contains("bad-fix-template.yaml"));
    test_helper.clean_work_dir();
}

#[test]
fn test_multiple_known_error_failures_are_all_reported() {
    let test_helper = ScopeTestHelper::new(
        "test_multiple_known_error_failures_are_all_reported",
        "empty",
    );
    let bad_reserved = "apiVersion: scope.github.com/v1alpha
kind: ScopeKnownError
metadata:
  name: bad-reserved
  description: uses a reserved capture group name
spec:
  pattern: \"boom: (?<captures>.*)\"
  help: \"should never load\"
";
    let bad_fix_template = "apiVersion: scope.github.com/v1alpha
kind: ScopeKnownError
metadata:
  name: bad-fix-template
  description: has a broken fix template
spec:
  pattern: \"boom: (.*)\"
  help: \"should never load\"
  fix:
    commands:
      - \"echo {{ unclosed\"
";
    test_helper
        .work_dir
        .child(".scope/bad-reserved.yaml")
        .write_str(bad_reserved)
        .unwrap();
    test_helper
        .work_dir
        .child(".scope/bad-fix-template.yaml")
        .write_str(bad_fix_template)
        .unwrap();

    let result = test_helper.run_command(&["list"]);

    result
        .failure()
        .stdout(predicate::str::contains("bad-reserved.yaml"))
        .stdout(predicate::str::contains("bad-fix-template.yaml"));
    test_helper.clean_work_dir();
}

#[test]
fn test_doctor_group_broken_template_warns_and_continues() {
    let test_helper = ScopeTestHelper::new(
        "test_doctor_group_broken_template_warns_and_continues",
        "empty",
    );
    let example_file = "apiVersion: scope.github.com/v1alpha
kind: ScopeDoctorGroup
metadata:
  name: broken-template-group
  description: has a broken check command template
spec:
  actions:
    - name: broken-check
      description: check with a broken template
      check:
        commands:
          - \"echo {{ unclosed\"
";
    test_helper
        .work_dir
        .child(".scope/broken-template-group.yaml")
        .write_str(example_file)
        .unwrap();

    let result = test_helper.run_command(&["list"]);

    result
        .success()
        .stdout(predicate::str::contains("broken-template-group.yaml"));
    test_helper.clean_work_dir();
}
