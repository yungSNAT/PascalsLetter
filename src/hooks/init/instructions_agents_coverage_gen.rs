use super::*;
use crate::core::test_isolation;

#[cfg(unix)]
#[test]
fn cline_and_windsurf_preview_append_and_preserve_workspace_rules() {
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    // These agents resolve their rule files against the process working directory.
    // Set it on the child command only, leaving the parallel test harness untouched.
    for (agent, rules_file) in [("cline", ".clinerules"), ("windsurf", ".windsurfrules")] {
        let project = test_isolation::tempdir();
        let rules_path = project.path().join(rules_file);
        let init = |dry_run| {
            let mut cmd = test_isolation::rtk_command();
            cmd.current_dir(project.path())
                .args(["-v", "init", "--agent", agent]);
            if dry_run {
                cmd.arg("--dry-run");
            }
            let output = cmd.output().unwrap();
            let stdout = String::from_utf8(output.stdout).unwrap();
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(
                output.status.success(),
                "{agent} init failed ({}):\n{stdout}\n{stderr}",
                output.status
            );
            (stdout, stderr)
        };

        let (preview, _) = init(true);
        assert!(preview.contains(&format!("[dry-run] would write {rules_file}:")));
        assert!(preview.contains(&format!("[dry-run] content:\n{RTK_AWARENESS_FULL}")));
        assert!(!rules_path.exists(), "{agent} preview created a rules file");

        fs::write(&rules_path, "  Keep workspace guidance.\n").unwrap();
        let expected = format!("Keep workspace guidance.\n\n{RTK_AWARENESS_FULL}");
        let (installed, diagnostics) = init(false);
        assert!(installed.contains(&format!("Rules: {rules_file} (installed)")));
        assert!(installed.contains("Awareness: full"));
        assert!(diagnostics.contains(&format!("Wrote {rules_file}")));
        assert_eq!(fs::read_to_string(&rules_path).unwrap(), expected);

        let (preview, _) = init(true);
        assert!(!preview.contains("[dry-run] would write"), "{preview}");
        assert_eq!(fs::read_to_string(&rules_path).unwrap(), expected);

        let (unchanged, _) = init(false);
        assert!(unchanged.contains(&format!("Rules: {rules_file} (already present)")));
        assert_eq!(fs::read_to_string(&rules_path).unwrap(), expected);
    }
}

#[test]
fn kilocode_uses_workspace_rules_and_keeps_existing_guidance() {
    let project = test_isolation::tempdir();
    let _workspace = test_isolation::enter(project.path());
    let kilo_dir = project.path().join(".kilocode");
    let rules_dir = kilo_dir.join("rules");
    let rules_path = rules_dir.join("rtk-rules.md");
    let install = InitContext {
        verbose: 1,
        awareness: AwarenessLevel::High,
        ..InitContext::default()
    };
    let preview = InitContext {
        dry_run: true,
        ..install
    };

    run_kilocode_mode(preview).unwrap();
    assert!(!kilo_dir.exists(), "preview created the rules directory");

    fs::create_dir_all(&rules_dir).unwrap();
    fs::write(&rules_path, "\n  Follow workspace conventions. \n").unwrap();
    run_kilocode_mode(install).unwrap();
    let expected = format!("Follow workspace conventions.\n\n{RTK_AWARENESS_FULL}");
    assert_eq!(fs::read_to_string(&rules_path).unwrap(), expected);

    for ctx in [preview, install] {
        run_kilocode_mode(ctx).unwrap();
        assert_eq!(fs::read_to_string(&rules_path).unwrap(), expected);
    }
}

#[test]
fn kimi_previews_then_adds_full_instructions_to_workspace_agents_md() {
    let project = test_isolation::tempdir();
    let _workspace = test_isolation::enter(project.path());
    let agents_path = project.path().join(AGENTS_MD);
    let original = "# Project guidance\nKeep changes focused.";
    fs::write(&agents_path, original).unwrap();
    let install = InitContext {
        awareness: AwarenessLevel::High,
        ..InitContext::default()
    };

    run_kimi_mode(InitContext {
        dry_run: true,
        ..install
    })
    .unwrap();
    assert_eq!(fs::read_to_string(&agents_path).unwrap(), original);

    run_kimi_mode(install).unwrap();
    let updated = fs::read_to_string(&agents_path).unwrap();
    assert!(updated.starts_with(original));
    assert_eq!(updated.matches(RTK_BLOCK_START).count(), 1);
    assert!(updated.contains(RTK_AWARENESS_FULL));
    assert!(!project.path().join(".kimirules").exists());
}
