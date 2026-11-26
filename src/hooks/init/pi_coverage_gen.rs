use super::*;

type ScopedOperation = fn(bool, PatchMode, InitContext) -> Result<()>;

const UNINSTALLERS: [(&str, &str, ScopedOperation); 2] = [
    ("Pi", PI_LOCAL_DIR, uninstall_pi_with_patch_mode),
    ("OMP", OMP_LOCAL_DIR, uninstall_omp_with_patch_mode),
];

fn dry_run_context() -> InitContext {
    InitContext {
        dry_run: true,
        ..InitContext::default()
    }
}

#[test]
fn ownership_state_deduplicates_names_and_rejects_unknown_or_empty_records() {
    let tmp = test_isolation::tempdir();
    let plugin = tmp.path().join(PI_PLUGIN_FILE);
    fs::write(&plugin, PI_PLUGIN).unwrap();
    let state = shared_agent_state_path(&plugin);

    fs::write(&state, "\n pi \r\n\t\nomp\npi\n").unwrap();
    let ManagedAgentState::Known(agents) = read_managed_agents(&plugin).unwrap() else {
        panic!("ownership with whitespace and duplicates should still be known");
    };
    assert_eq!(agents, [PiCompatibleAgent::Pi, PiCompatibleAgent::Omp]);

    for invalid in ["pi\nunknown-agent\nomp\n", "\n \t\r\n"] {
        fs::write(&state, invalid).unwrap();
        assert!(
            matches!(
                read_managed_agents(&plugin).unwrap(),
                ManagedAgentState::Unknown
            ),
            "expected unknown ownership for {invalid:?}"
        );
    }
}

#[test]
fn installs_report_when_a_file_blocks_the_extensions_directory() {
    let tmp = test_isolation::tempdir();
    let _entered = test_isolation::enter(tmp.path());

    let cases: [(&str, &str, ScopedOperation); 2] = [
        ("Pi", PI_LOCAL_DIR, run_pi_mode_with_patch_mode),
        ("OMP", OMP_LOCAL_DIR, run_omp_mode_with_patch_mode),
    ];
    for (name, local_dir, install) in cases {
        let agent_dir = tmp.path().join(local_dir);
        fs::create_dir_all(&agent_dir).unwrap();
        let blocker = agent_dir.join(PI_EXTENSIONS_SUBDIR);
        fs::write(&blocker, "keep this file").unwrap();

        let error = install(false, PatchMode::Auto, InitContext::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "Failed to create local {name} extensions directory: {}",
                blocker.display()
            )
        );
        assert_eq!(fs::read_to_string(&blocker).unwrap(), "keep this file");
    }
}

#[test]
fn uninstall_previews_preserve_missing_unreadable_and_unrelated_extensions() {
    let tmp = test_isolation::tempdir();
    let _entered = test_isolation::enter(tmp.path());

    for (name, local_dir, uninstall) in UNINSTALLERS {
        let plugin = pi_plugin_path(&tmp.path().join(local_dir));
        fs::create_dir_all(plugin.parent().unwrap()).unwrap();
        let state = shared_agent_state_path(&plugin);
        let owners = "omp\npi\n";
        fs::write(&state, owners).unwrap();

        for ctx in [dry_run_context(), InitContext::default()] {
            uninstall(false, PatchMode::Auto, ctx).unwrap();
            assert!(!plugin.exists(), "{name} uninstall created a missing file");
            assert_eq!(fs::read_to_string(&state).unwrap(), owners);
        }

        let unreadable = [0xff, 0xfe, 0xfd];
        fs::write(&plugin, unreadable).unwrap();
        uninstall(false, PatchMode::Auto, dry_run_context()).unwrap();
        assert_eq!(fs::read(&plugin).unwrap(), unreadable);
        assert_eq!(fs::read_to_string(&state).unwrap(), owners);

        let unrelated = "// Extension from another tool\nexport default () => {};\n";
        fs::write(&plugin, unrelated).unwrap();
        assert!(
            !validate_stock_pi_plugin_path(
                &plugin,
                &format!("{name} extension"),
                PatchMode::Skip,
                dry_run_context(),
            )
            .unwrap()
        );
        for ctx in [dry_run_context(), InitContext::default()] {
            uninstall(false, PatchMode::Auto, ctx).unwrap();
            assert_eq!(fs::read_to_string(&plugin).unwrap(), unrelated);
            assert_eq!(fs::read_to_string(&state).unwrap(), owners);
        }
    }
}

