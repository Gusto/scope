---
sidebar_position: 5
---

# Lint

`scope lint` is used to validate configurations without needing to run large samples.

When run `scope lint` will check:
- `ScopeReportLocation`

To validate `ScopeReportLocation`'s, inputs are generated and templates are rendered. This allows report templates to be validated before they exposed to others.

`ScopeKnownError` and `ScopeDoctorGroup` aren't checked here because they're already validated
at config load time, before any command (including `lint`) runs. An invalid `ScopeKnownError`
will fail the command outright rather than showing up as a lint finding; see
[ScopeKnownError](../models/ScopeKnownError.mdx#validation).

