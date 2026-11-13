//! Without the `rtk-proxy` daemon, `rtk llm-proxy` says how to get one in this
//! distribution, which publishes no release archive of it.

mod common;

#[test]
fn a_missing_daemon_points_to_the_source_build_not_a_release_archive() {
    // rtk looks for the daemon next to its own executable and on PATH, so run
    // a copy from a directory that holds nothing else, with an empty PATH.
    let built = std::path::PathBuf::from(common::rtk_command().get_program());
    let dir = common::tempdir();
    let rtk = dir.path().join(built.file_name().unwrap());
    std::fs::copy(&built, &rtk).expect("copy rtk");
    let empty_path = common::tempdir();

    let mut cmd = std::process::Command::new(&rtk);
    common::isolate_rtk(&mut cmd);
    let out = cmd
        .env("PATH", empty_path.path())
        .args(["llm-proxy", "setup"])
        .output()
        .expect("rtk llm-proxy setup");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "setup without the daemon: {out:?}");
    assert!(stderr.contains("not found"), "{stderr}");
    assert!(
        stderr.contains("cargo build --release --manifest-path proxy/Cargo.toml"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("release archive (rtk-proxy-<target>)"),
        "{stderr}"
    );
    assert!(stderr.contains("publishes no release archive"), "{stderr}");
}
