# CI/CD Flows

This fork runs pull request checks in `ci.yml`, which also runs on each push to
`develop` and saves the Rust caches pull requests restore, coverage after pushes
to `develop` or manual dispatch, and draft releases in `release.yml`. Upstream's
`cd.yml`, `next-release.yml`, `release.yml` and `stale.yml` workflows are
removed; this `release.yml` is the fork's own.

## PR Quality Gates (ci.yml)

Trigger: pull_request to develop or master | push to develop (without test
presence, semgrep and the security scan's diff steps, which review a pull
request's changes)

```
                          ┌──────────────────┐
                          │    PR opened      │
                          └────────┬─────────┘
                                   │
                          ┌────────▼─────────┐
                          │    fmt --all     │
                          └────────┬─────────┘
                                   │
                       ┌───────────▼──────────┐
                       │ clippy --all-targets │
                       └─────┬───┬───┬───┬────┘
                             │   │   │   │
           ┌─────────────────┘   │   │   └────────┐
           │            ┌────────┘   │            │
           ▼            ▼            ▼            ▼
     ┌──────────┐ ┌──────────┐ ┌───────────┐ ┌─────────┐
     │ test     │ │ security │ │ semgrep   │ │benchmark│
     │ ubuntu   │ │ cargo    │ │ AST-aware │ │ no empty│
     │ windows  │ │ audit    │ │ diff-only │ │ or grown│
     │ macos    │ │ patterns │ │           │ │ output  │
     └────┬─────┘ └────┬─────┘ └─────┬─────┘ └────┬────┘
          │            │             │            │
          └────────────┴─────────┬───┴────────────┘
                                 │
                      ┌──────────▼─────────┐
                      │  All must pass     │
                      │  to merge          │
                      └────────────────────┘

     + release (build, after clippy: must also pass to merge)
     + DCO check (independent, develop PRs only)
     + Dependabot (weekly: Cargo deps + GitHub Actions)
     + coverage.yml (push to develop or manual dispatch only: cargo-llvm-cov, then Codecov)
```

## Draft release (release.yml)

Trigger: push of a `fork-v*` tag | pull_request to develop that changes
release.yml (a dry run: build and checksums only)

```
     ┌──────────────────────────┐
     │ build, one per target    │
     │ --no-default-features    │
     │ run --version, package   │
     └────────────┬─────────────┘
                  │
     ┌────────────▼─────────────┐
     │ checksums                │
     │ exactly 5 archives       │
     │ checksums.txt            │
     └────────────┬─────────────┘
                  │ tags only
     ┌────────────▼─────────────┐
     │ draft                    │
     │ tag unmoved, on develop  │
     │ no release for it yet    │
     │ provenance attestation   │
     │ DRAFT GitHub release     │
     └──────────────────────────┘
```

Only the Apache-2.0 edition (`rtk-oss-<target>`) is built, for x86_64 and
aarch64 Linux (musl), x86_64 and aarch64 macOS, and x86_64 Windows with a
static C runtime. The version is upstream's with `+pascalsletter` build
metadata for the first release on that upstream version, and
`+pascalsletter.N` for each later one, counting from 1. The tag is `fork-v`
followed by the upstream version, then `-N` if there is one: `fork-v1.2.0` for
`rtk 1.2.0+pascalsletter` and `fork-v1.2.0-1` for `rtk 1.2.0+pascalsletter.1`.
The build job prints the tag it expects. A merge that brings in a new upstream
version sets the metadata back to `+pascalsletter`. The draft's notes list the
pull requests merged since the latest published release. Nothing is published
until a maintainer publishes the draft.
