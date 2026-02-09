# Publishing scope to crates.io - Checklist

This document outlines the steps and TODOs required to publish `dx-scope` as a crate on crates.io.

## Current State

The crate is already configured for publishing with:
- `publish = true` in Cargo.toml
- License: BSD-3-Clause
- Repository, homepage, and description set
- README.md present

## Crate Naming

The crate is named `dx-scope`. Important considerations:

1. **Check name availability**: The name `dx-scope` may already be taken on crates.io. Check at https://crates.io/crates/dx-scope

2. **Alternative names** if `dx-scope` is taken:
   - `scope-cli`
   - `scope-dev`
   - `oscope` (from oscope-dev)
   - `gusto-scope`

3. **Scoped packages**: Unlike npm, Rust/crates.io does not support scoped packages like `@gusto/scope`. The crate name must be a single identifier.

4. **To change the name**, update in `Cargo.toml`:
   ```toml
   [package]
   name = "new-name-here"

   [lib]
   name = "new_name_here"  # Use underscores for the lib name
   ```

---

## Critical Issues (Must Fix Before Publishing)

### 1. Wildcard Dependencies

**Problem:** crates.io does not allow wildcard (`*`) version specifications.

**Affected dependencies:**
```toml
indicatif = "*"
jsonwebtoken = "*"
octocrab = "*"
opentelemetry = { version = "*", ... }
opentelemetry_sdk = { version = "*", ... }
opentelemetry-otlp = { version = "*", ... }
schemars = "*"
tonic = "*"
tracing-indicatif = "*"
tracing-opentelemetry = "*"
tracing-subscriber = { version = "*", ... }
```

**Fix:** Pin each dependency to a specific version or version range:
```toml
indicatif = "0.17"
jsonwebtoken = "9"
octocrab = "0.32"
opentelemetry = { version = "0.21", ... }
# etc.
```

**Command to check current versions:**
```bash
cargo tree -d  # Show duplicates
cargo update   # Update Cargo.lock
cargo outdated # If cargo-outdated is installed
```

### 2. Edition Year

**Problem:** `edition = "2024"` is not yet stable. The current stable edition is 2021.

**Fix:** Change to `edition = "2021"` in Cargo.toml, or wait until Rust 2024 edition is stabilized.

### 3. Yanked Dependencies

**Problem:** `os_info v3.8.0` in Cargo.lock is yanked.

**Fix:** Update dependencies:
```bash
cargo update
```

### 4. build.rs Git Dependency

**Problem:** The build script uses `vergen` to embed git information, which fails when building from a crates.io download (no `.git` directory).

**Current build.rs:**
```rust
EmitBuilder::builder().all_build().all_git().emit()?;
```

**Fix:** Make git info optional with graceful fallback:
```rust
use anyhow::Result;
use vergen::EmitBuilder;

pub fn main() -> Result<()> {
    // Build info always works
    let mut builder = EmitBuilder::builder();
    builder.all_build();

    // Git info only available when building from git repo
    if std::path::Path::new(".git").exists() {
        builder.all_git();
    }

    builder.emit()?;
    Ok(())
}
```

Or use `vergen`'s built-in fallback features.

---

## Recommended Improvements

### 5. Add Minimum Supported Rust Version (MSRV)

Add to Cargo.toml:
```toml
[package]
rust-version = "1.75"  # Or appropriate version
```

This helps users know if their Rust version is compatible.

### 6. Add Feature Flags for Library vs CLI

Separate CLI dependencies from library core to reduce compile times and dependency count for library users.

**Proposed feature structure:**
```toml
[features]
default = ["cli"]
cli = ["inquire", "clap", "human-panic", "tracing-indicatif"]

[dependencies]
# Core library dependencies (always included)
anyhow = "1.0"
async-trait = "0.1"
tokio = { version = "1", features = ["full"] }
# ...

# CLI-only dependencies
inquire = { version = "0.6", features = ["editor"], optional = true }
clap = { version = "4.5", features = ["derive", "env"], optional = true }
human-panic = { version = "2.0", optional = true }
tracing-indicatif = { version = "0.3", optional = true }
```

