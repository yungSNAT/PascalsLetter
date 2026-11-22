use super::*;

#[test]
fn run_reads_local_files_and_reports_a_missing_path() {
    let dir = crate::core::test_isolation::tempdir();
    for name in ["example.ts", "README"] {
        let path = dir.path().join(name);
        fs::write(&path, "const render = () => 1;\n").expect("write source fixture");
        run(&path, "unused", false, 1).expect("summarize readable source");
    }

    let missing = dir.path().join("missing.ts");
    let error = run(&missing, "unused", false, 0).expect_err("missing source should fail");
    assert_eq!(
        error.to_string(),
        format!("Failed to read file: {}", missing.display())
    );
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::NotFound)
    );
}

#[test]
fn javascript_and_typescript_summaries_report_dependencies_and_patterns() {
    let typescript = r#"import { useState } from 'react';
const helpers = require("helpers");
import { useEffect } from 'react';
import { name } from 'third';
import extra from 'fourth';
interface View {}
type Label = string;
class Panel {}
async function load() { await helpers.read(); }
const render = async () => useState(null);
export default render;
"#;
    let lang = Language::TypeScript;
    assert_eq!(
        extract_imports(typescript, &lang),
        ["react", "helpers", "third", "fourth"]
    );
    assert_eq!(extract_functions(typescript, &lang), ["load", "render"]);
    assert_eq!(
        extract_structs(typescript, &lang),
        ["View", "Label", "Panel"]
    );
    assert_eq!(extract_traits(typescript, &lang), ["View"]);

    let summary = analyze_code(typescript, &lang);
    assert_eq!(
        summary.line1,
        "TypeScript module (2 fn, 3 struct, 1 trait) - 11 lines"
    );
    assert_eq!(
        summary.line2,
        "uses: react, helpers, third | patterns: async, React hooks, ES modules"
    );

    let javascript = "\
import { useEffect } from 'react';
class Panel {}
async function render() { useEffect(refresh); }
";
    let summary = analyze_code(javascript, &Language::JavaScript);
    assert_eq!(summary.line1, "JavaScript functions (1 fn) - 3 lines");
    assert_eq!(summary.line2, "uses: react | patterns: React hooks");
}

#[test]
fn go_recognizes_receiver_methods_and_reports_only_the_first_three_definitions() {
    let functions = "\
func (w *Widget) render() {}
func repaint() {}
func refresh() {}
func hide() {}
func test_render() {}
func main() {}
func new() {}
";
    let lang = Language::Go;
    let summary = analyze_code(functions, &lang);
    assert_eq!(summary.line1, "Go functions (4 fn) - 7 lines");
    assert_eq!(summary.line2, "defines: render, repaint, refresh");

    let declarations = "\
import (
\t\"fmt\"
\t\"example.com/ui\"
)
type Widget struct {}
type ID int
";
    assert_eq!(
        extract_imports(declarations, &lang),
        ["fmt", "example.com/ui"]
    );
    assert_eq!(extract_structs(declarations, &lang), ["Widget"]);
}

#[test]
fn java_counts_classes_and_unsupported_languages_keep_generic_summaries() {
    let summary = analyze_code("public class Widget {}\nclass Frame {}\n", &Language::Java);
    assert_eq!(summary.line1, "Java data structures (2 struct) - 2 lines");
    assert_eq!(summary.line2, "General purpose code file");

    for (lang, name) in [
        (Language::C, "C"),
        (Language::Cpp, "C++"),
        (Language::Ruby, "Ruby"),
        (Language::Shell, "Shell"),
        (Language::Data, "Data"),
        (Language::Unknown, "Code"),
    ] {
        let summary = analyze_code("use fake::Thing;\nfn pretend() {}\n", &lang);
        assert_eq!(summary.line1, format!("{name} code (2 lines)"));
        assert_eq!(summary.line2, "General purpose code file");
    }
}

#[test]
fn pattern_detection_preserves_priority_and_recognizes_python_class_idioms() {
    let rust_with_many_patterns = "\
#[derive(Clone)]
struct Job;
impl Worker for Job {}
async fn poll() { task().await; }
#[test]
fn test_poll() {}
";
    assert_eq!(
        detect_patterns(rust_with_many_patterns, &Language::Rust),
        ["async", "trait impl", "derive"]
    );

    let rust_with_other_patterns = "\
fn make() -> Result<Box<dyn Worker>, Error> { todo!() }
#[test]
fn test_make() {}
";
    assert_eq!(
        detect_patterns(rust_with_other_patterns, &Language::Rust),
        ["error handling", "tests", "dyn dispatch"]
    );

    let python = "@dataclass\nclass Widget:\n    def __init__(self):\n        pass\n";
    let summary = analyze_code(python, &Language::Python);
    assert_eq!(summary.line1, "Python module (1 fn, 1 struct) - 4 lines");
    assert_eq!(summary.line2, "patterns: dataclass, OOP");
}
