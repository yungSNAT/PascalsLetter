use super::*;

#[test]
fn chain_metadata_uses_the_endpoints_and_requires_every_member_to_be_cache_stable() {
    static TOON_FIRST: &[&dyn Method] = &[&crate::methods::TOON, &crate::methods::ANSI];
    static TOON_LAST: &[&dyn Method] = &[&crate::methods::ANSI, &crate::methods::TOON];

    let defaults = MethodInfo::LOSSLESS_TRANSFORM;
    for methods in [METHODS, TOON_FIRST, TOON_LAST, &[]] {
        let info = chain_info(methods);
        let first = methods.first().map_or(defaults, |method| method.info());
        let last = methods.last().map_or(defaults, |method| method.info());

        assert_eq!(info.consumes, first.consumes);
        assert_eq!(info.produces, last.produces);
        assert_eq!(info.terminal, last.terminal);
        assert_eq!(
            info.cache_stable,
            methods.iter().all(|method| method.info().cache_stable)
        );
    }
}

#[test]
fn redact_and_poisoned_registry_in_isolation() {
    const CHILD_ARG: &str = "--skip=rtk-engine-registry-coverage-child-marker";
    const DONE: &str = "isolated registry assertions completed";

    // The child runs only this test, so its filled slot and poisoned mutex
    // cannot affect the rest of the suite.
    if std::env::args_os().any(|arg| arg == CHILD_ARG) {
        assert!(redact().is_none());
        assert_eq!(register_redact(&crate::methods::ANSI), Ok(()));
        assert_eq!(
            register_redact(&crate::methods::TOON).unwrap_err(),
            "redact slot already filled"
        );
        assert_eq!(redact().map(|method| method.name()), Some(names::ANSI));

        let baseline = known_names();
        // Seed the private list so skipping it on poison is observable.
        registered().lock().unwrap().push(&crate::methods::ANSI);
        let mut with_extra = baseline.clone();
        with_extra.push(names::ANSI.to_string());
        assert_eq!(known_names(), with_extra);

        assert!(
            std::thread::spawn(|| {
                let _guard = registered().lock().unwrap();
                panic!("deliberately poison the isolated registry");
            })
            .join()
            .is_err()
        );

        assert_eq!(
            register(&crate::methods::TOON).unwrap_err(),
            "registry poisoned"
        );
        assert_eq!(known_names(), baseline);
        println!("{DONE}");
        return;
    }

    let (_, module) = module_path!().split_once("::").unwrap();
    let test_name = format!("{module}::redact_and_poisoned_registry_in_isolation");
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name.as_str(), CHILD_ARG, "--nocapture"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(DONE),
        "isolated registry test did not pass ({}):\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
