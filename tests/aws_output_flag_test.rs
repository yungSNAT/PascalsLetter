//! `rtk aws` adds `--output json` to a structured read only when the user named no output
//! format. Stubs `aws` on PATH with a script that records its argv, so the assertions are on
//! what the AWS CLI actually received.

#![cfg(unix)]

mod common;

fn aws_argv(args: &[&str]) -> Vec<String> {
    let dir = common::tempdir();
    let argv_file = dir.path().join("argv.txt");
    let stub = dir.path().join("aws");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho '{{}}'\n",
            argv_file.display()
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
        .current_dir(dir.path())
        .arg("aws")
        .args(args)
        .output()
        .expect("spawn rtk");
    assert!(output.status.success(), "{output:?}");
    std::fs::read_to_string(&argv_file)
        .expect("read captured argv")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn an_explicit_output_format_is_kept_in_both_spellings() {
    assert_eq!(
        aws_argv(&["cloudfront", "list-distributions", "--output=table"]),
        ["cloudfront", "list-distributions", "--output=table"]
    );
    assert_eq!(
        aws_argv(&["cloudfront", "list-distributions", "--output", "table"]),
        ["cloudfront", "list-distributions", "--output", "table"]
    );
}

#[test]
fn a_structured_read_without_a_format_asks_for_json() {
    assert_eq!(
        aws_argv(&["cloudfront", "list-distributions"]),
        ["cloudfront", "list-distributions", "--output", "json"]
    );
}
