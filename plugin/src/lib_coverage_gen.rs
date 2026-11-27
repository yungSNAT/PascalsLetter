use super::*;

#[test]
fn shorthand_plugin_exports_one_lossless_method_and_no_task_kinds() {
    assert!(tests::__rtk_plugin_kinds().is_empty());

    let methods = tests::__rtk_plugin_methods();
    assert_eq!(methods.len(), 1);

    let method = methods[0];
    assert_eq!(method.name(), "uppercase-demo");
    assert_eq!(
        format!("{:?}", method.info()),
        format!("{:?}", MethodInfo::LOSSLESS_TRANSFORM)
    );
}
