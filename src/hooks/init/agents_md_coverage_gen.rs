use super::*;
use crate::core::test_isolation::tempdir;

fn verbose_context(is_dry_run: bool) -> InitContext {
    let mut ctx = InitContext::default();
    let InitContext {
        verbose, dry_run, ..
    } = &mut ctx;
    *verbose = 1;
    *dry_run = is_dry_run;
    ctx
}

fn write_instructions(path: &Path, ctx: InitContext) -> Result<RtkBlockUpsert> {
    write_rtk_block(
        path,
        RTK_INSTRUCTIONS,
        "RTK instructions",
        "rtk init -g",
        ctx,
    )
}

#[test]
fn replaces_a_stale_block_at_the_start_without_losing_following_notes() {
    let temp = tempdir();
    let path = temp.path().join("AGENTS.md");

    for (tail, expected_tail) in [
        ("", ""),
        ("\n \n  Keep these notes.\n", "\n\nKeep these notes.\n"),
    ] {
        let original =
            format!(" \n{RTK_BLOCK_START} v1 -->\nObsolete instructions\n{RTK_BLOCK_END}{tail}");
        let expected = format!("{}{expected_tail}", RTK_INSTRUCTIONS.trim());
        fs::write(&path, &original).unwrap();

        assert_eq!(
            write_instructions(&path, verbose_context(true)).unwrap(),
            RtkBlockUpsert::Updated
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), original);

        assert_eq!(
            write_instructions(&path, InitContext::default()).unwrap(),
            RtkBlockUpsert::Updated
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);

        assert_eq!(
            write_instructions(&path, verbose_context(true)).unwrap(),
            RtkBlockUpsert::Unchanged
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    }
}

#[test]
fn preserves_an_incomplete_block_when_writing_or_migrating_instructions() {
    let temp = tempdir();
    let path = temp.path().join("AGENTS.md");
    let original = format!("# Team rules\n{RTK_MD_REF}\n\n{RTK_BLOCK_START} v1 -->\nUnfinished.\n");
    fs::write(&path, &original).unwrap();

    let error = write_instructions(&path, InitContext::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Refusing to modify malformed RTK instructions at {}",
            path.display()
        )
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    assert!(!patch_agents_md(&path, RTK_MD_REF, verbose_context(false)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn previews_then_migrates_a_relative_reference_to_the_requested_path() {
    let temp = tempdir();
    let path = temp.path().join("AGENTS.md");
    let reference = format!("@{}", temp.path().join("RTK.md").display());
    let original = format!("# Team rules\n\n{RTK_MD_REF}\n\nKeep this note.\n");
    let expected = format!("# Team rules\n\n{reference}\n\nKeep this note.\n");
    fs::write(&path, &original).unwrap();

    assert!(patch_agents_md(&path, &reference, verbose_context(true)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    assert!(patch_agents_md(&path, &reference, verbose_context(false)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);

    assert!(!patch_agents_md(&path, &reference, verbose_context(false)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
}

#[test]
fn removes_a_legacy_inline_block_without_adding_a_duplicate_reference() {
    let temp = tempdir();
    let path = temp.path().join("AGENTS.md");
    let original = format!("# Team rules\n\n{RTK_INSTRUCTIONS}\n\n{RTK_MD_REF}\n");
    fs::write(&path, &original).unwrap();

    assert!(!patch_agents_md(&path, RTK_MD_REF, verbose_context(true)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    assert!(!patch_agents_md(&path, RTK_MD_REF, verbose_context(false)).unwrap());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("# Team rules\n\n{RTK_MD_REF}\n")
    );
}

#[test]
fn previews_reference_addition_and_removal_without_changing_the_file() {
    let temp = tempdir();
    let path = temp.path().join("AGENTS.md");
    let original = "# Team rules\n\nUse the project's formatter.\n";
    fs::write(&path, original).unwrap();
    let refs = [RTK_MD_REF];

    assert!(patch_agents_md(&path, RTK_MD_REF, verbose_context(true)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    assert!(patch_agents_md(&path, RTK_MD_REF, verbose_context(false)).unwrap());
    let installed = format!("{}\n\n{RTK_MD_REF}\n", original.trim_end());
    assert_eq!(fs::read_to_string(&path).unwrap(), installed);

    assert!(remove_rtk_reference_from_agents(&path, &refs, verbose_context(true)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), installed);

    assert!(remove_rtk_reference_from_agents(&path, &refs, verbose_context(false)).unwrap());
    let removed = fs::read_to_string(&path).unwrap();
    assert_eq!(removed.trim_end(), original.trim_end());
    assert!(!remove_rtk_reference_from_agents(&path, &refs, verbose_context(false)).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), removed);
}

#[test]
fn reports_invalid_utf8_and_blocked_file_creation_without_overwriting_input() {
    let temp = tempdir();
    let path = temp.path().join("AGENTS.md");
    let invalid_utf8 = [0xff_u8];
    fs::write(&path, invalid_utf8).unwrap();

    let read_error = format!("Failed to read AGENTS.md: {}", path.display());
    for result in [
        patch_agents_md(&path, RTK_MD_REF, InitContext::default()),
        remove_rtk_reference_from_agents(&path, &[RTK_MD_REF], InitContext::default()),
    ] {
        assert_eq!(result.unwrap_err().to_string(), read_error);
    }

    let child = path.join("instructions.md");
    let error = write_instructions(&child, InitContext::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to write {}", child.display())
    );

    let error = patch_agents_md(&child, RTK_MD_REF, InitContext::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to write AGENTS.md: {}", child.display())
    );
    assert!(!child.exists());
    assert_eq!(fs::read(&path).unwrap(), invalid_utf8);
}

#[cfg(target_os = "linux")]
#[test]
fn reports_atomic_replacement_failures_for_readable_instructions() {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    let temp = tempdir();
    let mut file = tempfile::tempfile_in(temp.path()).unwrap();
    let original = format!(
        "# Team rules\n\n{RTK_BLOCK_START} v1 -->\nObsolete instructions\n{RTK_BLOCK_END}\n\n{RTK_MD_REF}\n"
    );
    file.write_all(original.as_bytes()).unwrap();

    // This process-local descriptor can reopen the scratch file for reading,
    // but procfs does not allow an atomic replacement, even as root.
    let path = Path::new("/proc/self/fd").join(file.as_raw_fd().to_string());
    if !path.is_file() {
        return;
    }

    let error = write_instructions(&path, InitContext::default()).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("Failed to write {}", path.display())
    );

    let reference = format!("@{}", temp.path().join("RTK.md").display());
    let write_error = format!("Failed to write AGENTS.md: {}", path.display());
    for result in [
        patch_agents_md(&path, &reference, InitContext::default()),
        patch_agents_md(&path, RTK_MD_REF, InitContext::default()),
        remove_rtk_reference_from_agents(&path, &[RTK_MD_REF], InitContext::default()),
    ] {
        assert_eq!(result.unwrap_err().to_string(), write_error);
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
}
