//! `rtk trust` and `rtk untrust` end to end: a project's `.rtk/filters.toml`
//! rewrites what the agent sees, so it must stay inert until the user trusts
//! that exact content, and stop applying once it changes or trust is revoked.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Replaces the whole output of `rustc --print target-list`, which rtk runs
/// through its fallback, once trusted. The list is long enough that the
/// message and its recall hint are shorter, so rtk emits them rather than the
/// raw output. rustc is on `PATH` wherever the suite runs, and the probe takes
/// no flag rtk itself answers (`--help`, `--version`).
const FILTER: &str = r#"schema_version = 1

[filters.trust-probe]
description = "trust probe"
match_command = "^rustc\\s+--print\\s+target-list$"
match_output = [{ pattern = "x86_64-unknown-linux-gnu", message = "FILTERED-BY-PROJECT" }]
"#;

const MARK: &str = "FILTERED-BY-PROJECT";

/// `rtk rustc --print target-list`: not an rtk command, so rtk runs it.
const PROBE: [&str; 3] = ["rustc", "--print", "target-list"];

/// In every target list, so in the probe's output when no filter applies.
const LISTED: &str = "x86_64-unknown-linux-gnu";

struct Project {
    root: tempfile::TempDir,
}

impl Project {
    fn new() -> Self {
        Self {
            root: common::tempdir(),
        }
    }

    fn dir(&self) -> PathBuf {
        self.root.path().join("project")
    }

    fn filter_path(&self) -> PathBuf {
        self.dir().join(".rtk").join("filters.toml")
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.dir().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn command(&self) -> Command {
        let mut cmd = common::rtk_command_in(self.root.path());
        // No terminal: `rtk trust` must never wait for an answer here.
        cmd.stdin(Stdio::null());
        cmd
    }

    fn rtk(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("run rtk")
    }

    /// What [`PROBE`] prints: the project filter's message when it is applied,
    /// rustc's target list when it is not.
    fn probe(&self) -> Output {
        self.rtk(&PROBE)
    }

    fn filtered(&self) -> bool {
        applied(&self.probe())
    }

    /// [`Project::filtered`], run from `dir` instead of the project.
    fn filtered_from(&self, dir: &Path) -> bool {
        applied(
            &self
                .command()
                .current_dir(dir)
                .args(PROBE)
                .output()
                .unwrap(),
        )
    }

    fn trust_store(&self) -> PathBuf {
        find(self.root.path(), "trusted_filters.json").expect("a trust store was written")
    }
}

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find(&path, name) {
                return Some(found);
            }
        } else if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
    }
    None
}

