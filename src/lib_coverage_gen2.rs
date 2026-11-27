use crate::core::test_isolation;
use std::process::{Output, Stdio};

/// Runs the rtk binary in a fresh scratch working directory with no stdin.
fn rtk_output(args: &[&str]) -> Output {
    let dir = test_isolation::tempdir();
    test_isolation::rtk_command()
        .args(args)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .output()
        .expect("spawn rtk")
}

#[test]
fn run_without_command_bypass_without_args_and_hook_audit() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }

    // `rtk run` with nothing to run succeeds without output.
    let out = rtk_output(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");

    // A blank -c script is the same as nothing to run.
    let out = rtk_output(&["run", "-c", "   "]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");

    // `rtk bypass` with no command is an error reported by rtk itself.
    let out = rtk_output(&["bypass"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bypass requires a command to execute"),
        "stderr: {stderr}"
    );

    // hook-audit reads the (empty) audit log in the scratch home and succeeds.
    let out = rtk_output(&["hook-audit", "--since", "0"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[cfg(unix)]
struct FakeEnv {
    bin: tempfile::TempDir,
    work: tempfile::TempDir,
    log: std::path::PathBuf,
    path: std::ffi::OsString,
}

#[cfg(unix)]
fn write_script(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).expect("write fake tool");
    let mut perms = std::fs::metadata(path)
        .expect("stat fake tool")
        .permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(path, perms).expect("chmod fake tool");
}

#[cfg(unix)]
fn fake_env(tools: &[&str]) -> FakeEnv {
    let bin = test_isolation::tempdir();
    let work = test_isolation::tempdir();
    let log = bin.path().join("calls.log");
    let script = format!(
        "#!/bin/sh\nprintf '%s %s\\n' \"${{0##*/}}\" \"$*\" >> '{}'\necho fake-tool-output\nexit 0\n",
        log.display()
    );
    for tool in tools {
        write_script(&bin.path().join(tool), &script);
    }
    // Some wrappers look for a project-local launcher or test script.
    write_script(&work.path().join("gradlew"), &script);
    std::fs::write(work.path().join("run-tests.php"), "<?php\n").expect("write run-tests.php");

    let inherited = test_isolation::rtk_command()
        .get_envs()
        .find(|(k, _)| *k == "PATH")
        .and_then(|(_, v)| v.map(|v| v.to_os_string()))
        .unwrap_or_default();
    let mut path = bin.path().as_os_str().to_os_string();
    if !inherited.is_empty() {
        path.push(":");
        path.push(&inherited);
    }
    FakeEnv {
        bin,
        work,
        log,
        path,
    }
}

#[cfg(unix)]
impl FakeEnv {
    fn run(&self, args: &[&str]) -> (Output, String) {
        std::fs::write(&self.log, "").expect("reset log");
        let out = test_isolation::rtk_command()
            .env("PATH", &self.path)
            .current_dir(self.work.path())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("spawn rtk");
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
        (out, log)
    }
}

#[cfg(unix)]
#[test]
fn npx_routes_prisma_and_unknown_tools_to_fake_npx() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }
    let env = fake_env(&["npx", "npm", "prisma", "next", "prettier", "playwright"]);

    // Other prisma subcommands pass straight through to `npx prisma ...`.
    let (out, log) = env.run(&["npx", "prisma", "migrate", "status"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(log.contains("npx prisma migrate status"), "log: {log}");

    // Bare `npx prisma` runs `npx prisma`.
    let (out, log) = env.run(&["npx", "prisma"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(log.contains("npx prisma"), "log: {log}");

    // Specialised prisma routes still reach a prisma launcher.
    let (out, log) = env.run(&["npx", "prisma", "generate"]);
    assert!(out.status.code().is_some(), "{out:?}");
    assert!(log.contains("generate"), "log: {log}");

    let (out, log) = env.run(&["npx", "prisma", "db", "push"]);
    assert!(out.status.code().is_some(), "{out:?}");
    assert!(log.contains("push"), "log: {log}");

    for (args, tool) in [
        (&["npx", "next", "build"][..], "next"),
        (&["npx", "prettier", "--check", "."], "prettier"),
        (&["npx", "playwright", "test"], "playwright"),
        (&["npx", "cowsay", "hello"], "cowsay"),
    ] {
        let (out, log) = env.run(args);
        assert!(out.status.code().is_some(), "{args:?}: {out:?}");
        assert!(
            log.contains(tool),
            "`rtk {}` never reached {tool}; log: {log}; stderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[cfg(unix)]
#[test]
fn language_tool_wrappers_dispatch_to_their_tools() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }
    let env = fake_env(&[
        "pytest", "python", "python3", "mypy", "sqlfluff", "php", "phpunit", "phpstan", "pest",
        "paratest", "ecs", "pint", "rake", "rubocop", "rspec", "bundle", "uv", "go", "sbt", "gt",
        "gradlew", "gradle", "mvnd", "mvn",
    ]);

    // Each wrapper must reach its own tool, named first in the fake's log line.
    let cases: &[(&[&str], &str)] = &[
        (&["sqlfluff", "lint", "q.sql"][..], "sqlfluff"),
        (&["pytest", "-q"][..], "pytest"),
        (&["mypy", "src"][..], "mypy"),
        (&["php", "-v"][..], "php"),
        (&["phpunit"][..], "phpunit"),
        (&["phpstan", "analyse", "src"][..], "phpstan"),
        (&["pest"][..], "pest"),
        (&["paratest"][..], "paratest"),
        (&["ecs", "check", "src"][..], "ecs"),
        (&["pint", "--test"][..], "pint"),
        (&["phpt", "Zend/tests/"][..], "php"),
        (&["rake", "test"][..], "rake"),
        (&["rubocop"][..], "rubocop"),
        (&["rspec"][..], "rspec"),
        (&["uv", "run", "python", "x.py"][..], "uv"),
        (&["go", "build", "./..."][..], "go"),
        (&["sbt", "test"][..], "sbt"),
        (&["sbt", "compile"][..], "sbt"),
        (&["sbt", "run"][..], "sbt"),
        (&["sbt", "clean"][..], "sbt"),
        (&["gt", "log"][..], "gt"),
        (&["gt", "submit"][..], "gt"),
        (&["gt", "sync"][..], "gt"),
        (&["gt", "restack"][..], "gt"),
        (&["gt", "create", "feature"][..], "gt"),
        (&["gt", "branch"][..], "gt"),
        (&["gt", "foo"][..], "gt"),
        (&["gradlew", "build"][..], "gradlew"),
        (&["mvnd", "test"][..], "mvnd"),
    ];
    for (args, tool) in cases {
        let (out, log) = env.run(args);
        assert!(out.status.code().is_some(), "{args:?}: {out:?}");
        assert!(
            log.lines()
                .any(|line| line.split(' ').next() == Some(*tool)),
            "`rtk {}` never reached {tool}; log: {log}; stderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[cfg(unix)]
#[test]
fn verbose_bypass_splits_a_quoted_command_and_streams_both_pipes() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }
    let env = fake_env(&[]);
    write_script(
        &env.bin.path().join("bypasstool"),
        "#!/bin/sh\necho bypass-stdout-line\necho bypass-stderr-line >&2\nexit 3\n",
    );

    let (out, _) = env.run(&["-v", "bypass", "bypasstool a b"]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stdout.contains("bypass-stdout-line"), "stdout: {stdout}");
    assert!(stderr.contains("bypass-stderr-line"), "stderr: {stderr}");
    assert!(
        stderr.contains("Bypass mode: bypasstool a b"),
        "stderr: {stderr}"
    );
}
