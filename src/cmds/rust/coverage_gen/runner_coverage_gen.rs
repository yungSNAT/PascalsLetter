#[cfg(unix)]
#[test]
fn runner_wrappers_distinguish_unrunnable_commands_from_other_spawn_errors() {
    use super::{run_err, run_test};
    use crate::core::test_isolation;
    use std::{fs, io, os::unix::fs::PermissionsExt};

    let scratch = test_isolation::tempdir();
    test_isolation::with_root(scratch.path(), || {
        let _working_dir = test_isolation::enter(scratch.path());
        let executable = scratch.path().join("unlaunchable-test");

        // The file passes executable lookup, but the kernel cannot execute
        // its shebang interpreter because `/` is a directory.
        fs::write(&executable, b"#!/\n").expect("write executable fixture");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("make fixture executable");

        let mut command = vec![executable.to_str().expect("UTF-8 fixture path").to_owned()];
        assert_eq!(
            run_test(&command, None, 0).expect("permission failure becomes an exit status"),
            126
        );

        // A NUL argument is rejected before execution and must not be
        // converted into an unrunnable-command exit status.
        command.push("invalid\0argument".to_owned());
        for (wrapper, result) in [
            ("err", run_err(&command, None, 0)),
            ("test", run_test(&command, None, 0)),
        ] {
            let error = result.expect_err("a NUL argument must produce a spawn error");
            let cause = error.root_cause().downcast_ref::<io::Error>();
            assert_eq!(
                cause.map(io::Error::kind),
                Some(io::ErrorKind::InvalidInput),
                "{wrapper}: {error:#}"
            );
        }
    });
}
