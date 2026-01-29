# Simple Analyze Example

This example demonstrates how to use dx-scope's analyze functionality programmatically.

## Running

```bash
cd examples/simple-analyze
cargo run
```

## Features Demonstrated

- Loading scope configuration with known errors
- Analyzing text strings directly
- Analyzing lines from a vector
- Analyzing files
- Using different interaction modes (AutoApprove, DenyAll)

## Key Concepts

### AnalyzeInput

Specify where input comes from:

- `AnalyzeInput::from_lines(vec)` - Analyze in-memory lines
- `AnalyzeInput::from_file(path)` - Analyze a file
- `AnalyzeInput::Stdin` - Analyze from stdin

### AnalyzeOptions

Configure the analysis:

- `known_errors`: Map of known error patterns to detect
- `working_dir`: Directory to run fix commands in

### UserInteraction

Control how fixes are handled:

- `AutoApprove` - Automatically apply all fixes (good for CI)
- `DenyAll` - Never apply fixes (good for dry-run)
- `InquireInteraction` - Interactive prompts (CLI only)

### AnalyzeStatus

The result indicates what happened:

- `NoKnownErrorsFound` - Clean scan
- `KnownErrorFoundNoFixFound` - Error detected but no automatic fix
- `KnownErrorFoundUserDenied` - User declined the fix
- `KnownErrorFoundFixFailed` - Fix was attempted but failed
- `KnownErrorFoundFixSucceeded` - Error detected and fix applied successfully