/// Whether the probe's output is the filter's message. Fails unless the probe ran
/// and printed one or the other.
fn applied(out: &Output) -> bool {
    assert!(out.status.success(), "probe failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let applied = stdout.contains(MARK);
    assert!(
        applied || stdout.contains(LISTED),
        "probe printed neither the target list nor the filter message: {out:?}"
    );
    applied
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn trust_yes(project: &Project) -> Output {
    let out = project.rtk(&["trust", "--yes"]);
    assert!(out.status.success(), "rtk trust --yes: {out:?}");
    out
}

#[test]
fn an_untrusted_filter_is_not_applied_and_trust_off_a_terminal_needs_yes() {
    let project = Project::new();
    project.write(".rtk/filters.toml", FILTER);
    assert!(!project.filtered(), "an untrusted filter was applied");

    let out = project.rtk(&["trust"]);
    assert!(
        !out.status.success(),
        "trust without --yes succeeded: {out:?}"
    );
    let stderr = text(&out.stderr);
    assert!(stderr.contains("none enabled"), "{stderr}");
    assert!(stderr.contains("Not enabled"), "{stderr}");
    assert!(!project.filtered(), "a declined filter was applied");

    let list = project.rtk(&["trust", "--list"]);
    assert!(list.status.success(), "{list:?}");
    assert!(
        text(&list.stdout).contains("No trusted filters."),
        "{list:?}"
    );
}

#[test]
fn trust_yes_enables_the_filter_and_records_the_hash_of_its_content() {
    let project = Project::new();
    project.write(".rtk/filters.toml", FILTER);

    let out = trust_yes(&project);
    let stdout = text(&out.stdout);
    assert!(stdout.contains("Filters will now be applied."), "{stdout}");
    assert!(stdout.contains("Filters: 1"), "{stdout}");
    assert!(
        stdout.contains("[!] Contains 'match_output' rules"),
        "{stdout}"
    );
    let stderr = text(&out.stderr);
    assert!(stderr.contains("trust-probe"), "{stderr}");
    assert!(stderr.contains("Enabled"), "{stderr}");
    assert!(project.filtered(), "a trusted filter was not applied");

    let list = text(&project.rtk(&["trust", "--list"]).stdout);
    let key = std::fs::canonicalize(project.filter_path()).unwrap();
    assert!(list.contains(&key.to_string_lossy().into_owned()), "{list}");
    assert!(
        list.contains(&format!("sha256:{}", sha256_hex(FILTER.as_bytes()))),
        "{list}"
    );

    let again = trust_yes(&project);
    assert!(text(&again.stderr).contains("already trusted"), "{again:?}");
}

#[test]
fn editing_a_trusted_filter_withdraws_its_trust() {
    // Trust is in the exact bytes: an edit that changes only line endings or
    // trailing whitespace counts as an edit.
    let edits = [
        ("a content edit", format!("{FILTER}\n# edited\n")),
        ("CRLF line endings", FILTER.replace('\n', "\r\n")),
        (
            "a trailing space",
            FILTER.replace(
                "description = \"trust probe\"",
                "description = \"trust probe\" ",
            ),
        ),
    ];
    for (edit, content) in edits {
        let project = Project::new();
        project.write(".rtk/filters.toml", FILTER);
        trust_yes(&project);
        assert!(project.filtered());

        project.write(".rtk/filters.toml", &content);
        assert!(!project.filtered(), "{edit} kept the filter's trust");
        let out = project.rtk(&["trust"]);
        assert!(
            !out.status.success(),
            "after {edit} the filter counted as trusted: {out:?}"
        );
    }
}

#[test]
fn untrust_revokes_the_filter_and_reports_when_nothing_is_trusted() {
    let project = Project::new();
    project.write(".rtk/filters.toml", FILTER);
    trust_yes(&project);
    assert!(project.filtered());

    let out = project.rtk(&["untrust"]);
    assert!(out.status.success(), "{out:?}");
    assert!(text(&out.stdout).contains("Trust revoked for"), "{out:?}");
    assert!(!project.filtered(), "a revoked filter was applied");
    let list = text(&project.rtk(&["trust", "--list"]).stdout);
    assert!(list.contains("No trusted filters."), "{list}");

    let again = project.rtk(&["untrust"]);
    assert!(again.status.success(), "{again:?}");
    assert!(
        text(&again.stdout).contains("No trusted filters found to revoke."),
        "{again:?}"
    );
}

#[test]
fn an_unreadable_trust_store_fails_closed() {
    let project = Project::new();
    project.write(".rtk/filters.toml", FILTER);
    trust_yes(&project);
    assert!(project.filtered());

    std::fs::write(project.trust_store(), "not json").unwrap();
    let out = project.probe();
    assert!(!applied(&out), "{out:?}");
    assert!(
        text(&out.stderr).contains("trust store unreadable"),
        "{out:?}"
    );
}

#[test]
fn the_environment_override_applies_only_in_ci() {
    let project = Project::new();
    project.write(".rtk/filters.toml", FILTER);

    let run = |vars: &[(&str, &str)]| {
        project
            .command()
            .envs(vars.iter().copied())
            .args(PROBE)
            .output()
            .unwrap()
    };

    let out = run(&[("RTK_TRUST_PROJECT_FILTERS", "1")]);
    assert!(!applied(&out), "{out:?}");
    assert!(
        text(&out.stderr).contains("CI environment not detected"),
        "{out:?}"
    );

    for ci in [
        "CI",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "JENKINS_URL",
        "BUILDKITE",
    ] {
        let out = run(&[("RTK_TRUST_PROJECT_FILTERS", "1"), (ci, "true")]);
        assert!(applied(&out), "{ci}: {out:?}");
    }

    let out = run(&[("RTK_TRUST_PROJECT_FILTERS", "true"), ("CI", "true")]);
    assert!(!applied(&out), "an override other than 1 applied: {out:?}");
}

#[test]
fn a_trusted_filter_that_is_not_utf8_fails_closed() {
    let project = Project::new();
    let path = project.filter_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, [FILTER.as_bytes(), &b"# \xff\n"[..]].concat()).unwrap();
    trust_yes(&project);

    let out = project.probe();
    assert!(!applied(&out), "{out:?}");
    assert!(text(&out.stderr).contains("is not valid UTF-8"), "{out:?}");
}

#[test]
fn a_filter_in_a_parent_directory_is_not_read() {
    let project = Project::new();
    project.write(".rtk/filters.toml", FILTER);
    trust_yes(&project);
    let sub = project.dir().join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    assert!(project.filtered());
    assert!(!project.filtered_from(&sub), "a parent's filter applied");
    let out = project
        .command()
        .current_dir(&sub)
        .arg("trust")
        .output()
        .unwrap();
    assert!(
        text(&out.stderr).contains("No custom filters or pipeline found"),
        "{out:?}"
    );
}

/// Trust is keyed by the file a link resolves to, so re-pointing a trusted
/// link, or the `.rtk` directory it sits in, needs a new review.
#[cfg(unix)]
#[test]
fn trust_follows_symlinks_to_their_target() {
    use std::os::unix::fs::symlink;

    let project = Project::new();
    let dir = project.dir();
    std::fs::create_dir_all(dir.join(".rtk")).unwrap();
    std::fs::write(dir.join("a.toml"), FILTER).unwrap();
    std::fs::write(dir.join("b.toml"), format!("{FILTER}# b\n")).unwrap();
    symlink(dir.join("a.toml"), project.filter_path()).unwrap();
    trust_yes(&project);
    assert!(project.filtered());

    std::fs::remove_file(project.filter_path()).unwrap();
    symlink(dir.join("b.toml"), project.filter_path()).unwrap();
    assert!(!project.filtered(), "a re-pointed link kept its trust");

    let linked = Project::new();
    let real = linked.root.path().join("real-rtk");
    let other = linked.root.path().join("other-rtk");
    for (target, content) in [
        (&real, FILTER.to_string()),
        (&other, format!("{FILTER}# b\n")),
    ] {
        std::fs::create_dir_all(target).unwrap();
        std::fs::write(target.join("filters.toml"), content).unwrap();
    }
    let rtk_dir = linked.dir().join(".rtk");
    std::fs::create_dir_all(linked.dir()).unwrap();
    symlink(&real, &rtk_dir).unwrap();
    trust_yes(&linked);
    assert!(linked.filtered());

    std::fs::remove_file(&rtk_dir).unwrap();
    symlink(&other, &rtk_dir).unwrap();
    assert!(!linked.filtered(), "a re-pointed .rtk kept its trust");
}

/// The user-global `filters.toml` is gated the same way. Only Linux lets the
/// test move a child's config directory (`XDG_CONFIG_HOME`).
#[cfg(target_os = "linux")]
#[test]
fn a_global_filter_is_gated_like_a_project_one() {
    let project = Project::new();
    let global = project.root.path().join(".config/rtk/filters.toml");
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::write(&global, FILTER).unwrap();
    assert!(!project.filtered(), "an untrusted global filter applied");

    let out = trust_yes(&project);
    assert!(text(&out.stderr).contains("global scoped"), "{out:?}");
    assert!(
        project.filtered(),
        "a trusted global filter was not applied"
    );

    project.rtk(&["untrust"]);
    assert!(!project.filtered(), "a revoked global filter applied");
}

#[test]
fn trust_fails_when_there_is_nothing_valid_to_trust() {
    let project = Project::new();
    let out = project.rtk(&["trust"]);
    assert!(!out.status.success(), "{out:?}");
    assert!(
        text(&out.stderr).contains("No custom filters or pipeline found"),
        "{out:?}"
    );

    project.write(".rtk/filters.toml", "[filters.broken\n");
    let out = project.rtk(&["trust", "--yes"]);
    assert!(!out.status.success(), "{out:?}");
    let stderr = text(&out.stderr);
    assert!(stderr.contains("invalid TOML, not loaded"), "{stderr}");
    assert!(stderr.contains("not valid TOML"), "{stderr}");
    assert!(
        text(&project.rtk(&["trust", "--list"]).stdout).contains("No trusted filters."),
        "an invalid filter was trusted"
    );
}

#[test]
fn the_risk_summary_flags_a_catch_all_pattern_and_passes_a_strip_only_filter() {
    let catch_all = Project::new();
    catch_all.write(
        ".rtk/filters.toml",
        "schema_version = 1\n[filters.all]\ndescription = \"all\"\nmatch_command = \"^x\"\nreplace = [{ pattern = \".\", replacement = \"\" }]\n",
    );
    let stdout = text(&trust_yes(&catch_all).stdout);
    assert!(stdout.contains("[!] Contains 'replace' rules"), "{stdout}");
    assert!(
        stdout.contains("[!] Contains catch-all pattern '.'"),
        "{stdout}"
    );

    let plain = Project::new();
    plain.write(
        ".rtk/filters.toml",
        "schema_version = 1\n[filters.quiet]\ndescription = \"quiet\"\nmatch_command = \"^x\"\nstrip_lines_matching = [\"^noise\"]\n",
    );
    let stdout = text(&trust_yes(&plain).stdout);
    assert!(
        stdout.contains("No high-risk patterns detected."),
        "{stdout}"
    );
}

#[test]
fn a_project_pipeline_is_trusted_and_revoked_with_the_filters() {
    let project = Project::new();
    project.write(
        ".rtk/pipeline.toml",
        "schema_version = 1\n[pipeline]\nname = \"trust-probe\"\n[override.rtk-rewrite]\nenabled = false\n",
    );

    let out = project.rtk(&["trust"]);
    assert!(
        !out.status.success(),
        "an unconfirmed pipeline was enabled: {out:?}"
    );
    assert!(
        text(&out.stderr).contains("Detected a project pipeline"),
        "{out:?}"
    );

    assert_eq!(
        active(&project),
        "rtk:default",
        "an untrusted pipeline loaded"
    );

    let out = trust_yes(&project);
    assert!(text(&out.stderr).contains("Enabled"), "{out:?}");
    assert_eq!(active(&project), "trust-probe");
    let pipeline = std::fs::canonicalize(project.dir().join(".rtk/pipeline.toml")).unwrap();
    let list = text(&project.rtk(&["trust", "--list"]).stdout);
    assert!(
        list.contains(&pipeline.to_string_lossy().into_owned()),
        "{list}"
    );
    assert!(text(&trust_yes(&project).stderr).contains("already trusted"));

    project.write(
        ".rtk/pipelines/extra.toml",
        "schema_version = 1\n[pipeline]\nname = \"extra-probe\"\n",
    );
    assert_eq!(
        active(&project),
        "trust-probe",
        "an untrusted pipeline loaded"
    );
    trust_yes(&project);
    assert_eq!(active(&project), "extra-probe");

    // Trust is per file: editing one leaves the other trusted.
    project.write(
        ".rtk/pipelines/extra.toml",
        "schema_version = 1\n[pipeline]\nname = \"extra-probe\"\n# edited\n",
    );
    assert_eq!(active(&project), "trust-probe", "an edited pipeline loaded");

    let out = project.rtk(&["untrust"]);
    assert!(text(&out.stdout).contains("Trust revoked for"), "{out:?}");
    assert_eq!(active(&project), "rtk:default", "a revoked pipeline loaded");
    assert!(
        text(&project.rtk(&["trust", "--list"]).stdout).contains("No trusted filters."),
        "a pipeline kept its trust"
    );
}

/// The pipeline `rtk pipeline status` reports as active.
fn active(project: &Project) -> String {
    let out = project.rtk(&["pipeline", "status"]);
    assert!(out.status.success(), "rtk pipeline status: {out:?}");
    let stdout = text(&out.stdout);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("active: "))
        .unwrap_or_else(|| panic!("no active line: {out:?}"))
        .to_string()
}

