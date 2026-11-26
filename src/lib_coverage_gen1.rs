#[cfg(unix)]
mod spawned {
    use crate::core::test_isolation;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::{Output, Stdio};

    /// Tools rtk may launch for the dispatch arms below. Each fake records its
    /// name and argv to a log and exits 0 with no output.
    const FAKE_TOOLS: &[&str] = &[
        "ast-grep",
        "sg",
        "bun",
        "bunx",
        "cargo",
        "ctest",
        "deno",
        "eslint",
        "jest",
        "next",
        "ng",
        "npm",
        "npx",
        "playwright",
        "pnpm",
        "prettier",
        "prisma",
        "tsc",
        "vitest",
        "wget",
        "yarn",
    ];

    struct Sandbox {
        _root: tempfile::TempDir,
        work: PathBuf,
        log: PathBuf,
        path: OsString,
    }

    impl Sandbox {
        fn new() -> Self {
            let root = test_isolation::tempdir();
            let bin = root.path().join("fakebin");
            let work = root.path().join("work");
            std::fs::create_dir_all(&bin).expect("create fake bin dir");
            std::fs::create_dir_all(&work).expect("create work dir");
            let log = root.path().join("calls.log");
            let log_q = log.to_string_lossy().replace('\'', r"'\''");
            for tool in FAKE_TOOLS {
                let script = format!(
                    "#!/bin/sh\nprintf '%s ' \"${{0##*/}}\" \"$@\" >> '{log_q}'\nprintf '\\n' >> '{log_q}'\nexit 0\n"
                );
                let p = bin.join(tool);
                std::fs::write(&p, script).expect("write fake tool");
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700))
                    .expect("chmod fake tool");
            }
            let inherited = test_isolation::rtk_command()
                .get_envs()
                .find(|(k, _)| *k == "PATH")
                .and_then(|(_, v)| v.map(|v| v.to_os_string()))
                .unwrap_or_default();
            let mut path = bin.into_os_string();
            if !inherited.is_empty() {
                path.push(":");
                path.push(&inherited);
            }
            Self {
                _root: root,
                work,
                log,
                path,
            }
        }

        fn run(&self, args: &[&str]) -> Output {
            test_isolation::rtk_command()
                .env("PATH", &self.path)
                .current_dir(&self.work)
                .args(args)
                .stdin(Stdio::null())
                .output()
                .expect("spawn rtk")
        }

        fn take_calls(&self) -> String {
            let calls = std::fs::read_to_string(&self.log).unwrap_or_default();
            let _ = std::fs::remove_file(&self.log);
            calls
        }
    }

    fn stderr_of(out: &Output) -> String {
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    fn stdout_of(out: &Output) -> String {
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// rtk parsed the command, dispatched it, and terminated normally.
    fn assert_dispatched(out: &Output, what: &str) {
        let stderr = stderr_of(out);
        assert!(out.status.code().is_some(), "{what}: killed by a signal");
        assert!(
            !stderr.contains("panicked"),
            "{what}: rtk panicked: {stderr}"
        );
        assert!(
            !stderr.contains("unexpected argument") && !stderr.contains("unrecognized subcommand"),
            "{what}: clap rejected the invocation: {stderr}"
        );
    }

    /// Runs `args`, checks it dispatched, and returns what the fake tools saw.
    fn run_and_calls(sb: &Sandbox, args: &[&str]) -> String {
        let out = sb.run(args);
        assert_dispatched(&out, &args.join(" "));
        sb.take_calls()
    }

    #[test]
    fn project_scoped_init_agents_refuse_global_and_dispatch_otherwise() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }
        let sb = Sandbox::new();

        let out = sb.run(&["init", "--agent", "kilocode", "--global"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_of(&out).contains("Kilo Code is project-scoped"),
            "{}",
            stderr_of(&out)
        );

        let out = sb.run(&["init", "--agent", "kimi", "--global"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_of(&out).contains("Kimi AI is project-scoped"),
            "{}",
            stderr_of(&out)
        );

        for args in [
            &["init", "--agent", "kilocode", "--dry-run"][..],
            &["init", "--agent", "kimi", "--dry-run"],
            &["init", "--copilot", "--dry-run"],
            &["init", "--copilot", "--global", "--dry-run"],
            &["init", "--uninstall", "--copilot", "--dry-run"],
            &["init", "--uninstall", "--copilot", "--global", "--dry-run"],
        ] {
            let out = sb.run(args);
            assert_dispatched(&out, &args.join(" "));
            assert!(
                !stderr_of(&out).contains("project-scoped"),
                "{args:?} must not hit the global refusal"
            );
        }
    }

    #[test]
    fn config_recall_sets_shows_and_rejects_modes() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }
        let sb = Sandbox::new();

        let out = sb.run(&["config", "recall", "bogus"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_of(&out).contains("unknown recall mode 'bogus'"),
            "{}",
            stderr_of(&out)
        );

        let out = sb.run(&["config", "recall", "tee"]);
        assert!(out.status.success(), "{}", stderr_of(&out));
        assert!(
            stdout_of(&out).contains("recall mode set to tee in"),
            "{}",
            stdout_of(&out)
        );

        let out = sb.run(&["config", "recall"]);
        assert!(out.status.success(), "{}", stderr_of(&out));
        assert!(stdout_of(&out).contains("tee"), "{}", stdout_of(&out));
    }

    #[test]
    fn npx_routes_tools_and_requires_a_command() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }
        let sb = Sandbox::new();

        let out = sb.run(&["npx"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr_of(&out).contains("npx requires a command argument"),
            "{}",
            stderr_of(&out)
        );

        let calls = run_and_calls(&sb, &["npx", "tsc", "--noEmit"]);
        assert!(calls.contains("tsc"), "npx tsc ran: {calls}");

        let calls = run_and_calls(&sb, &["npx", "eslint", "src"]);
        assert!(calls.contains("eslint"), "npx eslint ran: {calls}");

        let calls = run_and_calls(&sb, &["npx", "prisma", "generate"]);
        assert!(calls.contains("prisma generate"), "npx prisma ran: {calls}");
    }

    #[test]
    fn cargo_bun_and_deno_subcommands_reach_their_tool() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }
        let sb = Sandbox::new();

        let cases: &[(&[&str], &str)] = &[
            (&["cargo", "check"], "cargo check"),
            (&["cargo", "install", "ripgrep"], "cargo install"),
            (&["cargo", "nextest", "run"], "cargo nextest"),
            (&["cargo", "fmt"], "cargo fmt"),
            (&["bun", "install"], "bun install"),
            (&["bun", "add", "left-pad"], "bun add"),
            (&["bun", "remove", "left-pad"], "bun remove"),
            (&["bun", "run", "dev"], "bun run dev"),
            (&["bun", "build", "index.ts"], "bun build"),
            (&["bun", "test"], "bun test"),
            (&["bun", "pm", "ls"], "bun pm ls"),
            (&["bun", "pm", "cache", "rm"], "bun pm cache rm"),
            (&["bun", "x", "cowsay"], "cowsay"),
            (&["bunx", "figlet"], "figlet"),
            (&["bun", "upgrade"], "bun upgrade"),
            (&["deno", "test"], "deno test"),
            (&["deno", "check", "main.ts"], "deno check"),
            (&["deno", "lint"], "deno lint"),
            (&["deno", "run", "main.ts"], "deno run main.ts"),
            (&["deno", "task", "build"], "deno task build"),
            (&["deno", "compile", "main.ts"], "deno compile"),
            (&["deno", "install"], "deno install"),
            (&["deno", "fmt"], "deno fmt"),
        ];
        for (args, expected) in cases {
            let calls = run_and_calls(&sb, args);
            assert!(
                calls.contains(expected),
                "`rtk {}` should run `{expected}`, tools saw: {calls}",
                args.join(" ")
            );
        }
    }

    #[test]
    fn wget_ast_grep_and_ctest_forward_their_arguments() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }
        let sb = Sandbox::new();
        let url = "http://rtk-coverage.invalid/file";

        let calls = run_and_calls(&sb, &["wget", url, "-O", "saved.bin"]);
        assert!(calls.contains("wget"), "{calls}");
        assert!(calls.contains("saved.bin"), "-O must reach wget: {calls}");
        assert!(calls.contains(url), "{calls}");

        let calls = run_and_calls(&sb, &["wget", url, "-O", "-"]);
        assert!(calls.contains(url), "stdout mode still downloads: {calls}");

        let calls = run_and_calls(&sb, &["ast-grep", "run", "-p", "needle_fn($A)", "."]);
        assert!(calls.contains("needle_fn"), "{calls}");

        let calls = run_and_calls(&sb, &["ctest", "-R", "unit_only"]);
        assert!(calls.contains("ctest"), "{calls}");
        assert!(calls.contains("unit_only"), "{calls}");
    }

    #[test]
    fn js_tool_subcommands_dispatch() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }
        let sb = Sandbox::new();

        let prisma: &[(&[&str], &str)] = &[
            (&["prisma", "generate"], "generate"),
            (
                &["prisma", "migrate", "dev", "--name", "init"],
                "migrate dev",
            ),
            (&["prisma", "migrate", "status"], "migrate status"),
            (&["prisma", "migrate", "deploy"], "migrate deploy"),
            (&["prisma", "db-push"], "db push"),
        ];
        for (args, expected) in prisma {
            let calls = run_and_calls(&sb, args);
            assert!(
                calls.contains("prisma") && calls.contains(expected),
                "`rtk {}` should run prisma {expected}: {calls}",
                args.join(" ")
            );
        }

        for (args, tool) in [
            (&["vitest", "run"][..], "vitest"),
            (&["jest", "src"], "jest"),
            (&["tsc", "--noEmit"], "tsc"),
            (&["next", "build"], "next"),
            (&["playwright", "test"], "playwright"),
        ] {
            let calls = run_and_calls(&sb, args);
            assert!(
                calls.contains(tool),
                "`rtk {}` should run {tool}: {calls}",
                args.join(" ")
            );
        }
    }

    #[cfg(feature = "engine")]
    #[test]
    fn methods_list_names_the_builtin_methods() {
        if !test_isolation::rtk_binary_ready() {
            return;
        }
        let sb = Sandbox::new();

        let out = sb.run(&["methods", "list"]);
        assert!(out.status.success(), "{}", stderr_of(&out));
        assert!(stdout_of(&out).contains("dedup"), "{}", stdout_of(&out));
    }
}
