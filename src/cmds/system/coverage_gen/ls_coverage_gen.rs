use super::*;

#[test]
fn full_time_requests_long_output_only_when_used_as_an_option() {
    for flag in ["--full-time", "--full-t"] {
        let option = plan_for(&[flag.to_string()], Flavor::Gnu);
        assert!(option.show_long, "{flag} should request long output");
        assert_eq!(option.child_args, ["-l", flag, "--", "."]);

        let operand = plan_for(&["--".to_string(), flag.to_string()], Flavor::Gnu);
        assert!(!operand.show_long, "{flag} is a path after --");
        assert_eq!(operand.child_args, ["-l", "--", flag]);
    }
}

#[test]
fn verbose_long_listing_keeps_sizes_and_reports_hidden_rows_without_recovery() {
    use crate::core::{test_isolation, user_env};

    let raw = "\
-rw 1 user staff 1024 Jan 1 12:00 data.bin
-rw-r--r-- 1 99 Jan 1 12:00 incomplete.txt
";
    let (entries, parsed, truncated, filtered) = compact_ls(raw, false, true);
    assert_eq!(entries, "data.bin  1.0K\n");
    assert_eq!(parsed, 1);
    assert!(truncated.is_empty());
    assert_eq!(filtered, ["-rw-r--r-- 1 99 Jan 1 12:00 incomplete.txt"]);

    let root = test_isolation::tempdir();
    let blocked_tee_dir = root.path().join("blocked-tee");
    std::fs::write(&blocked_tee_dir, b"not a directory").expect("block recovery file creation");

    test_isolation::with_root(root.path(), || {
        user_env::with_vars(&[("RTK_TEE", Some("0"))], || {
            user_env::with_path("RTK_TEE_DIR", Some(&blocked_tee_dir), || {
                assert_eq!(
                    render(raw, None, false, true, 1),
                    "data.bin  1.0K\n... (1 filtered)\n"
                );

                // Exercise both zero input bytes and a listing that grows when rendered.
                for empty_listing in ["", ".\n"] {
                    assert_eq!(render(empty_listing, None, false, true, 1), "(empty)\n");
                }
            });
        });
    });
}
