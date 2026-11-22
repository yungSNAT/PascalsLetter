use super::{filter_worktree_list, fold_ls_tree_paths};
use crate::core::{test_isolation, user_dirs};
use std::path::Path;

#[test]
fn worktree_list_shortens_home_paths_and_preserves_unparsed_rows() {
    let root = test_isolation::tempdir();
    test_isolation::with_root(root.path(), || {
        let listing = "\n \t\ncheckout     abc1234 (detached   HEAD)\n  incomplete row  \n";
        assert_eq!(
            filter_worktree_list(listing),
            "checkout abc1234 (detached HEAD)\n  incomplete row  "
        );

        let checkout = user_dirs::home().expect("isolated home").join("checkout");
        let path = checkout.to_string_lossy();
        // Worktree rows are split on whitespace, so this case needs a single-token path.
        if !path.chars().any(char::is_whitespace) {
            let listing = format!("{path}   def5678 [topic]\n");
            let abbreviated = Path::new("~").join("checkout");
            assert_eq!(
                filter_worktree_list(&listing),
                format!("{} def5678 [topic]", abbreviated.display())
            );
        }
    });
}

#[test]
fn ls_tree_fold_terminates_the_final_directory_run() {
    // These separators are part of Git's listing format, not host filesystem paths.
    let listing = "documentation/chapters/intro.md\ndocumentation/chapters/setup.md\nindex.md\n";
    let expected = "documentation/chapters/ intro.md setup.md\n./ index.md\n";
    assert_eq!(fold_ls_tree_paths(listing).as_deref(), Some(expected));
}

#[cfg(unix)]
mod cli {
    use super::test_isolation;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Output, Stdio};

    fn install_fake_git(dir: &Path, body: &str) {
        let script = dir.join("git");
        fs::write(&script, format!("#!/bin/sh\n{body}")).expect("write fake git");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("make fake git executable");
    }

    fn run_git(dir: &Path, args: &[&str]) -> Output {
        let mut cmd = test_isolation::rtk_command();
        let inherited_path = cmd
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value)
            .expect("isolated command PATH");
        let path = std::env::join_paths(
            std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(inherited_path)),
        )
        .expect("prepend fake git directory to PATH");

        cmd.env("PATH", path)
            .current_dir(dir)
            .stdin(Stdio::null())
            .args(["-v", "git"])
            .args(args)
            .output()
            .expect("run isolated rtk git")
    }

    #[test]
    fn worktree_separates_successful_writes_and_listings_from_failures() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let dir = test_isolation::tempdir();
        install_fake_git(
            dir.path(),
            r#"case "$*" in
    'worktree list')
        if [ -f unavailable ]; then
            printf '%s\n' 'fixture: listing unavailable' >&2
            exit 29
        fi
        printf '%s %s %s\n' 'fixture-checkout' 'abc1234' '[topic]'
        ;;
    'worktree lock fixture-checkout')
        : > checkout.lock
        ;;
    'worktree remove fixture-checkout')
        [ -f checkout.lock ] || exit 98
        printf '%s\n' 'fixture: checkout is locked' >&2
        exit 27
        ;;
    *) exit 99 ;;
esac
"#,
        );

        let listed = run_git(dir.path(), &["worktree"]);
        assert_eq!(listed.status.code(), Some(0), "{listed:?}");
        assert_eq!(
            std::str::from_utf8(&listed.stdout)
                .expect("worktree listing")
                .trim_end(),
            "fixture-checkout abc1234 [topic]"
        );

        let locked = run_git(dir.path(), &["worktree", "lock", "fixture-checkout"]);
        assert_eq!(locked.status.code(), Some(0), "{locked:?}");
        assert_eq!(locked.stdout.as_slice(), b"ok\n");
        assert!(dir.path().join("checkout.lock").is_file());

        let refused = run_git(dir.path(), &["worktree", "remove", "fixture-checkout"]);
        assert_eq!(refused.status.code(), Some(27), "{refused:?}");
        assert!(refused.stdout.is_empty(), "{refused:?}");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(
            stderr.contains("FAILED: git worktree remove fixture-checkout"),
            "{stderr}"
        );
        assert!(stderr.contains("fixture: checkout is locked"), "{stderr}");
        assert!(dir.path().join("checkout.lock").is_file());

        fs::write(dir.path().join("unavailable"), "").expect("make listing fail");
        let unavailable = run_git(dir.path(), &["worktree", "list"]);
        assert_eq!(unavailable.status.code(), Some(29), "{unavailable:?}");
        assert!(unavailable.stdout.is_empty(), "{unavailable:?}");
        let stderr = String::from_utf8_lossy(&unavailable.stderr);
        assert!(stderr.contains("fixture: listing unavailable"), "{stderr}");
    }

    #[test]
    fn verbose_git_reports_the_output_route_without_changing_passthrough_bytes() {
        if !test_isolation::rtk_binary_is_built() {
            return;
        }

        let dir = test_isolation::tempdir();
        install_fake_git(
            dir.path(),
            r#"case "$*" in
    'ls-tree -r --name-only HEAD')
        printf '%s\n' 'documentation/alpha.txt' 'documentation/beta.txt'
        ;;
    'ls-tree -rz --name-only HEAD')
        printf '%s\000%s\000' 'documentation/alpha.txt' 'documentation/beta.txt'
        ;;
    'log -p -1')
        printf '%s\n' 'fixture patch output'
        exit 23
        ;;
    *) exit 99 ;;
esac
"#,
        );

        let folded = run_git(dir.path(), &["ls-tree", "-r", "--name-only", "HEAD"]);
        assert_eq!(folded.status.code(), Some(0), "{folded:?}");
        assert_eq!(
            folded.stdout.as_slice(),
            b"documentation/ alpha.txt beta.txt\n"
        );
        let stderr = String::from_utf8_lossy(&folded.stderr);
        assert!(
            stderr.contains("git ls-tree (paths folded by directory)"),
            "{stderr}"
        );

        let raw = run_git(dir.path(), &["ls-tree", "-rz", "--name-only", "HEAD"]);
        assert_eq!(raw.status.code(), Some(0), "{raw:?}");
        assert_eq!(
            raw.stdout.as_slice(),
            b"documentation/alpha.txt\0documentation/beta.txt\0"
        );
        let stderr = String::from_utf8_lossy(&raw.stderr);
        assert!(stderr.contains("git ls-tree passthrough:"), "{stderr}");

        let failed = run_git(dir.path(), &["log", "-p", "-1"]);
        assert_eq!(failed.status.code(), Some(23), "{failed:?}");
        assert_eq!(failed.stdout.as_slice(), b"fixture patch output\n");
        let stderr = String::from_utf8_lossy(&failed.stderr);
        assert!(stderr.contains("git passthrough:"), "{stderr}");
    }
}
