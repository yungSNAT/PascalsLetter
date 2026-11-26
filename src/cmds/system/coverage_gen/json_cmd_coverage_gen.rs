use super::*;
use crate::core::test_isolation;

#[test]
fn json_file_run_validates_before_reading_and_reports_failures() {
    let dir = test_isolation::tempdir();
    let _working_dir = test_isolation::enter(dir.path());
    test_isolation::with_root(dir.path(), || {
        for (extension, format_name) in [
            ("xml", "XML"),
            ("csv", "CSV"),
            ("ini", "INI"),
            ("env", "env"),
            ("txt", "plain text"),
        ] {
            let path = dir.path().join(format!("missing.{extension}"));
            let error = run(&path, 2, false, 0).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "{} is not a JSON file (detected {}). Use `rtk read` for non-JSON files.",
                    path.display(),
                    format_name
                )
            );
        }

        let path = dir.path().join("payload.json");
        let error = run(&path, 2, false, 0).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Failed to read file: {}", path.display())
        );
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::NotFound
        );

        fs::write(&path, "{").unwrap();
        let error = run(&path, 2, false, 0).unwrap_err();
        assert_eq!(error.to_string(), "Failed to parse JSON");

        fs::write(&path, r#"{"ready":true}"#).unwrap();
        run(&path, 2, true, 1).expect("a valid JSON file should render successfully");
    });
}

#[test]
fn compact_json_formats_arrays_by_shape_size_and_depth() {
    let input = r#"[[], [null, true, 3, "x", false], [7, 8, 9, 10, 11, 12], [[42]]]"#;
    assert_eq!(
        filter_json_compact(input, 2).unwrap(),
        concat!(
            "[\n",
            "  [],\n",
            "  [null, true, 3, \"x\", false],\n",
            "  [7, ... +5 more],\n",
            "  [\n",
            "    [...],\n",
            "  ],\n",
            "]",
        )
    );
}

#[test]
fn json_schema_classifies_values_and_honors_the_depth_limit() {
    let input = r#"{
        "url": "https://example.test",
        "nil": null,
        "ratio": 1.25,
        "empty": "",
        "date": "2026-02-03",
        "array": [],
        "object": {},
        "nested": {"hidden": 17}
    }"#;
    assert_eq!(
        filter_json_string(input, 1).unwrap(),
        "{array:[],date:date?,empty:string,nested:{hidden:...},nil:null,object:{},ratio:float,url:url}"
    );
}

#[cfg(unix)]
#[test]
fn json_stdin_prints_schema_and_verbose_diagnostic() {
    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let dir = test_isolation::tempdir();
    let path = dir.path().join("input.json");
    fs::write(&path, r#"{"ready":true}"#).unwrap();

    let output = test_isolation::rtk_command()
        .args(["-v", "json", "--keys-only", "-"])
        .stdin(fs::File::open(&path).unwrap())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "rtk json failed: {stderr}");
    assert_eq!(output.stdout.as_slice(), b"{ready:bool}\n");
    assert!(
        stderr
            .lines()
            .any(|line| line == "Analyzing JSON from stdin"),
        "missing verbose diagnostic: {stderr}"
    );
}
