//! `rtk pnpm outdated` against a `pnpm` stub on PATH that prints fixed output and exits with a
//! fixed status. PNPM exits 1 when a dependency is outdated, and with other statuses (or 1
//! without its JSON report) when it fails.

#![cfg(unix)]

use std::process::Output;

mod common;

fn outdated(args: &[&str], stdout: &str, stderr: &str, status: i32) -> Output {
    let dir = common::tempdir();
    std::fs::write(dir.path().join("stdout.txt"), stdout).expect("write stdout");
    std::fs::write(dir.path().join("stderr.txt"), stderr).expect("write stderr");
    let stub = dir.path().join("pnpm");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\ncd '{0}'\nprintf '%s\\n' \"$@\" > argv.txt\ncat stdout.txt\ncat stderr.txt >&2\nexit {status}\n",
            dir.path().display()
        ),
    )
    .expect("write stub");
    let mut permissions = std::fs::metadata(&stub).expect("stat stub").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&stub, permissions).expect("chmod stub");

    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = common::rtk_command()
        .env("PATH", path)
        .args(["pnpm", "outdated"])
        .args(args)
        .output()
        .expect("spawn rtk");
    let argv = std::fs::read_to_string(dir.path().join("argv.txt")).expect("read argv");
    let mut expected = String::from("outdated\n--format\njson\n");
    for arg in args {
        expected.push_str(arg);
        expected.push('\n');
    }
    assert_eq!(argv, expected);
    output
}

#[test]
fn a_failure_keeps_pnpms_streams_and_status() {
    for (stdout, stderr, status) in [
        (
            "",
            "ERR_PNPM_NO_IMPORTER_MANIFEST_FOUND No package.json found in PUBLIC fixture\n",
            1,
        ),
        (
            "PUBLIC fatal output here\n",
            "ERR_PNPM_PUBLIC_FAILURE PUBLIC diagnostic\n",
            1,
        ),
        (
            "{\"public-prod\":{\"current\":\"1.0.0\",\"latest\":\"2.0.0\"}}\n",
            "PUBLIC unexpected native failure\n",
            42,
        ),
    ] {
        let output = outdated(&[], stdout, stderr, status);
        assert_eq!(output.status.code(), Some(status), "{output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), stdout);
        assert_eq!(String::from_utf8(output.stderr).unwrap(), stderr);
    }
}

#[test]
fn an_outdated_report_keeps_status_one_and_its_warnings_on_stderr() {
    let raw = "{\"public-prod\":{\"current\":\"1.0.0\",\"wanted\":\"1.1.0\",\"latest\":\"2.0.0\",\"dependencyType\":\"dependencies\"}}";
    let warning = "WARN PUBLIC registry notice\n";
    let output = outdated(&["--prod"], raw, warning, 1);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "1 outdated packages (of 1)\npublic-prod: 1.0.0 → 2.0.0\n"
    );
    assert_eq!(String::from_utf8(output.stderr).unwrap(), warning);
}

#[test]
fn an_up_to_date_project_keeps_the_shorter_native_report() {
    let output = outdated(&[], "{}\n", "", 0);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "{}\n\n");
}
