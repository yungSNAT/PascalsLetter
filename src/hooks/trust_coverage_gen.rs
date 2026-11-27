use super::*;
use crate::core::test_isolation;
use std::fs;

fn in_isolated_project(test: impl FnOnce()) {
    let user_root = test_isolation::tempdir();
    let project = test_isolation::tempdir();
    test_isolation::with_root(user_root.path(), || {
        let _working_dir = test_isolation::enter(project.path());
        user_env::with_vars(&[("RTK_TRUST_PROJECT_FILTERS", None)], test);
    });
}

#[test]
fn trust_store_reports_filesystem_errors_and_can_recover() {
    let scratch = test_isolation::tempdir();
    let root = scratch.path().join("user");
    fs::create_dir(&root).unwrap();

    test_isolation::with_root(&root, || {
        let path = store_path().unwrap();
        let parent = path.parent().unwrap();
        let store = TrustStore {
            version: 1,
            ..TrustStore::default()
        };

        fs::create_dir_all(&path).unwrap();
        let Err(err) = read_store() else {
            panic!("a directory must not be readable as a trust store");
        };
        assert_eq!(
            err.to_string(),
            format!("Failed to read trust store: {}", path.display())
        );
        assert_eq!(
            write_store(&store).unwrap_err().to_string(),
            format!("Failed to write trust store: {}", path.display())
        );

        fs::remove_dir(&path).unwrap();
        fs::remove_dir(parent).unwrap();
        fs::write(parent, b"not a directory").unwrap();
        assert_eq!(
            write_store(&store).unwrap_err().to_string(),
            format!("Failed to create directory: {}", parent.display())
        );
        assert_eq!(fs::read(parent).unwrap(), b"not a directory");

        fs::remove_file(parent).unwrap();
        write_store(&store).unwrap();
        assert_eq!(read_store().unwrap().version, 1);
    });
}

#[test]
fn missing_filter_is_untrusted_and_ci_override_still_requires_readable_content() {
    in_isolated_project(|| {
        let path = user_dirs::in_working_dir("missing.toml");
        assert_eq!(
            check_trust_with_content(&path).unwrap(),
            (TrustStatus::Untrusted, None)
        );

        user_env::with_vars(
            &[
                ("RTK_TRUST_PROJECT_FILTERS", Some("1")),
                ("CI", Some("true")),
            ],
            || {
                let err = check_trust_with_content(&path).unwrap_err();
                assert_eq!(
                    err.to_string(),
                    format!("Failed to read filter: {}", path.display())
                );
                assert_eq!(
                    err.downcast_ref::<std::io::Error>().unwrap().kind(),
                    std::io::ErrorKind::NotFound
                );
            },
        );
    });
}

#[test]
fn active_filter_count_excludes_trusted_and_unreadable_files() {
    const FILTER: &str = r#"schema_version = 1

[filters.sample]
match_command = "^echo"
strip_lines_matching = ["^debug:"]
"#;

    in_isolated_project(|| {
        let project = user_dirs::in_working_dir(".rtk").join("filters.toml");
        let global = user_dirs::config()
            .unwrap()
            .join(crate::core::constants::FILTERS_TOML);
        assert_eq!(
            gated_filter_paths_labeled(),
            vec![("project", project.clone()), ("global", global.clone())]
        );
        assert_eq!(untrusted_active_filter_count(), 0);

        for path in [&project, &global] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, FILTER).unwrap();
        }
        assert_eq!(untrusted_active_filter_count(), 2);

        trust_filter_with_hash(&project, &integrity::compute_hash_bytes(FILTER.as_bytes()))
            .unwrap();
        assert_eq!(untrusted_active_filter_count(), 1);

        fs::write(&global, [0xff_u8]).unwrap();
        assert_eq!(untrusted_active_filter_count(), 0);
    });
}

#[test]
fn noninteractive_trust_distinguishes_empty_unreadable_and_invalid_inputs() {
    in_isolated_project(|| {
        let filter = user_dirs::in_working_dir(".rtk").join("filters.toml");
        let pipeline = crate::core::pipeline_config::project_pipeline_path();

        // Passing --yes keeps the review path from reading stdin.
        fs::create_dir_all(&filter).unwrap();
        // rtk joins `.rtk/filters.toml` with '/', so compare the ends, not the platform path.
        let err = run_trust(false, true).unwrap_err().to_string();
        assert!(
            err.starts_with("Failed to read ") && err.ends_with("filters.toml"),
            "{err}"
        );

        fs::remove_dir(&filter).unwrap();
        fs::write(&filter, "schema_version = 1\n\n[filters]\n").unwrap();
        assert!(
            run_trust(false, true)
                .unwrap_err()
                .to_string()
                .starts_with("No custom filters or pipeline found")
        );

        fs::create_dir_all(&pipeline).unwrap();
        assert_eq!(
            run_trust(false, true).unwrap_err().to_string(),
            format!("Failed to read {}", pipeline.display())
        );

        fs::remove_dir(&pipeline).unwrap();
        fs::write(&pipeline, "[pipeline\n").unwrap();
        let result = run_trust(false, true);
        // The Apache-2.0 edition treats pipeline files as inert, so it trusts without parsing.
        if cfg!(feature = "engine") {
            assert_eq!(
                result.unwrap_err().to_string(),
                "Filter file present but not valid TOML — see the error above."
            );
            assert!(list_trusted().unwrap().is_empty());
        } else {
            result.unwrap();
        }
    });
}
