use super::*;
use crate::core::test_isolation;
use std::fs;

fn finite_workspace() -> serde_json::Value {
    serde_json::json!({
        "projects": {
            "app": {
                "targets": {
                    "build": { "builder": "@angular/build:application" }
                }
            }
        }
    })
}

#[test]
fn workspace_watch_detection_handles_missing_projects_and_nested_arrays() {
    let workspace = test_isolation::tempdir();
    let config = workspace.path().join("angular.json");
    fs::write(&config, r#"{"version":1}"#).expect("write workspace without projects");
    assert!(
        workspace_may_watch(workspace.path()),
        "a workspace without projects must pass through"
    );

    let mut value = finite_workspace();
    for watch in [false, true] {
        value["projects"]["app"]["targets"]["build"]["options"] =
            serde_json::json!({ "entries": [{"watch": false}, [{"watch": watch}]] });
        fs::write(&config, value.to_string()).expect("write workspace with nested options");
        assert_eq!(
            workspace_may_watch(workspace.path()),
            watch,
            "watch={watch}"
        );
    }
}

#[test]
fn compacting_the_final_bundle_row_does_not_add_a_newline() {
    let raw = "Initial chunk files   | Names   | Raw size\nmain.js   | main   | 1 kB";
    assert_eq!(
        filter_build(raw),
        "Initial chunk files | Names | Raw size\nmain.js | main | 1 kB"
    );
}

#[cfg(unix)]
#[test]
fn verbose_build_reports_the_command_and_compacts_the_table() {
    use std::os::unix::fs::PermissionsExt;

    if !test_isolation::rtk_binary_ready() {
        return;
    }

    let workspace = test_isolation::tempdir();
    fs::write(
        workspace.path().join("angular.json"),
        finite_workspace().to_string(),
    )
    .expect("write finite Angular workspace");

    let ng = workspace.path().join("ng");
    fs::write(
        &ng,
        "#!/bin/sh\n\
         printf '%s\\n' '❯ Building...' '✔ Building...' \
         'Initial chunk files          | Names          | Raw size' \
         'main.js                      | main           | 1 kB'\n",
    )
    .expect("write fake ng");
    fs::set_permissions(&ng, fs::Permissions::from_mode(0o700)).expect("make fake ng executable");

    let mut command = test_isolation::rtk_command();
    let inherited_path = command
        .get_envs()
        .find(|(name, _)| *name == "PATH")
        .and_then(|(_, value)| value)
        .expect("isolated rtk command has a PATH");
    let path = std::env::join_paths(
        std::iter::once(workspace.path().to_path_buf())
            .chain(std::env::split_paths(inherited_path)),
    )
    .expect("prepend fake ng directory to PATH");
    let output = command
        .current_dir(workspace.path())
        .env("PATH", path)
        .args(["-v", "ng", "build"])
        .output()
        .expect("run rtk ng build");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "rtk ng build failed: {stderr}");
    assert!(
        stderr.lines().any(|line| line == "Running: ng build"),
        "missing verbose command in stderr: {stderr}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Initial chunk files | Names | Raw size\nmain.js | main | 1 kB\n"
    );
}
