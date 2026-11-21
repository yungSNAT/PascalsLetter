//! Concurrent plugin registration keeps one entry per name.
//!
//! Each round releases several threads at once to register methods (or task
//! kinds) that share a fresh name. Exactly one registration of each name may
//! succeed; the rest must report the duplicate.

use std::sync::{Arc, Barrier};
use std::thread;

use rtk_engine::block::Block;
use rtk_engine::method::{Method, MethodCtx, MethodInfo, Outcome};
use rtk_engine::registry::{METHODS, find, known_names, register};
use rtk_engine::task::{
    CorpusFile, TaskIntent, TaskKind, TaskOutcome, WorkerRequest, find_kind, kinds, register_kind,
};

const ROUNDS: usize = 100;
const THREADS: usize = 8;

struct NamedMethod(&'static str);

impl Method for NamedMethod {
    fn name(&self) -> &'static str {
        self.0
    }
    fn info(&self) -> MethodInfo {
        METHODS[0].info()
    }
    fn apply(&self, _: &Block, _: &MethodCtx) -> Outcome {
        Outcome::Skip
    }
}

struct NamedKind(&'static str);

impl TaskKind for NamedKind {
    fn name(&self) -> &'static str {
        self.0
    }
    fn usage(&self) -> &'static str {
        ""
    }
    fn default_instructions(&self) -> &'static str {
        ""
    }
    fn assemble(
        &self,
        _: &TaskIntent,
        _: &[CorpusFile],
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<WorkerRequest, String> {
        Err("fixture".into())
    }
    fn handle(&self, output: &str) -> TaskOutcome {
        TaskOutcome::Print(output.into())
    }
}

/// Run `register` for one name on every thread at once and return the results.
fn race(
    register: impl Fn() -> Result<(), String> + Send + Sync + 'static,
) -> Vec<Result<(), String>> {
    let register = Arc::new(register);
    let start = Arc::new(Barrier::new(THREADS));
    let threads: Vec<_> = (0..THREADS)
        .map(|_| {
            let (register, start) = (Arc::clone(&register), Arc::clone(&start));
            thread::spawn(move || {
                start.wait();
                register()
            })
        })
        .collect();
    threads
        .into_iter()
        .map(|t| t.join().expect("registration thread"))
        .collect()
}

fn leaked(prefix: &str, round: usize) -> &'static str {
    Box::leak(format!("{prefix}-{round}").into_boxed_str())
}

#[test]
fn concurrent_method_registrations_keep_one_entry_per_name() {
    for round in 0..ROUNDS {
        let name = leaked("croft-manor-method", round);
        let methods: &'static [NamedMethod] = Box::leak(
            (0..THREADS)
                .map(|_| NamedMethod(name))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let results = race(move || {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            register(&methods[i])
        });
        let accepted = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(accepted, 1, "round {round}: {results:?}");
        for error in results.iter().filter_map(|r| r.as_ref().err()) {
            assert_eq!(error, &format!("method `{name}` is already registered"));
        }
        assert_eq!(find(name).map(|m| m.name()), Some(name));
        assert_eq!(known_names().iter().filter(|n| *n == name).count(), 1);
    }
}

#[test]
fn concurrent_task_registrations_keep_one_entry_per_name() {
    for round in 0..ROUNDS {
        let name = leaked("croft-manor-task", round);
        let kinds_for_threads: &'static [NamedKind] = Box::leak(
            (0..THREADS)
                .map(|_| NamedKind(name))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let results = race(move || {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            register_kind(&kinds_for_threads[i])
        });
        let accepted = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(accepted, 1, "round {round}: {results:?}");
        for error in results.iter().filter_map(|r| r.as_ref().err()) {
            assert_eq!(error, &format!("task kind `{name}` is already registered"));
        }
        assert_eq!(find_kind(name).map(|k| k.name()), Some(name));
        assert_eq!(kinds().iter().filter(|k| k.name() == name).count(), 1);
    }
}