**Update cli/mod.rs:**
```rust
#[cfg(feature = "cli")]
mod inquire_interaction;

#[cfg(feature = "cli")]
pub use inquire_interaction::InquireInteraction;
```

### 7. Update README for Library Usage

Add a section to README.md about library usage:

```markdown
## Library Usage

`dx-scope` can be used as a library in your Rust projects:

```toml
[dependencies]
scope = { version = "2026.1", default-features = false }
```

See [Library Usage Guide](docs/library-usage.md) for detailed documentation.
```

### 8. Update Package Metadata

Enhance categories and keywords for better discoverability:

```toml
[package]
keywords = ["developer-tools", "diagnostics", "health-check", "error-detection", "devex"]
categories = [
    "command-line-utilities",
    "development-tools::debugging",
    "development-tools::build-utils",
    "development-tools::testing",  # Add if applicable
]
```

### 9. Add rustdoc Documentation Link

Update documentation to point to docs.rs (automatic for crates.io):

```toml
[package]
documentation = "https://docs.rs/dx-scope"
```

Or keep both:
```toml
# docs.rs will automatically be used for API docs
# Keep homepage for user guides
homepage = "https://oscope-dev.github.io/scope/"
```

### 10. Add Package Metadata for docs.rs

Configure docs.rs build:

```toml
[package.metadata.docs.rs]
all-features = true
rustdoc-args = ["--cfg", "docsrs"]
```

---

## Pre-Publication Checklist

### Code Quality
- [ ] Run `cargo fmt --check` - code is formatted
- [ ] Run `cargo clippy -- -D warnings` - no clippy warnings
- [ ] Run `cargo test` - all tests pass
- [ ] Run `cargo doc --no-deps` - documentation builds

### Cargo.toml
- [ ] Remove all wildcard (`*`) version specifications
- [ ] Set correct edition (2021 or wait for 2024 stable)
- [ ] Add `rust-version` (MSRV)
- [ ] Verify `license` matches LICENSE file
- [ ] Verify `repository` URL is correct
- [ ] Verify `description` is accurate
- [ ] Update `keywords` and `categories`

### Dependencies
- [ ] Run `cargo update` to update Cargo.lock
- [ ] Verify no yanked dependencies: `cargo publish --dry-run`
- [ ] Consider adding feature flags for optional dependencies

### Documentation
- [ ] README.md includes library usage section
- [ ] API documentation is complete (`cargo doc`)
- [ ] Examples compile and run
- [ ] CHANGELOG.md is updated (if maintained)

### Build
- [ ] Fix build.rs to handle missing `.git` directory
- [ ] Verify `cargo publish --dry-run` succeeds

### Legal
- [ ] LICENSE file is present and matches Cargo.toml
- [ ] All dependencies have compatible licenses
- [ ] No proprietary code included

---

## Publishing Steps

Once all checklist items are complete:

### 1. Login to crates.io
```bash
cargo login <your-api-token>
```

Get your API token from https://crates.io/me

### 2. Verify Package
```bash
cargo publish --dry-run
```

### 3. Publish
```bash
cargo publish
```

### 4. Verify Publication
- Check https://crates.io/crates/dx-scope (Note: this name may be taken - see naming section below)
- Check https://docs.rs/dx-scope

### 5. Tag Release
```bash
git tag v2026.1.12
git push origin v2026.1.12
```

---

## Post-Publication

### Monitor
- Watch for issues reported on GitHub
- Monitor docs.rs build status
- Check for security advisories on dependencies

### Maintenance
- Keep dependencies updated
- Respond to breaking changes in dependencies
- Maintain backward compatibility or bump major version

---

## Quick Reference: Common Issues

| Issue | Solution |
|-------|----------|
| "wildcard dependency" | Pin to specific version |
| "failed to verify package" | Fix build.rs or check permissions |
| "yanked dependency" | Run `cargo update` |
| "missing LICENSE" | Ensure LICENSE file exists |
| "edition not found" | Use stable edition (2021) |
| "git info failed" | Make git info optional in build.rs |

---

## Version History

| Date | Version | Changes |
|------|---------|---------|
| 2026-01-28 | 2026.1.12 | Library-first refactoring complete |
