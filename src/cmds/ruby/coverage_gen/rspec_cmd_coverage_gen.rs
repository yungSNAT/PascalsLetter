use super::*;

#[test]
fn noisy_json_preserves_pending_counts_and_failures_without_user_frames() {
    let input = r#"Running via Spring preloader in process 42
{
  "examples": [
    {
      "full_description": "Order calculates tax",
      "status": "failed",
      "file_path": "order_spec.rb",
      "line_number": 12,
      "exception": {
        "class": "RSpec::Expectations::ExpectationNotMetError",
        "message": "expected 10, got 11\nadditional detail",
        "backtrace": ["/bundle/gems/rspec-core/example.rb:20", "lib/rspec/core/runner.rb:30"]
      }
    },
    {
      "full_description": "Order applies discounts",
      "status": "failed",
      "file_path": "order_spec.rb",
      "line_number": 24,
      "exception": null
    },
    {
      "full_description": "Order supports refunds",
      "status": "pending",
      "file_path": "order_spec.rb",
      "line_number": 36
    }
  ],
  "summary": {
    "duration": 0.25,
    "example_count": 3,
    "failure_count": 2,
    "pending_count": 1
  }
}
Coverage report generated for RSpec
90 lines covered
"#;

    assert_eq!(
        filter_rspec_output(input),
        concat!(
            "RSpec: 0 passed, 2 failed, 1 pending (0.25s)\n\n",
            "Failures:\n",
            "1. ✗ Order calculates tax\n",
            "   order_spec.rb:12\n",
            "   ExpectationNotMetError: expected 10, got 11\n\n",
            "2. ✗ Order applies discounts\n",
            "   order_spec.rb:24",
        )
    );
}

#[test]
fn outside_error_does_not_create_an_empty_failure_section() {
    let input = r#"{
      "examples": [
        {
          "full_description": "Order saves",
          "status": "passed",
          "file_path": "order_spec.rb",
          "line_number": 8
        }
      ],
      "summary": {
        "duration": 0.5,
        "example_count": 1,
        "failure_count": 0,
        "pending_count": 0,
        "errors_outside_of_examples_count": 1
      }
    }"#;

    assert_eq!(
        filter_rspec_output(input),
        "RSpec: 1 passed, 0 failed (0.50s)"
    );
}

#[test]
fn text_keeps_the_last_failure_before_a_summary_or_rerun_list() {
    let location = "./test/order_spec.rb:12";
    let block = format!(
        "  1) Order calculates tax\n\
         Failure/Error: expect(total).to eq(10)\n\
         # {location}\n\
         # /bundle/gems/rspec-core/lib/rspec/core/example.rb:20\n"
    );
    let compact = format!(
        "1) Order calculates tax\n   Failure/Error: expect(total).to eq(10)\n   {location}"
    );
    assert_eq!(compact_failure_block(&block), compact);

    let summary = "2 examples, 1 failure";
    let rerun_list = format!("Failed examples:\n\nrspec {location} # Order calculates tax\n\n");
    let expected = format!("RSpec: {summary}\n1. ✗ {compact}");
    for section in ["", rerun_list.as_str()] {
        let input = format!("Failures:\n\n{block}\n{section}{summary}\n");
        assert_eq!(filter_rspec_text(&input), expected);
    }
}

#[test]
fn text_prefers_a_structured_summary_and_falls_back_to_the_last_loose_summary() {
    let structured = "\
....
  4 examples, 0 failures, 1 pending
Failures:
  1) Unrelated output after the summary
5 examples, 1 failure";
    assert_eq!(
        filter_rspec_text(structured),
        "RSpec: 4 examples, 0 failures, 1 pending"
    );

    let incomplete = "\
1 example, 1 pending
Failures:
  1) Order calculates tax
     Failure/Error: expect(total).to eq(10)
     # ./test/order_spec.rb:12
4 examples, 1 pending";
    assert_eq!(
        filter_rspec_text(incomplete),
        "RSpec: 4 examples, 1 pending"
    );
}

#[cfg(unix)]
#[test]
fn cli_injects_json_only_when_no_formatter_was_requested() {
    use crate::core::test_isolation::{rtk_binary_is_built, rtk_command, tempdir};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    if !rtk_binary_is_built() {
        return;
    }

    let dir = tempdir();
    let rspec = dir.path().join("rspec");
    fs::write(
        &rspec,
        r#"#!/bin/sh
case "$*" in
    '--format json --fail-fast')
        printf '%s\n' '{"examples":[],"summary":{"duration":0.1,"example_count":0,"failure_count":0,"pending_count":0}}'
        ;;
    '--format documentation'|'-f documentation'|'--format=documentation'|'-fdocumentation')
        printf '%s\n' \
            'DEPRECATION WARNING: The old runner reported 1 example, 1 failure' \
            'Order' \
            '  saves successfully' \
            'Finished in 0.1 seconds' \
            '1 example, 0 failures'
        ;;
    *)
        printf 'Unexpected rspec arguments: %s\n' "$*"
        ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&rspec, fs::Permissions::from_mode(0o700)).unwrap();

    let cases: &[(&[&str], bool)] = &[
        (&["--fail-fast"], true),
        (&["--format", "documentation"], false),
        (&["-f", "documentation"], false),
        (&["--format=documentation"], false),
        (&["-fdocumentation"], false),
    ];

    for &(args, inject_json) in cases {
        let mut command = rtk_command();
        let inherited_path = command
            .get_envs()
            .find_map(|(name, value)| if name == "PATH" { value } else { None })
            .expect("isolated rtk command should provide PATH");
        let path = std::env::join_paths(
            std::iter::once(dir.path().to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .unwrap();
        let output = command
            .current_dir(dir.path())
            .env("PATH", path)
            .args(["-v", "rspec"])
            .args(args)
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {stderr}");

        let (summary, injected) = if inject_json {
            ("RSpec: No examples found", " --format json")
        } else {
            ("RSpec: 1 example, 0 failures", "")
        };
        assert!(
            stdout.lines().any(|line| line == summary),
            "{args:?}: {stdout}"
        );
        let running = format!("Running: rspec{injected} {}", args.join(" "));
        assert!(
            stderr.lines().any(|line| line == running),
            "{args:?}: {stderr}"
        );
        assert!(!stderr.contains("JSON parse failed"), "{args:?}: {stderr}");
    }
}