#[test]
fn init_enables_a_project_filter_only_when_told_to() {
    let project = Project::new();
    project.write(".rtk/filters.toml", FILTER);
    let init = |args: &[&str]| {
        let out = project.rtk(&[&["init"][..], args].concat());
        assert!(out.status.success(), "rtk init {args:?}: {out:?}");
        out
    };

    let out = init(&["--dry-run", "--trust-filters"]);
    assert!(
        text(&out.stdout).contains("[dry-run] 1 untrusted custom filter(s)"),
        "{out:?}"
    );
    assert!(!project.filtered(), "a dry run trusted the filter");

    let declining: [&[&str]; 2] = [&["--no-trust-filters"], &[]];
    for args in declining {
        let out = init(args);
        assert!(
            text(&out.stderr).contains("Not enabled"),
            "{args:?}: {out:?}"
        );
        assert!(!project.filtered(), "rtk init {args:?} trusted the filter");
    }

    let out = init(&["--trust-filters"]);
    assert!(
        text(&out.stderr).contains("Enabled. Revoke with"),
        "{out:?}"
    );
    assert!(
        project.filtered(),
        "rtk init --trust-filters left it disabled"
    );

    let out = init(&["--trust-filters"]);
    assert!(
        !text(&out.stderr).contains("Enabled. Revoke with"),
        "a trusted filter was offered again: {out:?}"
    );
}
