use super::*;

#[test]
fn print_tasks_advertise_their_inputs_and_trim_worker_output() {
    let cases: [(&dyn TaskKind, &str, &str); 2] = [
        (
            &SUMMARIZE,
            "rtk <task> --question \"...\" <paths...>",
            "Never summarise style",
        ),
        (
            &DIGEST,
            "rtk <task> --question \"...\" -- <command>",
            "Quote error text verbatim",
        ),
    ];

    for (kind, usage, instructions_fragment) in cases {
        assert_eq!(kind.usage(), usage);
        assert!(kind.default_instructions().contains(instructions_fragment));

        let TaskOutcome::Print(text) = kind.handle("\n  - first\n  - second \t\n") else {
            panic!("{} should print its response", kind.name());
        };
        assert_eq!(text, "- first\n  - second");
    }

    let intent = TaskIntent {
        question: Some("Where is the parser?".to_string()),
        ..TaskIntent::default()
    };
    assert_eq!(
        SUMMARIZE.assemble(&intent, &[], None, None).unwrap_err(),
        "summarize needs at least one file path"
    );
}

#[test]
fn generate_assembles_the_spec_target_and_first_reference() {
    assert_eq!(
        GENERATE.usage(),
        "rtk <task> --spec \"...\" --reference <file> --target <file>"
    );
    assert!(
        GENERATE
            .default_instructions()
            .contains("Output ONLY the file's content")
    );

    let intent = TaskIntent {
        spec: Some("Add a parser.".to_string()),
        target: Some("src/parser.rs".to_string()),
        ..TaskIntent::default()
    };
    let corpus = [
        CorpusFile {
            path: "src/reference.rs".to_string(),
            content: "fn example() {}".to_string(),
        },
        CorpusFile {
            path: "src/extra.rs".to_string(),
            content: "fn ignore_me() {}".to_string(),
        },
    ];

    let request = GENERATE
        .assemble(&intent, &corpus, None, None)
        .expect("a spec, target, and reference are sufficient");
    assert_eq!(request.system, GENERATE.default_instructions());
    assert_eq!(request.max_tokens, 8192);
    assert_eq!(
        request.user,
        concat!(
            "Spec: Add a parser.\n\nTarget path: src/parser.rs\n\n",
            "Reference file:\n\n=== src/reference.rs ===\nfn example() {}"
        )
    );

    let customized = GENERATE
        .assemble(
            &intent,
            &corpus,
            Some("Match the example's naming."),
            Some("${target}: ${spec}\n${reference}"),
        )
        .expect("custom generation prompts can use all three substitutions");
    assert_eq!(customized.system, "Match the example's naming.");
    assert_eq!(customized.max_tokens, 8192);
    assert_eq!(
        customized.user,
        "src/parser.rs: Add a parser.\n=== src/reference.rs ===\nfn example() {}"
    );
}

struct NamedKind(&'static str);

impl TaskKind for NamedKind {
    fn name(&self) -> &'static str {
        self.0
    }

    fn usage(&self) -> &'static str {
        SUMMARIZE.usage()
    }

    fn default_instructions(&self) -> &'static str {
        SUMMARIZE.default_instructions()
    }

    fn assemble(
        &self,
        intent: &TaskIntent,
        corpus: &[CorpusFile],
        instructions: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<WorkerRequest, String> {
        SUMMARIZE.assemble(intent, corpus, instructions, prompt)
    }

    fn handle(&self, output: &str) -> TaskOutcome {
        SUMMARIZE.handle(output)
    }
}

#[test]
fn registry_validates_names_and_handles_poisoning() {
    const CHILD_MODE: &str = "RTK_ENGINE_TASK_COVERAGE_REGISTRY_CHILD";
    const CHILD_DONE: &str = "registry validation and poison checks completed";

    // Poison the registry only in a fresh process so parallel tests cannot see it.
    if std::env::var_os(CHILD_MODE).is_none() {
        let (_, test_name) = concat!(
            module_path!(),
            "::registry_validates_names_and_handles_poisoning"
        )
        .split_once("::")
        .expect("the module path includes the crate name");
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", test_name, "--nocapture"])
            .env(CHILD_MODE, "1")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run the isolated registry test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(CHILD_DONE),
            "isolated registry test did not complete ({})\nstdout:\n{stdout}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    static INVALID: [NamedKind; 3] = [NamedKind(""), NamedKind("two words"), NamedKind("1task")];
    static VALID: NamedKind = NamedKind("coverage-gen_registry");
    static NEW: NamedKind = NamedKind("coverage-gen_new");

    for kind in &INVALID {
        let name = kind.name();
        assert_eq!(
            register_kind(kind),
            Err(format!(
                "task kind name `{name}` must be alphanumeric with - or _, starting with a letter"
            ))
        );
        assert!(find_kind(name).is_none());
    }

    assert_eq!(register_kind(&VALID), Ok(()));
    assert_eq!(
        find_kind(VALID.name()).map(TaskKind::name),
        Some(VALID.name())
    );

    let panic = std::panic::catch_unwind(|| {
        let _guard = registered_kinds()
            .lock()
            .expect("registry is not yet poisoned");
        panic!("poison the isolated task registry");
    });
    assert!(panic.is_err());
    assert_eq!(
        register_kind(&NEW),
        Err("kind registry poisoned".to_string())
    );
    let available: Vec<_> = kinds().into_iter().map(TaskKind::name).collect();
    assert_eq!(available, ["summarize", "generate", "digest"]);

    println!("{CHILD_DONE}");
}
