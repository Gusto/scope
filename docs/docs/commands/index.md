---
sidebar_position: 2
---

# Commands

`scope` has several built-in commands that most engineers will use:
- [`doctor`](doctor.md) - Run checks that will "checkup" your machine
- [`report`](report.md) - Generate a bug report based on a command
- [`analyze`](analyze/index.md) - Analyze configuration and print validation messages

Beyond the built-in command, scope will also run any binary prefixed with `scope-`.

Additionally, there is `list` that will output all the found configuration.

Here is an example output of `scope list` run from the `examples/` directory.

```text
 INFO Found Resources
 INFO   Name                                           Description                                                 Path
 INFO - ScopeDoctorCheck/path-exists-fix-in-scope-dir  Check your shell for basic functionality                    .scope/doctor-check-fix-in-scope.yaml
 INFO - ScopeDoctorGroup/group-1                       Check your shell for basic functionality                    .scope/doctor-group-1.yaml
 INFO - ScopeDoctorGroup/path-exists                   Check your shell for basic functionality                    .scope/doctor-group-path-exists.yaml
 INFO - ScopeDoctorGroup/path-exists-fix-in-scope-dir  Check your shell for basic functionality                    .scope/doctor-group-in-scope-dir.yaml
 INFO - ScopeDoctorGroup/setup                         You need to run bin/setup                                   .scope/doctor-group-setup.yaml
 INFO - ScopeKnownError/error-exists                   Check if the word error is in the logs                      .scope/known-error.yaml
 INFO - ScopeReportLocation/github                     Description not provided                                    .scope/report.yaml
 INFO - ScopeReportLocation/report                     Description not provided                                    .scope/report.yaml
 INFO
 INFO Commands
 INFO   Name                Description
 INFO - analyze             Analyze logs, output, etc for known errors
 INFO - bar                 External sub-command, run `scope bar` for help
 INFO - doctor              Run checks that will "checkup" your machine
 INFO - foo                 External sub-command, run `scope foo` for help
 INFO - intercept           External sub-command, run `scope intercept` for help
 INFO - list                List the found config files, and resources detected
 INFO - report              Generate a bug report based on a command that was run
 INFO - version             Print version info and exit

```

Under the `Commands` section, notice two additional commands:
- `bar` which is located in `.scope/bin/scope-bar`
- `foo` which is a binary on the `PATH`

Scope automatically adds `.scope/bin` to the path when searching. Allowing teams to add commands to scope in large repos. For example, in a mono-repo with multiple services, you may want to add a `deploy` command. The `deploy` command would come from the working dir.