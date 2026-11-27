use super::*;
use crate::core::test_isolation::{tempdir, with_root};

#[test]
fn opencode_install_then_dry_run_and_remove() {
    let root = tempdir();
    with_root(root.path(), || {
        let path = prepare_opencode_plugin_path().unwrap();
        assert!(path.starts_with(root.path()));
        assert!(!is_configured());
        assert_eq!(opencode_version(), None);

        let installed = run_opencode_only_mode(InitContext::default()).unwrap();
        assert_eq!(installed.as_deref(), Some(path.as_path()));
        assert_eq!(fs::read_to_string(&path).unwrap(), OPENCODE_PLUGIN);
        assert!(is_configured());

        let dry_run = InitContext {
            dry_run: true,
            ..InitContext::default()
        };
        let planned = remove_opencode_plugin(dry_run).unwrap();
        assert_eq!(planned.as_slice(), std::slice::from_ref(&path));
        assert_eq!(fs::read_to_string(&path).unwrap(), OPENCODE_PLUGIN);

        let verbose = InitContext {
            verbose: 1,
            ..InitContext::default()
        };
        let removed = remove_opencode_plugin(verbose).unwrap();
        assert_eq!(removed.as_slice(), std::slice::from_ref(&path));
        assert!(!path.exists());
        assert!(!is_configured());
        assert!(
            remove_opencode_plugin(InitContext::default())
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn opencode_uninstall_reports_a_directory_at_the_plugin_path() {
    let root = tempdir();
    with_root(root.path(), || {
        let path = prepare_opencode_plugin_path().unwrap();
        assert!(path.starts_with(root.path()));
        fs::create_dir_all(&path).unwrap();
        let user_file = path.join("keep.txt");
        fs::write(&user_file, "user data").unwrap();

        let error = remove_opencode_plugin(InitContext::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Failed to remove OpenCode plugin: {}", path.display())
        );
        assert_eq!(fs::read_to_string(user_file).unwrap(), "user data");
    });
}
