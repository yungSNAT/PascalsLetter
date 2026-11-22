#![cfg(feature = "engine")]

use super::{run_bench, run_methods_list};
use crate::core::{pipeline_config, test_isolation};
use rtk_engine::block::ContentFormat;
use std::{fs, process::Command};

fn successful_stdout(command: &mut Command) -> String {
    let output = command.output().expect("run isolated rtk");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{command:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("rtk emits UTF-8")
}

#[test]
fn bench_reports_transform_savings_and_distinguishes_no_match_from_no_rules() {
    const PIPELINE: &str = r#"
schema_version = 1

[pipeline]
name = "bench-fixture"
extends = "rtk:default"

[[hook.after_tool.shell]]
name = "bench-repeat"
when = { lines_gt = 2, bytes_gt = 100, tokens_gt = 20 }
use = ["dedup"]

[[hook.after_tool.read]]
name = "bench-empty"
use = ["dedup"]
"#;

    let root = test_isolation::tempdir();
    let fixtures = [
        (
            "repeat.log",
            "Building the same deterministic work item with verbose benchmark output\n".repeat(64),
        ),
        ("short.log", "short\n".to_owned()),
        ("empty.json", String::new()),
        ("page.htm", "<p>Benchmark fixture</p>\n".to_owned()),
    ];
    for (name, contents) in &fixtures {
        fs::write(root.path().join(name), contents).unwrap();
    }

    // Exercise the implementation in the test process even when no CLI binary
    // is available. A benchmark must leave every input byte untouched.
    test_isolation::with_root(root.path(), || {
        let _working_dir = test_isolation::enter(root.path());
        let dir = pipeline_config::user_pipelines_dir().unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("bench-fixture.toml"), PIPELINE).unwrap();

        let loaded = pipeline_config::load();
        assert_eq!(loaded.resolved.name, "bench-fixture");
        for (name, contents) in &fixtures {
            let path = root.path().join(name);
            assert_eq!(run_bench(&path).unwrap(), 0, "{name}");
            assert_eq!(&fs::read_to_string(&path).unwrap(), contents, "{name}");
        }
    });

    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    // Use one isolated child environment for both installation and measurement.
    let base = test_isolation::rtk_command();
    let rtk = || {
        // nosemgrep: dynamic-command-execution -- a test: runs the rtk binary under test from test_isolation::rtk_command
        let mut command = Command::new(base.get_program());
        command
            .args(base.get_args())
            .env_clear()
            .envs(
                base.get_envs()
                    .filter_map(|(key, value)| value.map(|value| (key, value))),
            )
            .current_dir(root.path());
        command
    };
    let source = root.path().join("source.toml");
    fs::write(&source, PIPELINE).unwrap();
    let installed = successful_stdout(rtk().args(["pipeline", "add", "--force"]).arg(&source));
    assert!(installed.contains("installed:"), "{installed}");

    let bench = |name: &str| {
        successful_stdout(
            rtk()
                .args(["pipeline", "bench"])
                .arg(root.path().join(name)),
        )
    };
    let term = ContentFormat::Term.as_str();
    let repeated = bench("repeat.log");
    assert!(
        repeated.contains(&format!(
            "rule [bench-repeat] chain [dedup] (hook.after_tool.shell, format={term})"
        )),
        "{repeated}"
    );
    let measurements =
        regex::Regex::new(r"before: (\d+) tokens +after: (\d+) tokens +savings: (-?\d+\.\d)%")
            .unwrap();
    let fields = measurements
        .captures(&repeated)
        .unwrap_or_else(|| panic!("missing measurements:\n{repeated}"));
    let before = fields[1].parse::<usize>().unwrap();
    let after = fields[2].parse::<usize>().unwrap();
    assert_eq!(before, rtk_engine::tokens::estimate_tokens(&fixtures[0].1));
    assert!(after < before, "{repeated}");
    let savings = 100.0 - (after as f64 / before as f64 * 100.0);
    assert_eq!(&fields[3], format!("{savings:.1}"));

    let empty = bench("empty.json");
    assert!(
        empty.contains("rule [bench-empty] chain [dedup]"),
        "{empty}"
    );
    assert!(
        empty.contains("before: 0 tokens   after: 0 tokens   savings: 0.0%"),
        "{empty}"
    );

    let short = bench("short.log");
    assert!(
        short.contains(&format!(
            "no rule matches (hook.after_tool.shell, format={term}) in `bench-fixture`"
        )),
        "{short}"
    );
    let hint = "hint: `extends = \"rtk:posthook\"`";
    assert!(!short.contains(hint), "{short}");
    assert!(!short.contains("savings:"), "{short}");

    let page = bench("page.htm");
    let web = ContentFormat::Web.as_str();
    assert!(
        page.contains(&format!(
            "no rule matches (hook.after_tool.web, format={web}) in `bench-fixture`"
        )),
        "{page}"
    );
    assert!(page.contains(hint), "{page}");
}

