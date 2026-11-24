---
sidebar_position: 3
---

# Scope Intercept

Reporting bugs after the fact is useful, but if the tool can determine that there was an error and prompt the user to report a bug is better. Scope Intercept is an `env` "replacement" that can be used in [shebang](https://en.wikipedia.org/wiki/Shebang_(Unix)) in scripts.

Scope intercept captures stdout and stderr of any command that's run. In order to run the command, and to provide better compatibility `scope intercept` uses `env` under the hood to start the application.

When the script fails (exits with non 0), the user will be informed about KnownErrors, and then be prompted to report the error.

`test.sh` in the examples repo, shows how the intercept may be used. The shebang is setup to use cargo to execute, in a production execution the script would look like

```shell
#!/usr/bin/scope-intercept -- -ddd --extra-config examples bash

>&2 echo "error 1!"
sleep 1
echo "hello world"
exit 1
```

In the case that you expect non-0 exit codes, `--successful-exit` to add additional successful exit codes.

## Help

```text
A wrapper CLI that can be used to capture output from a program, check if there are known errors and let the user know.

`scope-intercept` will execute `/usr/bin/env -S [utility] [args...]` capture the output from STDOUT and STDERR. After the program exits, the exit code will be checked, and if it's non-zero the output will be parsed for known errors.

Usage: scope-intercept [OPTIONS] <UTILITY> [ARGS]...

Arguments:
  <UTILITY>
          Command to execute within scope-intercept

  [ARGS]...
          Arguments to be passed to the utility

Options:
(omit default params)

  -s, --successful-exit <SUCCESSFUL_EXIT>
          Add additional "successful" exit codes. A sub-command that exists 0 will always be considered a success
```
