use super::route_tasks_text;
use std::{fs, path::Path};

// Discover a registered usage template without tying the routing test to one
// particular built-in task kind. Only inspect the crate's Rust source files.
fn registered_kind_with_placeholder(dir: &Path) -> Option<String> {
    fs::read_dir(dir)
        .expect("read crate source directory")
        .find_map(|entry| {
            let entry = entry.expect("read source directory entry");
            let file_type = entry.file_type().expect("read source entry type");
            let path = entry.path();

            if file_type.is_dir() {
                return registered_kind_with_placeholder(&path);
            }
            if !file_type.is_file() || path.extension().is_none_or(|ext| ext != "rs") {
                return None;
            }

            let source = fs::read_to_string(path).expect("read crate source file");
            source
                .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')))
                .filter(|name| !name.is_empty())
                .find(|name| {
                    crate::task::find_kind(name).is_some_and(|kind| kind.usage().contains("<task>"))
                })
                .map(str::to_owned)
        })
}

#[test]
fn route_tasks_renders_registered_usage_and_unknown_kind_fallback_in_order() {
    let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let kind_name = registered_kind_with_placeholder(&source_dir)
        .expect("a registered task kind has a <task> usage placeholder");
    let kind = crate::task::find_kind(&kind_name).expect("discovered kind is registered");
    let alias = "coverage-route-alias";
    let usage = kind.usage().replace("<task>", alias);

    assert_eq!(
        route_tasks_text(&[
            (alias, kind_name.as_str()),
            ("custom-job", "__unregistered_route_kind__"),
        ]),
        format!(
            "Delegation commands (they hand work to a cheaper model; the files never \
             enter your context):\n- `{usage}`\n- `rtk custom-job ...`"
        )
    );
}
