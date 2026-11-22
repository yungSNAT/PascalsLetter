use super::*;

#[test]
fn generate_ignores_decorations_when_extracting_client_details() {
    let raw = "\
Prisma schema loaded
3 models generated
2 enums generated
7 types generated
Generated client at ./node_modules/@prisma/client
█ 99 models generated
▀ 99 models generated
▄ 99 models generated
┌ 99 models generated
└ 99 models generated
│ 99 models generated";

    assert_eq!(
        filter_prisma_generate(raw),
        "Prisma Client generated\n  • 3 models, 2 enums, 7 types\n  • Output: node_modules/@prisma/client"
    );
    assert_eq!(
        filter_prisma_generate("│ Generated client at ./node_modules/@prisma/client"),
        "Prisma Client generated"
    );
}

#[test]
fn migration_dev_reports_schema_changes_and_application() {
    let raw = r#"
Applying migration 20260512_add_sessions (local)
CREATE TABLE "Session" ("id" TEXT NOT NULL);
CREATE TABLE "Token" ("id" TEXT NOT NULL);
ALTER TABLE "Session" ADD FOREIGN KEY ("userId") REFERENCES "User" ("id");
CREATE INDEX "session_user_idx" ON "Session" ("userId");
CREATE UNIQUE INDEX "token_id_idx" ON "Token" ("id");
✓ Migration applied
"#;

    assert_eq!(
        filter_migrate_dev(raw),
        "Migration: 20260512_add_sessions\nChanges:\n  + 2 table(s)\n  ~ 1 table(s) modified\n  + 1 relation(s)\n  ~ 2 index(es)\n\nApplied | Pending: 0"
    );
}

#[test]
fn migration_status_counts_states_and_keeps_the_first_applied_id() {
    let raw = "\
Migration applied
20260512_add_sessions applied
20260511_init applied
20260513_add_tokens pending
20260514_add_indexes pending";

    assert_eq!(
        filter_migrate_status(raw),
        "Migrations: 3 applied, 2 pending\nLatest: 20260512_add_sessions"
    );
    assert_eq!(
        filter_migrate_status("20260511_init pending"),
        "Migrations: 0 applied, 1 pending"
    );
}

#[test]
fn migration_deploy_reports_success_or_at_most_five_errors() {
    assert_eq!(
        filter_migrate_deploy("20260511_init applied\n✓ 20260512_add_sessions\nDone"),
        "2 migration(s) deployed"
    );
    assert_eq!(
        filter_migrate_deploy(
            "✓ init\n  ERROR first  \n  error second\nERROR third\nerror fourth\nERROR fifth\nerror sixth"
        ),
        "[FAIL] Deployment failed:\n  ERROR first\n  error second\n  ERROR third\n  error fourth\n  ERROR fifth"
    );
}

#[test]
fn db_push_counts_create_alter_and_drop_lines() {
    let raw = "\
CREATE TABLE Session
CREATE TABLE Token
ALTER TABLE Session ADD COLUMN expires
ADD COLUMN created
ALTER TABLE Session DROP COLUMN legacy
DROP TABLE OldSession";

    assert_eq!(
        filter_db_push(raw),
        "Schema pushed to database\n  + 2 tables, ~ 3 columns, - 2 dropped"
    );
    assert_eq!(
        filter_db_push("No schema changes"),
        "Schema pushed to database"
    );
}

