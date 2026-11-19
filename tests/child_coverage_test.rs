//! Under `cargo llvm-cov`, a spawned rtk's profile must land where the tool
//! collects this test's own, or none of the code the integration tests run
//! through the binary counts towards coverage.

use std::path::Path;

mod common;

/// Cargo has put test executables at more than one depth below
/// `llvm-cov-target` (`deps/`, then `build/<package>/<hash>/out/`), so the depth
/// is not a contract. Outside a coverage run there is nothing to collect.
#[test]
fn a_spawned_rtk_writes_its_profile_where_the_coverage_tool_collects_it() {
    let Some(own) = std::env::var_os("LLVM_PROFILE_FILE") else {
        return;
    };
    let collected = Path::new(&own).parent().expect("a profile directory");
    if !collected.ends_with("llvm-cov-target") {
        return;
    }
    let cmd = common::rtk_command();
    let child = cmd
        .get_envs()
        .find(|(name, _)| *name == "LLVM_PROFILE_FILE")
        .and_then(|(_, value)| value)
        .expect("a spawned rtk gets a profile path");
    assert_eq!(
        Path::new(child).parent(),
        Some(collected),
        "a spawned rtk's profile goes where cargo llvm-cov does not merge it"
    );
}