#[test]
fn shared_uninstall_preserves_both_owners_until_removal_is_approved() {
    let tmp = test_isolation::tempdir();
    with_pi_dir_override(&tmp, |agent_dir| {
        let plugin = pi_plugin_path(agent_dir);
        fs::create_dir_all(plugin.parent().unwrap()).unwrap();
        let state = shared_agent_state_path(&plugin);
        let owners = "omp\npi\n";

        for (agent, uninstall) in [
            (
                PiCompatibleAgent::Pi,
                uninstall_pi_with_patch_mode as ScopedOperation,
            ),
            (PiCompatibleAgent::Omp, uninstall_omp_with_patch_mode),
        ] {
            fs::write(&plugin, PI_PLUGIN).unwrap();
            fs::write(&state, owners).unwrap();
            assert!(
                !confirm_shared_extension_uninstall(
                    true,
                    &plugin,
                    agent,
                    PatchMode::Skip,
                    dry_run_context(),
                )
                .unwrap()
            );
            uninstall(true, PatchMode::Skip, dry_run_context()).unwrap();
            assert_eq!(fs::read_to_string(&plugin).unwrap(), PI_PLUGIN);
            assert_eq!(fs::read_to_string(&state).unwrap(), owners);

            let error = uninstall(true, PatchMode::Skip, InitContext::default()).unwrap_err();
            assert!(error.to_string().contains("rerun with --auto-patch"));
            assert_eq!(fs::read_to_string(&plugin).unwrap(), PI_PLUGIN);
            assert_eq!(fs::read_to_string(&state).unwrap(), owners);

            uninstall(
                true,
                PatchMode::Auto,
                InitContext {
                    verbose: 1,
                    ..InitContext::default()
                },
            )
            .unwrap();
            assert!(!plugin.exists());
            assert!(!state.exists());
        }
    });
}

#[test]
fn uninstall_reports_sidecar_cleanup_failure_without_deleting_directory_contents() {
    let tmp = test_isolation::tempdir();
    let _entered = test_isolation::enter(tmp.path());
    let plugin = pi_plugin_path(&tmp.path().join(PI_LOCAL_DIR));
    fs::create_dir_all(plugin.parent().unwrap()).unwrap();
    fs::write(&plugin, PI_PLUGIN).unwrap();

    let state = shared_agent_state_path(&plugin);
    fs::create_dir(&state).unwrap();
    let sentinel = state.join("keep.txt");
    fs::write(&sentinel, "not an ownership record").unwrap();

    let error =
        uninstall_pi_with_patch_mode(false, PatchMode::Auto, InitContext::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Failed to remove RTK extension ownership state: {}",
            state.display()
        )
    );
    assert!(
        !plugin.exists(),
        "the extension is removed before its sidecar"
    );
    assert_eq!(
        fs::read_to_string(&sentinel).unwrap(),
        "not an ownership record"
    );
}

