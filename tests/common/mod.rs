//! Shared helpers for the integration tests.
//!
//! Every integration target compiles this module separately and uses only the
//! part it needs, so what one target leaves untouched is dead code there. A
//! target whose only caller sits behind `#[cfg(unix)]` uses none of it on
//! Windows, where `warnings = "deny"` would then fail the build.
#![allow(dead_code)]

/// rtk is a binary crate, so `tests/` cannot import `core::test_isolation`.
/// Its scratch directory is compiled in by path instead.
#[path = "../../src/core/test_isolation/scratch.rs"]
mod scratch;

use std::process::Command;

/// A recall store (0x5254_4B02 is the recall kind) stamped at `version`, set up
/// further by `sql`.
pub fn recall_store(path: &std::path::Path, version: i64, sql: &str) {
    rusqlite::Connection::open(path)
        .expect("open should succeed")
        .execute_batch(&format!(
            "PRAGMA application_id = 0x52544B02;
             PRAGMA user_version = {version};
             {sql}"
        ))
        .expect("the store should be seeded");
}

/// Build a `Command` for the rtk binary, isolated by [`isolate_rtk`]. Use it in
/// place of `Command::new(env!("CARGO_BIN_EXE_rtk"))`.
///
/// A spawned rtk resolves the same data directory a normal invocation would,
/// writing into the contributor's `~/.local/share/rtk/`: rows in their savings
/// history, eviction of the raw output rtk keeps for them under `tee/`, and
/// database corruption when several children write at once.
/// `core::test_isolation` fails the suite on a spawn that skips this.
pub fn rtk_command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    scratch::isolate_rtk(&mut cmd);
    cmd
}

/// [`rtk_command`] with `root` as the child's home and data directory, started
/// in `root/project`: for a test whose child must find a home of its own, such
/// as one without `~/.claude/settings.json`. Keep `root` alive for the test.
pub fn rtk_command_in(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    scratch::isolate_rtk_in(&mut cmd, root);
    cmd
}

/// Isolate a child the way [`rtk_command`] does: an empty environment but for
/// what `scratch::kept` lets through, with rtk's data in this test binary's
/// scratch directory. It clears whatever the command set before and starts it
/// in a project directory of its own, so call it before setting the test's own
/// variables or directory.
pub fn isolate_rtk(cmd: &mut Command) {
    scratch::isolate_rtk(cmd);
}

/// [`isolate_rtk`] with `root` as the child's home and data directory, as
/// [`rtk_command_in`] gives it, for a command that runs a copy of rtk.
pub fn isolate_rtk_in(cmd: &mut Command, root: &std::path::Path) {
    scratch::isolate_rtk_in(cmd, root);
}

/// This test binary's scratch directory, where an isolated child's data goes.
pub fn scratch_dir() -> &'static std::path::Path {
    scratch::scratch_dir()
}

/// Build a `Command` for a native tool whose output a test compares with
/// rtk's, in the environment an rtk child gets, so a setting only the
/// developer's shell exports, such as `RIPGREP_CONFIG_PATH`, reaches neither
/// side rather than one. It starts in the test's directory, not the child's,
/// so give both the same `current_dir` or absolute paths.
pub fn native_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    scratch::isolate_environment(&mut cmd);
    cmd
}

/// Run git in the environment every rtk child from [`rtk_command`] gets, with
/// no configuration of the developer's and its messages in English, where a
/// child's git passes its output through in the test's locale. It clears
/// whatever the command set before, so call it before setting the test's own
/// variables.
pub fn isolate_git(cmd: &mut Command) {
    scratch::isolate_git(cmd);
}

/// [`isolate_git`] with `home` as the home directory, a fixed identity and no
/// prompts, for a test that runs git and rtk in a repository of its own.
pub fn isolate_git_in(cmd: &mut Command, home: &std::path::Path) {
    isolate_git(cmd);
    cmd.env("HOME", home)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_TERMINAL_PROMPT", "0");
}

/// A temporary directory of the test's own under the scratch directory, so
/// concurrent runs never share one. Keep the `TempDir` alive for the test.
pub fn tempdir() -> tempfile::TempDir {
    scratch::tempdir()
}

/// A throwaway git repository with one commit, which works with no user git
/// configuration at all. Keep the `TempDir` alive for the test.
pub fn temp_git_repo() -> tempfile::TempDir {
    scratch::temp_git_repo()
}

/// What `rtk hook claude`, run by `rtk`, prints for a Bash call of `command` in
/// `cwd`, sent as Claude Code sends it. The hook must exit 0 whatever it decides.
pub fn claude_hook(mut rtk: Command, command: &str, cwd: &std::path::Path) -> String {
    use std::io::Write;
    use std::process::Stdio;
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": command },
        "cwd": cwd,
    })
    .to_string();
    let mut child = rtk
        .args(["hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk hook claude");
    // Dropping stdin closes it, so the hook reads the whole payload and ends.
    let mut stdin = child.stdin.take().expect("stdin");
    stdin.write_all(payload.as_bytes()).expect("write payload");
    drop(stdin);
    let out = child.wait_with_output().expect("wait rtk");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rtk hook claude failed for {command:?}: {stderr}"
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}
