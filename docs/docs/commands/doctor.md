---
sidebar_position: 1
---

# Doctor

Doctor is used to fix a local environment. To fix a machine, you'll need to a [ScopeDoctorGroup](../models/ScopeDoctorGroup.mdx). Multiple are supported and recommended.

**Help Text**

```text
Run checks that will "checkup" your machine

Usage: scope doctor [OPTIONS] <COMMAND>

Commands:
  run   Run checks against your machine, generating support output
  list  List all doctor config, giving you the ability to know what is possible
  help  Print this message or the help of the given subcommand(s)
```

## `run`

`scope doctor run` is used to execute all the doctor steps. All checks will be run, if you want to only run specific checks, the `--only` flag with the name of the check to run. This option can be provided multiple times.

If you want to remove a group from the run instead, use `--skip`. Skipping a group also removes any
of its dependencies that aren't also required by another group still in the run — a dependency
shared with a non-skipped group keeps running. This option can be provided multiple times.

If you only want to remove the named group itself, and let its dependencies keep running, use
`--skip-only` instead.

A group can also skip itself via its [`skip` config](../models/ScopeDoctorGroup.mdx#skip)
(`skip: true` or a `skip` command); it's resolved the same way `--skip` is, trimming that group's
dependency subtree.

By default, any provided fix's will be run. If you don't want to run fixes add `--fix=false` to disable fixing issues.

When using a [ScopeDoctorGroup](../models/ScopeDoctorGroup.mdx), the checksum of files are stored on disk. If you need to disable caching, add `--no-cache`.

```text
Run checks against your machine, generating support output

Usage: scope doctor run [OPTIONS]

Options:
  -o, --only <ONLY>                  When set, only the checks listed will run
      --skip <SKIP>                  When set, these groups are removed from the run, along with any dependency that isn't also required by a group that's still included
      --skip-only <SKIP_ONLY>        When set, only the named groups are removed from the run; their dependencies still run
  -f, --fix <FIX>                    When set, if a fix is specified it will also run [default: true] [possible values: true, false]
  -n, --no-cache                     When set cache will be disabled, forcing all file based checks to run
      --yolo                         Automatically approve all fix prompts without asking
(excluded default args)
```

## `list`

Will print out all doctor checks available, in the order `run` will execute.

```text
 INFO Available checks that will run
 INFO   Name                                           Description                                                 Path
 INFO - ScopeDoctorGroup/setup                         You need to run bin/setup                                   .scope/doctor-group-setup.yaml
 INFO - ScopeDoctorGroup/path-exists-fix-in-scope-dir  Check your shell for basic functionality                    .scope/doctor-group-in-scope-dir.yaml
 INFO - ScopeDoctorGroup/path-exists                   Check your shell for basic functionality                    .scope/doctor-group-path-exists.yaml
 INFO - ScopeDoctorGroup/group-1                       Check your shell for basic functionality                    .scope/doctor-group-1.yaml
```