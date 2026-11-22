use super::tee_read_slug;
use crate::core::{test_isolation, user_dirs};
use std::path::Path;

#[test]
fn tee_read_slug_expands_both_home_prefixes() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let tee_dir = user_dirs::home()
            .expect("isolated home directory")
            .join("tee");

        for (cmd, expected_slug, file_name) in [
            (
                "tail -n +5 '~/tee/17_cargo-test.log'",
                "cargo-test",
                "17_cargo-test.log",
            ),
            (
                r#"grep error "$HOME/tee/18_build.log""#,
                "build",
                "18_build.log",
            ),
        ] {
            let (slug, expanded) = tee_read_slug(cmd, &tee_dir).expect(cmd);
            assert_eq!(slug, expected_slug, "{cmd}");
            assert_eq!(Path::new(&expanded), tee_dir.join(file_name), "{cmd}");
        }
    });
}
