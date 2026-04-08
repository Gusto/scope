# CI Workflows

```mermaid
flowchart LR
    PR["Pull Request"] -->|"*.rs, Cargo.*"| Tests
    PR -->|"docs/**"| Docs

    Push["Push to main"] --> Release
    Manual["Manual dispatch"] --> Release

    subgraph Tests["Tests Workflow"]
        T1["build, check, test, fmt, clippy<br/><i>ubuntu-latest</i>"]
    end

    subgraph Docs["Docs Workflow"]
        D1["Build docs via workflow_call"]
    end

    subgraph Release["Release Workflow"]
        direction TB
        R1["Prepare Release<br/><i>ubuntu-latest</i>"]
        R2["Push Release Branch + Tag<br/><i>gusto-ubuntu</i>"]
        R3["Build 4 targets in parallel<br/><i>ubuntu-latest + macos-latest</i>"]
        R4["Create GitHub Release<br/><i>gusto-ubuntu</i>"]
        R5["Deploy GitHub Pages<br/><i>ubuntu-latest</i>"]
        R1 --> R2 --> R3 --> R4 --> R5
    end

    subgraph Pages["Build Github Pages"]
        direction TB
        P1["Build Docusaurus site"] --> P2["Deploy to Pages"]
    end

    Docs --> Pages
    R5 --> Pages
```

## Workflow files

| File | Trigger | What it does |
|---|---|---|
| `test.yml` | PR (Rust/Cargo files) | cargo build, check, test, fmt, clippy |
| `docs.yml` | PR (docs/ files) | Calls `gh-page.yml` to verify docs build |
| `release.yml` | Push to main, manual dispatch | CalVer stamp, cross-compile 4 targets, create GitHub release, deploy Pages |
| `gh-page.yml` | `workflow_call` only | Build Docusaurus site and deploy to GitHub Pages |

## Notes

- **Runner split**: Most jobs use GitHub-hosted runners (`ubuntu-latest`, `macos-latest`). Jobs that need write access (git push, release creation) use `gusto-ubuntu-default` due to the Gusto org IP allow list.
- **Version format**: CalVer `YEAR.MONTH.DAY+HHMM` (e.g., `2026.4.7+1823`). The `+HHMM` build metadata is ignored by Cargo for version comparison but ensures unique tags. See the comment in `release.yml` for details.
- **Pages deployment**: Triggered by the Release workflow via `workflow_call`, not by tag patterns.
