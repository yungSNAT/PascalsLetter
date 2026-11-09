# CI/CD Flows

This fork runs pull request checks in `ci.yml`, coverage after pushes to
`develop` or manual dispatch, and draft releases in `release.yml`. Upstream's
`cd.yml`, `next-release.yml`, `release.yml` and `stale.yml` workflows are
removed; this `release.yml` is the fork's own.

## PR Quality Gates (ci.yml)

Trigger: pull_request to develop or master

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
static C runtime. The tag is `fork-v` followed by the version `rtk --version`
prints, less its `+pascalsletter` metadata: `fork-v1.1.0` for
`rtk 1.1.0+pascalsletter`. Nothing is published until a maintainer publishes
the draft.