#[cfg(any(unix, windows))]
#[test]
fn uninstall_reports_blocked_removal_and_succeeds_after_unblocking() {
    let tmp = test_isolation::tempdir();
    let _entered = test_isolation::enter(tmp.path());

    for (name, local_dir, uninstall) in UNINSTALLERS {
        let plugin = pi_plugin_path(&tmp.path().join(local_dir));
        let parent = plugin.parent().unwrap();
        fs::create_dir_all(parent).unwrap();
        fs::write(&plugin, PI_PLUGIN).unwrap();

        let remove = || uninstall(false, PatchMode::Auto, InitContext::default());
        let assert_removal_failure = |result: Result<()>| {
            assert_eq!(
                result.unwrap_err().to_string(),
                format!("Failed to remove {name} extension: {}", plugin.display())
            );
            assert_eq!(fs::read_to_string(&plugin).unwrap(), PI_PLUGIN);
        };

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let probe = parent.join("permission-probe");
            fs::write(&probe, "").unwrap();
            let original = fs::metadata(parent).unwrap().permissions();
            let restricted = fs::Permissions::from_mode(original.mode() & !0o222);
            fs::set_permissions(parent, restricted).unwrap();

            // Privileged runners may still be able to unlink files in this directory.
            let probe_result = fs::remove_file(&probe);
            let blocked_result = probe_result.is_err().then(remove);
            fs::set_permissions(parent, original).unwrap();

            if let Err(error) = probe_result {
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                assert_removal_failure(blocked_result.unwrap());
            }
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            // FILE_SHARE_READ allows validation to read the file but denies deletion.
            let _lock = fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&plugin)
                .unwrap();
            assert_removal_failure(remove());
        }

        remove().unwrap();
        assert!(!plugin.exists());
    }
}

fn historical_pi_plugin() -> Option<String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    if !manifest_dir.join(".git").exists() {
        return None;
    }

    let git = |args: &[&str]| {
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(manifest_dir).args(args);
        test_isolation::isolate_git(&mut cmd);
        cmd.env("GIT_NO_LAZY_FETCH", "1");
        cmd.output().unwrap()
    };
    let revisions = git(&[
        "rev-list",
        "HEAD",
        "--full-history",
        "--",
        "hooks/pi/rtk.ts",
    ]);
    assert!(
        revisions.status.success(),
        "could not list Pi extension revisions"
    );

    let current = PI_PLUGIN.replace("\r\n", "\n");
    for revision in String::from_utf8(revisions.stdout).unwrap().lines() {
        let object = format!("{revision}:hooks/pi/rtk.ts");
        let file = git(&["show", &object]);
        if file.status.success() {
            let content = String::from_utf8(file.stdout).unwrap();
            if content.replace("\r\n", "\n").trim_end() != current.trim_end() {
                return Some(content);
            }
        }
    }
    None
}

#[test]
fn omp_status_distinguishes_current_stock_from_an_older_stock_extension() {
    let tmp = test_isolation::tempdir();
    let project = pi_plugin_path(&tmp.path().join(OMP_LOCAL_DIR));
    fs::create_dir_all(project.parent().unwrap()).unwrap();
    let current = PI_PLUGIN.replace("\r\n", "\n").replace('\n', "\r\n");
    fs::write(&project, &current).unwrap();
    assert!(is_current_pi_plugin(&current));
    print_omp_extension_status("Project extension", &project).unwrap();

    let agent_dir = tmp.path().join("omp-agent");
    let global = pi_plugin_path(&agent_dir);
    let historical = historical_pi_plugin();
    if let Some(content) = &historical {
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        fs::write(&global, content).unwrap();
        assert!(!is_current_pi_plugin(content));
        assert!(is_known_stock_pi_plugin(content));
        print_omp_extension_status("Global extension", &global).unwrap();
    }

    #[cfg(unix)]
    {
        if !test_isolation::rtk_binary_ready() {
            return;
        }
        let output = test_isolation::rtk_command()
            .current_dir(tmp.path())
            .env(PI_CODING_AGENT_DIR_ENV, &agent_dir)
            .args(["init", "--agent", "omp", "--show"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.lines().any(|line| {
                line.trim_start().starts_with("Project extension: ")
                    && line.ends_with(" (up to date)")
            }),
            "{stdout}"
        );
        let global_status = if historical.is_some() {
            " (stock version - will be replaced on next rtk init)"
        } else {
            " (not installed)"
        };
        assert!(
            stdout.lines().any(|line| {
                line.trim_start().starts_with("Global extension: ") && line.ends_with(global_status)
            }),
            "{stdout}"
        );
    }
}