#[test]
fn methods_list_reports_registry_metadata_and_expands_composites() {
    use rtk_engine::{directive::DIRECTIVES, method::MethodKind, registry};

    assert_eq!(run_methods_list().unwrap(), 0);
    if !test_isolation::rtk_binary_is_built() {
        return;
    }

    let stdout = successful_stdout(test_isolation::rtk_command().args(["methods", "list"]));
    let (table, remainder) = stdout
        .split_once("\nchains (composites, usable anywhere a method is):\n")
        .expect("composite section");
    let mut rows = table.lines().filter(|line| !line.trim().is_empty());
    assert!(
        rows.next()
            .unwrap()
            .split_whitespace()
            .eq(["method", "kind", "lossy", "recovery", "cost", "site"])
    );
    let hook = registry::METHODS
        .iter()
        .map(|method| (method.name(), method.info(), "hook/proxy"));
    let proxy = registry::PROXY_METHOD_INFO.iter().map(|(name, info)| {
        let name: &str = name;
        (name, *info, "proxy")
    });
    for (name, info, site) in hook.chain(proxy) {
        let kind = match info.kind {
            MethodKind::Transform => "transform",
            MethodKind::Directive => "directive",
        };
        let lossy = info.lossy.to_string();
        let recovery = format!("{:?}", info.recovery).to_lowercase();
        let cost = format!("{:?}", info.cost).to_lowercase();
        let row = rows
            .next()
            .unwrap_or_else(|| panic!("missing row for {name}"));
        assert!(
            row.split_whitespace().eq([
                name,
                kind,
                lossy.as_str(),
                recovery.as_str(),
                cost.as_str(),
                site,
            ]),
            "incorrect metadata for {name}: {row}"
        );
    }
    assert_eq!(rows.next(), None);

    let (chains, tail) = remainder
        .split_once("\ndirectives:\n")
        .expect("directive section");
    let mut chain_rows = chains.trim().lines();
    for name in registry::CHAIN_NAMES {
        let (label, expansion) = chain_rows.next().unwrap().split_once('=').unwrap();
        assert_eq!(label.trim(), *name);
        let members: Vec<_> = registry::chain(name)
            .unwrap()
            .iter()
            .map(|method| method.name())
            .collect();
        assert_eq!(expansion.trim(), format!("[{}]", members.join(", ")));
    }
    assert_eq!(chain_rows.next(), None);

    let (directives, reserved) = tail.split_once("reserved names: ").expect("reserved names");
    assert!(
        directives.trim().lines().map(str::trim).eq(DIRECTIVES
            .iter()
            .map(|directive| directive.name())
            .chain(["rtk-awareness (levels: default | high | full)"])),
        "{directives}"
    );
    assert_eq!(reserved.trim(), registry::names::RESERVED.join(", "));
}