#[cfg(all(unix, feature = "engine"))]
mod cli {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Command, Output};

    const SCHEMA: &str = "fixture schema.prisma";
    const BANNER: &str = concat!(
        "│ Prisma command fixture: verbose setup information before displaying results\n",
        "│ Extra diagnostic detail that should not survive a successful filtered invocation\n"
    );

    fn install_tool(bin: &Path, name: &str) {
        let tool = bin.join(name);
        fs::write(
            &tool,
            r#"#!/bin/sh
printf '%s\n' "${0##*/}" "$@" > "$RTK_TEST_PRISMA_ARGS"
printf '%s' "$RTK_TEST_PRISMA_OUT"
printf '%s' "$RTK_TEST_PRISMA_ERR" >&2
exit "$RTK_TEST_PRISMA_CODE"
"#,
        )
        .expect("write fixture tool");
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755))
            .expect("make fixture tool executable");
    }

    fn command_path(command: &Command) -> &OsStr {
        command
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated command should set PATH")
    }

    fn rtk_with_tools(bin: &Path) -> Command {
        let mut command = rtk_command();
        let path = std::env::join_paths(
            std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(command_path(&command))),
        )
        .expect("join fixture and inherited tool paths");
        command.env("PATH", path).current_dir(bin);
        command
    }

    fn invocation(bin: &Path, args: &[&str], log: &Path) -> Command {
        let mut command = rtk_with_tools(bin);
        command
            .args(["-v", "prisma"])
            .args(args)
            .args(["--", "--schema", SCHEMA])
            .env("RTK_TEST_PRISMA_ARGS", log);
        command
    }

    fn run_fixture(command: &mut Command, stdout: &str, stderr: &str, code: &str) -> Output {
        command
            .env("RTK_TEST_PRISMA_OUT", stdout)
            .env("RTK_TEST_PRISMA_ERR", stderr)
            .env("RTK_TEST_PRISMA_CODE", code)
            .output()
            .expect("run isolated rtk")
    }

    #[test]
    fn prisma_cli_filters_success_and_preserves_failed_tool_output() {
        if !rtk_binary_is_built() {
            return;
        }

        let bin = tempdir();
        install_tool(bin.path(), "prisma");
        install_tool(bin.path(), "npx");
        let log = bin.path().join("tool-args");

        let help = rtk_with_tools(bin.path())
            .args(["prisma", "--help"])
            .output()
            .expect("read prisma command help");
        assert!(help.status.success(), "{help:?}");
        let help = String::from_utf8_lossy(&help.stdout);
        let db_args: &[&str] = if help
            .lines()
            .any(|line| line.split_whitespace().next() == Some("db-push"))
        {
            &["db-push"]
        } else {
            &["db", "push"]
        };

        for (cli_args, tool_args, label, raw, filtered) in [
            (
                ["generate"].as_slice(),
                ["generate"].as_slice(),
                "prisma generate",
                "2 models generated\n",
                "Prisma Client generated\n  • 2 models, 0 enums, 0 types\n  • Output: node_modules/@prisma/client",
            ),
            (
                ["migrate", "dev", "--name", "add sessions"].as_slice(),
                ["migrate", "dev", "--name", "add sessions"].as_slice(),
                "prisma migrate dev",
                "Applying migration 20260512_add_sessions\nCREATE TABLE Session\n✓ Migration applied\n",
                "Migration: 20260512_add_sessions\nChanges:\n  + 1 table(s)\n\nApplied | Pending: 0",
            ),
            (
                ["migrate", "status"].as_slice(),
                ["migrate", "status"].as_slice(),
                "prisma migrate status",
                "20260511_init applied\n20260512_add_sessions pending\n",
                "Migrations: 1 applied, 1 pending\nLatest: 20260511_init",
            ),
            (
                ["migrate", "deploy"].as_slice(),
                ["migrate", "deploy"].as_slice(),
                "prisma migrate deploy",
                "✓ 20260511_init\n20260512_add_sessions applied\n",
                "2 migration(s) deployed",
            ),
            (
                db_args,
                ["db", "push"].as_slice(),
                "prisma db push",
                "CREATE TABLE Session\nALTER TABLE Session ADD COLUMN owner\n",
                "Schema pushed to database\n  + 1 tables, ~ 1 columns, - 0 dropped",
            ),
        ] {
            let mut command = invocation(bin.path(), cli_args, &log);
            let tool_stderr = if label == "prisma generate" {
                "Generated into ./node_modules/@prisma/client\n"
            } else {
                ""
            };
            let success = run_fixture(&mut command, &format!("{BANNER}{raw}"), tool_stderr, "0");
            assert!(success.status.success(), "{label}: {success:?}");
            assert_eq!(String::from_utf8_lossy(&success.stdout).trim(), filtered);
            assert!(
                String::from_utf8_lossy(&success.stderr)
                    .lines()
                    .any(|line| line == format!("Running: {label}")),
                "{label}: {success:?}"
            );

            let expected_args = std::iter::once("prisma")
                .chain(tool_args.iter().copied())
                .chain(["--schema", SCHEMA])
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                fs::read_to_string(&log).expect("read fake tool arguments"),
                format!("{expected_args}\n"),
                "{label}"
            );

            let failure = run_fixture(
                &mut command,
                "fixture diagnostic from stdout\n",
                "fixture diagnostic from stderr\n",
                "23",
            );
            assert_eq!(failure.status.code(), Some(23), "{label}: {failure:?}");
            assert!(failure.stdout.is_empty(), "{label}: {failure:?}");
            assert!(
                String::from_utf8_lossy(&failure.stderr)
                    .contains("fixture diagnostic from stdout\nfixture diagnostic from stderr\n"),
                "{label}: {failure:?}"
            );
        }
    }

    #[test]
    fn prisma_cli_falls_back_to_npx_without_a_global_prisma() {
        if !rtk_binary_is_built() {
            return;
        }

        let bin = tempdir();
        install_tool(bin.path(), "npx");
        let log = bin.path().join("tool-args");
        let mut command = invocation(bin.path(), &["generate"], &log);

        // Do not run this case if the inherited PATH could select a real Prisma.
        if which::which_in("prisma", Some(command_path(&command)), bin.path()).is_ok() {
            return;
        }

        let output = run_fixture(
            &mut command,
            &format!("{BANNER}4 models generated\n"),
            "",
            "0",
        );
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "Prisma Client generated\n  • 4 models, 0 enums, 0 types"
        );
        assert_eq!(
            fs::read_to_string(log).expect("read fake npx arguments"),
            format!("npx\nprisma\ngenerate\n--schema\n{SCHEMA}\n")
        );
    }
}
