use super::project_root;
use crate::core::test_isolation;

#[test]
fn project_root_returns_none_until_an_ancestor_has_the_marker() {
    let root = test_isolation::tempdir();
    let nested = root.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let _entered = test_isolation::enter(&nested);

    let marker = ".rtk-project-root-coverage-marker";
    assert_eq!(project_root(marker), None);

    std::fs::create_dir(root.path().join(marker)).unwrap();
    assert_eq!(project_root(marker).as_deref(), Some(root.path()));
}
