use super::filter_psql_output;

#[test]
fn table_output_omits_blank_lines() {
    let input = concat!(
        " id | label \n",
        "----+-------\n",
        "\n",
        "  7 | first \n",
        " \t \n",
        "  8 | second \n",
        "(2 rows)\n",
    );

    assert_eq!(filter_psql_output(input), "id\tlabel\n7\tfirst\n8\tsecond");
}

#[test]
fn expanded_output_keeps_the_notice_but_omits_blank_lines() {
    let input = concat!(
        "  NOTICE: showing sample rows  \n",
        " \t \n",
        "-[ RECORD 1 ]------\n",
        "id    | 7\n",
        "\n",
        "label | left | right\n",
        "-[ RECORD 2 ]------\n",
        "id    | 8\n",
        "(2 rows)\n",
    );

    assert_eq!(
        filter_psql_output(input),
        "NOTICE: showing sample rows\n[1] id=7 label=left | right\n[2] id=8"
    );
}

#[cfg(unix)]
#[test]
fn verbose_psql_forwards_arguments_and_filters_only_stdout() {
    use crate::core::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let tools = test_isolation::tempdir();
    let script = tools.path().join("psql");
    fs::write(
        &script,
        r#"#!/bin/sh
if [ "$#" -eq 2 ] && [ "$1" = 'coverage-db' ] && [ "$2" = 'coverage user' ]; then
    printf '%s\n' ' id | label ' '----+-------' ' 7  | fake row ' '(1 row)'
else
    printf '%s\n' 'unexpected psql arguments'
fi
printf '%s\n' 'fake psql diagnostic' >&2
"#,
    )
    .expect("write fake psql");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700))
        .expect("make fake psql executable");

    let mut cmd = test_isolation::rtk_command();
    let inherited_path = cmd
        .get_envs()
        .find_map(|(name, value)| if name == "PATH" { value } else { None })
        .expect("isolated rtk command has a PATH");
    let path = std::env::join_paths(
        std::iter::once(tools.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
    )
    .expect("prepend fake tool directory to PATH");

    let output = cmd
        .env("PATH", path)
        .args(["-v", "psql", "coverage-db", "coverage user"])
        .output()
        .expect("run rtk with fake psql");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        ["id\tlabel", "7\tfake row"]
    );
    assert!(
        stderr
            .lines()
            .any(|line| line == "Running: psql coverage-db coverage user"),
        "{stderr}"
    );
    assert!(!stderr.contains("fake psql diagnostic"), "{stderr}");
}
