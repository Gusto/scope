# Simple Doctor Example

This example demonstrates how to use dx-scope's doctor functionality programmatically.

## Running

```bash
cd examples/simple-doctor
cargo run
```

## Features Demonstrated

- Loading scope configuration
- Running health checks in CI mode (without fixes)
- Running specific groups with auto-fix enabled
- Listing available checks

## Key Concepts

### DoctorRunOptions

Configure how doctor checks run:

- `DoctorRunOptions::ci_mode()` - Run checks without applying fixes
- `DoctorRunOptions::with_fixes()` - Run checks and auto-apply fixes
- `DoctorRunOptions::for_groups(vec)` - Run only specific groups

### PathRunResult

The result contains:

- `did_succeed`: Overall success/failure
- `succeeded_groups`: Set of group names that passed
- `failed_group`: Set of group names that failed
- `skipped_group`: Set of group names that were skipped
- `group_reports`: Detailed reports for each group
