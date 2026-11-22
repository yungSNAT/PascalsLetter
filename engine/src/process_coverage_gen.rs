use super::*;
use crate::method::CostClass;
use std::sync::mpsc;

#[test]
fn gate_waits_for_capacity_and_tolerates_a_poisoned_lock() {
    const CHILD_FLAG: &str = "RTK_ENGINE_PROCESS_GATE_COVERAGE_CHILD";
    const COMPLETED: &str = "isolated gate checks completed";

    // Poison the gate only in a fresh test process, never in the shared suite.
    if std::env::var_os(CHILD_FLAG).as_deref() != Some(std::ffi::OsStr::new("1")) {
        let qualified_name = concat!(
            module_path!(),
            "::gate_waits_for_capacity_and_tolerates_a_poisoned_lock"
        );
        let (_, test_name) = qualified_name.split_once("::").expect("libtest test name");
        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", test_name, "--nocapture"])
            .env(CHILD_FLAG, "1")
            .stdin(Stdio::null())
            .output()
            .expect("run isolated gate test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(COMPLETED),
            "isolated gate test returned {}\nstdout: {stdout}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let (full_tx, full_rx) = mpsc::channel();
    let (next_tx, next_rx) = mpsc::channel();
    let contender = std::thread::spawn(move || {
        let slots: Vec<_> = (0..MAX_CONCURRENT)
            .map(|_| GateGuard::acquire().expect("fill an available slot"))
            .collect();
        full_tx.send(slots).expect("transfer the occupied slots");
        let _slot = GateGuard::acquire().expect("acquire the released slot");
        next_tx.send(()).expect("report acquisition");
    });

    let deadline = Duration::from_secs(5);
    let mut slots = full_rx.recv_timeout(deadline).expect("gate fills");
    assert_eq!(
        next_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "an extra caller must wait while all slots are held"
    );
    drop(slots.pop().expect("release one slot"));
    next_rx
        .recv_timeout(deadline)
        .expect("release wakes caller");
    contender.join().expect("contender drops its slot");
    assert_eq!(*gate().active.lock().expect("active count"), slots.len());
    drop(slots);
    assert_eq!(*gate().active.lock().expect("active count"), 0);

    let slot = GateGuard::acquire().expect("hold a slot before poisoning");
    let active = &gate().active;
    let poisoned = std::panic::catch_unwind(|| {
        let _lock = active.lock().expect("gate starts unpoisoned");
        panic!("intentionally poison the isolated gate");
    });
    assert!(poisoned.is_err());
    assert!(active.is_poisoned());
    assert!(GateGuard::acquire().is_none());
    drop(slot);
    assert_eq!(
        *active
            .lock()
            .expect_err("gate remains poisoned")
            .into_inner(),
        1,
        "dropping a slot must not modify the poisoned counter"
    );

    println!("{COMPLETED}");
}

#[test]
fn process_method_reports_metadata_and_returns_successful_stdout() {
    let method = ProcessMethod {
        method_name: "prefix-input",
        binary: "/bin/sh",
        args: &[
            "-c",
            "IFS= read -r line && printf 'processed: %s\\n' \"$line\"",
        ],
        timeout: Duration::from_secs(5),
        method_info: MethodInfo {
            lossy: true,
            idempotent: false,
            cost: CostClass::Fast,
            ..MethodInfo::LOSSLESS_TRANSFORM
        },
    };

    assert_eq!(method.name(), "prefix-input");
    let info = method.info();
    assert!(info.lossy);
    assert!(!info.idempotent);
    assert!(matches!(info.cost, CostClass::Fast));

    #[cfg(unix)]
    {
        let block = crate::testutil::block("two words\\here\n", crate::block::ContentFormat::Term);
        assert_eq!(
            method.apply(&block, &MethodCtx::new("test")),
            Outcome::Continue("processed: two words\\here\n".to_owned())
        );
    }
}
