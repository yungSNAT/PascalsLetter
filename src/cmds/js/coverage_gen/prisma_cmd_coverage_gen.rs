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
fn migration_status_counts_applied_and_pending_migrations() {
    let raw = "\
20260512_add_sessions applied
20260511_init applied
20260513_add_tokens pending
20260514_add_indexes pending";

    // The `Latest:` line is left unpinned: it takes the first applied entry
    // today, which this fixture cannot tell from the newest one.
    assert_eq!(
        filter_migrate_status(raw).lines().next(),
        Some("Migrations: 2 applied, 2 pending")
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
    use crate::core::test_isolation::{rtk_binary_ready, rtk_command, tempdir};
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
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700))
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
        // Leave out any directory with a real Prisma, so the npx fallback runs.
        let inherited: Vec<_> = std::env::split_paths(command_path(&command))
            .filter(|dir| !dir.join("prisma").exists())
            .collect();
        let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(inherited))
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
    fn prisma_cli_falls_back_to_npx_without_a_global_prisma() {
        if !rtk_binary_ready() {
            return;
        }

        let bin = tempdir();
        install_tool(bin.path(), "npx");
        let log = bin.path().join("tool-args");
        let mut command = invocation(bin.path(), &["generate"], &log);

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
