use super::*;
use crate::core::test_isolation;

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
