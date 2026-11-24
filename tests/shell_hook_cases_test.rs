//! The cases of the shell hook test suites this repository used to ship
//! (hooks/claude/test-rtk-rewrite.sh and hooks/copilot/test-rtk-rewrite.sh),
//! run against the native hooks `rtk init` installs: `rtk hook claude` and
//! `rtk hook copilot`. Each run gets a sandboxed HOME with no permission rules.
//! The scripts' own checks (`jq`, `RTK_AUDIT_DIR`) went with them; the audit
//! log the native hook writes is checked here end to end. Where the native
//! hooks answer differently, the case follows them: `rg` keeps its own
//! command, a command already using rtk gets no rewrite, both sides of `&` are
//! rewritten, Copilot CLI gets `modifiedArgs` in place of a deny, and with no
//! permission rule neither Copilot form carries a permission decision (the
//! host prompts) where the script expected `allow` for VS Code.

use std::io::Write;
use std::process::{Command, Stdio};

mod common;

/// (name, command, the rewrite `rtk hook claude` offers, or None for none).
const CLAUDE_CASES: &[(&str, &str, Option<&str>)] = &[
    (r#"git status"#, r#"git status"#, Some(r#"rtk git status"#)),
    (
        r#"git log --oneline -10"#,
        r#"git log --oneline -10"#,
        Some(r#"rtk git log --oneline -10"#),
    ),
    (
        r#"git diff HEAD"#,
        r#"git diff HEAD"#,
        Some(r#"rtk git diff HEAD"#),
    ),
    (
        r#"git show abc123"#,
        r#"git show abc123"#,
        Some(r#"rtk git show abc123"#),
    ),
    (r#"git add ."#, r#"git add ."#, Some(r#"rtk git add ."#)),
    (r#"gh pr list"#, r#"gh pr list"#, Some(r#"rtk gh pr list"#)),
    (
        r#"npx playwright test"#,
        r#"npx playwright test"#,
        Some(r#"rtk playwright test"#),
    ),
    (r#"ls -la"#, r#"ls -la"#, Some(r#"rtk ls -la"#)),
    (
        r#"curl -s https://example.com"#,
        r#"curl -s https://example.com"#,
        Some(r#"rtk curl -s https://example.com"#),
    ),
    (
        r#"cat package.json"#,
        r#"cat package.json"#,
        Some(r#"rtk read package.json"#),
    ),
    (
        r#"grep -rn pattern src/"#,
        r#"grep -rn pattern src/"#,
        Some(r#"rtk grep -rn pattern src/"#),
    ),
    (
        r#"rg pattern src/"#,
        r#"rg pattern src/"#,
        Some(r#"rtk rg pattern src/"#),
    ),
    (r#"cargo test"#, r#"cargo test"#, Some(r#"rtk cargo test"#)),
    (
        r#"npx prisma migrate"#,
        r#"npx prisma migrate"#,
        Some(r#"rtk prisma migrate"#),
    ),
    (r#"rtk git status (already rtk)"#, r#"rtk git status"#, None),
    (
        r#"env + playwright"#,
        r#"TEST_SESSION_ID=2 npx playwright test --config=foo"#,
        Some(r#"TEST_SESSION_ID=2 rtk playwright test --config=foo"#),
    ),
    (
        r#"env + git status"#,
        r#"GIT_PAGER=cat git status"#,
        Some(r#"GIT_PAGER=cat rtk git status"#),
    ),
    (
        r#"env + git log"#,
        r#"GIT_PAGER=cat git log --oneline -10"#,
        Some(r#"GIT_PAGER=cat rtk git log --oneline -10"#),
    ),
    (
        r#"multi env + vitest"#,
        r#"NODE_ENV=test CI=1 npx vitest"#,
        Some(r#"NODE_ENV=test CI=1 rtk vitest"#),
    ),
    (
        r#"env + ls"#,
        r#"LANG=C ls -la"#,
        Some(r#"LANG=C rtk ls -la"#),
    ),
    (
        r#"env + npm run"#,
        r#"NODE_ENV=test npm run test:e2e"#,
        Some(r#"NODE_ENV=test rtk npm run test:e2e"#),
    ),
    (
        r#"env + docker compose (unsupported subcommand, NOT rewritten)"#,
        r#"COMPOSE_PROJECT_NAME=test docker compose up -d"#,
        None,
    ),
    (
        r#"env + docker compose logs (supported, rewritten)"#,
        r#"COMPOSE_PROJECT_NAME=test docker compose logs web"#,
        Some(r#"COMPOSE_PROJECT_NAME=test rtk docker compose logs web"#),
    ),
    (
        r#"npm run test:e2e"#,
        r#"npm run test:e2e"#,
        Some(r#"rtk npm run test:e2e"#),
    ),
    (
        r#"npm run build"#,
        r#"npm run build"#,
        Some(r#"rtk npm run build"#),
    ),
    (r#"npm jest run"#, r#"npm jest run"#, Some(r#"rtk jest"#)),
    (
        r#"docker compose up -d (NOT rewritten — unsupported by rtk)"#,
        r#"docker compose up -d"#,
        None,
    ),
    (
        r#"docker compose logs postgrest"#,
        r#"docker compose logs postgrest"#,
        Some(r#"rtk docker compose logs postgrest"#),
    ),
    (
        r#"docker compose ps"#,
        r#"docker compose ps"#,
        Some(r#"rtk docker compose ps"#),
    ),
    (
        r#"docker compose build"#,
        r#"docker compose build"#,
        Some(r#"rtk docker compose build"#),
    ),
    (
        r#"docker compose down (NOT rewritten — unsupported by rtk)"#,
        r#"docker compose down"#,
        None,
    ),
    (
        r#"docker compose -f file.yml up (NOT rewritten — flag before subcommand)"#,
        r#"docker compose -f docker-compose.preview.yml --project-name myapp up -d --build"#,
        None,
    ),
    (
        r#"docker run --rm postgres"#,
        r#"docker run --rm postgres"#,
        Some(r#"rtk docker run --rm postgres"#),
    ),
    (
        r#"docker exec -it db psql"#,
        r#"docker exec -it db psql"#,
        Some(r#"rtk docker exec -it db psql"#),
    ),
    (
        r#"find . -name '*.ts'"#,
        r#"find . -name '*.ts'"#,
        Some(r#"rtk find . -name '*.ts'"#),
    ),
    (r#"tree src/"#, r#"tree src/"#, Some(r#"rtk tree src/"#)),
    (
        r#"wget https://example.com/file"#,
        r#"wget https://example.com/file"#,
        Some(r#"rtk wget https://example.com/file"#),
    ),
    (
        r#"gh api repos/owner/repo"#,
        r#"gh api repos/owner/repo"#,
        Some(r#"rtk gh api repos/owner/repo"#),
    ),
    (
        r#"gh release list"#,
        r#"gh release list"#,
        Some(r#"rtk gh release list"#),
    ),
    (
        r#"kubectl describe pod foo"#,
        r#"kubectl describe pod foo"#,
        Some(r#"rtk kubectl describe pod foo"#),
    ),
    (
        r#"kubectl apply -f deploy.yaml"#,
        r#"kubectl apply -f deploy.yaml"#,
        Some(r#"rtk kubectl apply -f deploy.yaml"#),
    ),
    (
        r#"RTK_DISABLED=1 git status (no rewrite)"#,
        r#"RTK_DISABLED=1 git status"#,
        None,
    ),
    (
        r#"RTK_DISABLED=1 cargo test (no rewrite)"#,
        r#"RTK_DISABLED=1 cargo test"#,
        None,
    ),
    (
        r#"FOO=1 RTK_DISABLED=1 git status (no rewrite)"#,
        r#"FOO=1 RTK_DISABLED=1 git status"#,
        None,
    ),
    (
        r#"cargo test 2>&1 | head"#,
        r#"cargo test 2>&1 | head"#,
        Some(r#"rtk cargo test 2>&1 | head"#),
    ),
    (
        r#"cargo test 2>&1"#,
        r#"cargo test 2>&1"#,
        Some(r#"rtk cargo test 2>&1"#),
    ),
    (
        r#"cargo test &>/dev/null"#,
        r#"cargo test &>/dev/null"#,
        Some(r#"rtk cargo test &>/dev/null"#),
    ),
    (
        r#"cargo test & git status"#,
        r#"cargo test & git status"#,
        Some(r#"rtk cargo test & rtk git status"#),
    ),
    (r#"vitest (no args)"#, r#"vitest"#, Some(r#"rtk vitest"#)),
    (
        r#"vitest run (no run)"#,
        r#"vitest run"#,
        Some(r#"rtk vitest"#),
    ),
    (
        r#"vitest --reporter"#,
        r#"vitest --reporter=verbose"#,
        Some(r#"rtk vitest --reporter=verbose"#),
    ),
    (r#"npx vitest"#, r#"npx vitest"#, Some(r#"rtk vitest"#)),
    (
        r#"pnpm vitest --coverage"#,
        r#"pnpm vitest --coverage"#,
        Some(r#"rtk vitest --coverage"#),
    ),
    (
        r#"heredoc"#,
        r#"cat <<'EOF'
hello
EOF"#,
        None,
    ),
    (r#"echo (no pattern)"#, r#"echo hello world"#, None),
    (r#"cd (no pattern)"#, r#"cd /tmp"#, None),
    (r#"mkdir (no pattern)"#, r#"mkdir -p foo/bar"#, None),
    (r#"python3 (no pattern)"#, r#"python3 script.py"#, None),
    (r#"node (no pattern)"#, r#"node -e 'console.log(1)'"#, None),
];

/// Runs `rtk hook <agent>` on `payload` in a sandbox under `root`; the hook
/// must exit 0 whatever it decides. Returns stdout parsed, or None if empty.
fn hook(mut rtk: Command, agent: &str, payload: &serde_json::Value) -> Option<serde_json::Value> {
    let mut child = rtk
        .args(["hook", agent])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk hook");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin
        .write_all(payload.to_string().as_bytes())
        .expect("write payload");
    drop(stdin);
    let out = child.wait_with_output().expect("wait rtk");
    assert!(
        out.status.success(),
        "rtk hook {agent} failed for {payload}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    (!stdout.trim().is_empty())
        .then(|| serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}")))
}

fn str_at<'a>(v: &'a serde_json::Value, pointer: &str) -> Option<&'a str> {
    v.pointer(pointer).and_then(serde_json::Value::as_str)
}

fn bash(command: &str) -> serde_json::Value {
    serde_json::json!({ "tool_name": "Bash", "tool_input": { "command": command } })
}

fn copilot_cli(command: &str) -> serde_json::Value {
    let args = serde_json::json!({ "command": command }).to_string();
    serde_json::json!({ "toolName": "bash", "toolArgs": args })
}

#[test]
fn claude_hook_rewrites_the_shell_suites_cases() {
    let root = common::tempdir();
    let mut failures = Vec::new();
    for (name, command, want) in CLAUDE_CASES {
        let out = hook(
            common::rtk_command_in(root.path()),
            "claude",
            &bash(command),
        );
        let got = out
            .as_ref()
            .and_then(|v| str_at(v, "/hookSpecificOutput/updatedInput/command"));
        // No rewrite means no output at all, not JSON without a command.
        if got != *want || (want.is_none() && out.is_some()) {
            failures.push(format!("{name}: {command:?} gave {got:?}, want {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn copilot_cli_gets_the_rewritten_command() {
    let root = common::tempdir();
    for (command, want) in [
        ("git status", "rtk git status"),
        ("git log --oneline -10", "rtk git log"),
        ("git diff HEAD", "rtk git diff"),
        ("cargo test", "rtk cargo test"),
        ("cargo clippy --all-targets", "rtk cargo clippy"),
        ("cargo build", "rtk cargo build"),
        ("grep -rn pattern src/", "rtk grep"),
        ("gh pr list", "rtk gh"),
    ] {
        let out = hook(
            common::rtk_command_in(root.path()),
            "copilot",
            &copilot_cli(command),
        )
        .unwrap_or_else(|| panic!("{command}: no output"));
        let modified = str_at(&out, "/modifiedArgs/command").unwrap_or_default();
        assert!(modified.contains(want), "{command}: {out}");
        assert_eq!(out.pointer("/permissionDecision"), None, "{command}: {out}");
    }
}

#[test]
fn copilot_vscode_gets_the_rewritten_command() {
    let root = common::tempdir();
    for (command, want) in [
        ("git status", "rtk git status"),
        ("cargo test", "rtk cargo test"),
        ("gh pr list", "rtk gh"),
    ] {
        let out = hook(
            common::rtk_command_in(root.path()),
            "copilot",
            &bash(command),
        )
        .unwrap_or_else(|| panic!("{command}: no output"));
        let updated = str_at(&out, "/hookSpecificOutput/updatedInput/command").unwrap_or_default();
        assert!(updated.contains(want), "{command}: {out}");
        assert_eq!(
            out.pointer("/hookSpecificOutput/permissionDecision"),
            None,
            "{command}: {out}"
        );
    }
}

#[test]
fn copilot_passes_other_calls_through_silently() {
    let root = common::tempdir();
    for payload in [
        copilot_cli("rtk git status"),
        copilot_cli("rtk cargo test"),
        copilot_cli("cat <<'EOF'\nhello\nEOF"),
        copilot_cli("htop"),
        copilot_cli("echo hello world"),
        serde_json::json!({ "toolName": "view", "toolArgs": "{}" }),
        serde_json::json!({ "toolName": "edit", "toolArgs": "{}" }),
        bash("rtk git status"),
        serde_json::json!({ "tool_name": "editFiles" }),
    ] {
        let out = hook(common::rtk_command_in(root.path()), "copilot", &payload);
        assert!(out.is_none(), "{payload}: {out:?}");
    }
}

/// Every `hook-audit.log` under `dir`.
fn audit_logs(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(audit_logs(&path));
        } else if path.file_name().is_some_and(|n| n == "hook-audit.log") {
            found.push(path);
        }
    }
    found
}

/// With RTK_HOOK_AUDIT=1 a rewrite is logged as 4 fields, the second
/// `rewrite`; without it nothing is written.
#[test]
fn claude_hook_audit_log_records_a_rewrite_only_when_asked() {
    let quiet = common::tempdir();
    hook(
        common::rtk_command_in(quiet.path()),
        "claude",
        &bash("git status"),
    );
    assert!(audit_logs(quiet.path()).is_empty());

    let root = common::tempdir();
    let mut rtk = common::rtk_command_in(root.path());
    rtk.env("RTK_HOOK_AUDIT", "1");
    hook(rtk, "claude", &bash("git status")).expect("a rewrite");
    let logs = audit_logs(root.path());
    assert_eq!(logs.len(), 1, "{logs:?}");
    let log = std::fs::read_to_string(&logs[0]).expect("read the audit log");
    let fields: Vec<&str> = log.trim_end().split(" | ").collect();
    assert_eq!(fields.len(), 4, "{log}");
    assert_eq!(fields[1], "rewrite", "{log}");
    assert_eq!(fields[2], "git status", "{log}");
    assert_eq!(fields[3], "rtk git status", "{log}");
}
