//! Summarizes project dependencies from lock files and manifests.

use crate::core::guard::never_worse;
use crate::core::tracking;
use crate::core::truncate::{CAP_WARNINGS, reduced};
use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::Path;
use toml_edit::{DocumentMut, Item, TableLike};

const MAX_DEPS: usize = CAP_WARNINGS;
// dev deps are secondary to prod — show fewer.
const MAX_DEV_DEPS: usize = reduced(CAP_WARNINGS, 5);

/// Summarize project dependencies
pub fn run(path: &Path, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let dir = if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    };

    if verbose > 0 {
        eprintln!("Scanning dependencies in: {}", dir.display());
    }

    let mut found = false;
    let mut rtk = String::new();
    let mut raw = String::new();

    let cargo_path = dir.join("Cargo.toml");
    if cargo_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&cargo_path).unwrap_or_default());
        rtk.push_str("Rust (Cargo.toml):\n");
        rtk.push_str(&summarize_cargo_str(&cargo_path)?);
    }

    let package_path = dir.join("package.json");
    if package_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&package_path).unwrap_or_default());
        rtk.push_str("Node.js (package.json):\n");
        rtk.push_str(&summarize_package_json_str(&package_path)?);
    }

    let requirements_path = dir.join("requirements.txt");
    if requirements_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&requirements_path).unwrap_or_default());
        rtk.push_str("Python (requirements.txt):\n");
        rtk.push_str(&summarize_requirements_str(&requirements_path)?);
    }

    let pyproject_path = dir.join("pyproject.toml");
    if pyproject_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&pyproject_path).unwrap_or_default());
        rtk.push_str("Python (pyproject.toml):\n");
        rtk.push_str(&summarize_pyproject_str(&pyproject_path)?);
    }

    let gomod_path = dir.join("go.mod");
    if gomod_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&gomod_path).unwrap_or_default());
        rtk.push_str("Go (go.mod):\n");
        rtk.push_str(&summarize_gomod_str(&gomod_path)?);
    }

    if !found {
        rtk.push_str(&format!("No dependency files found in {}", dir.display()));
    }

    // With no manifest there is no raw text to fall back to: say so.
    let shown = if found { never_worse(&raw, &rtk) } else { &rtk };
    print!("{}", shown);
    timer.track("cat */deps", "rtk deps", &raw, shown);
    Ok(())
}

fn summarize_cargo_str(path: &Path) -> Result<String> {
    let doc = read_toml(path)?;
    // (table, is dev) for the manifest's own tables, each `[target.<cfg>]` table's and
    // `[workspace.dependencies]`, listed in the order the file declares them.
    let mut scopes: Vec<&dyn TableLike> = vec![doc.as_table()];
    if let Some(targets) = doc.get("target").and_then(Item::as_table_like) {
        scopes.extend(targets.iter().filter_map(|(_, t)| t.as_table_like()));
    }
    let mut tables: Vec<(&Item, bool)> = scopes
        .iter()
        .flat_map(|scope| {
            [
                (scope.get("dependencies"), false),
                (scope.get("dev-dependencies"), true),
            ]
        })
        .chain([(
            doc.get("workspace").and_then(|w| w.get("dependencies")),
            false,
        )])
        .filter_map(|(table, dev)| Some((table?, dev)))
        .collect();
    tables.sort_by_key(|(table, _)| {
        let position = table.as_table().and_then(|t| t.position());
        (position.is_none(), position)
    });
    let mut deps = Vec::new();
    let mut dev_deps = Vec::new();
    for (table, dev) in tables {
        cargo_deps(Some(table), if dev { &mut dev_deps } else { &mut deps });
    }

    let mut out = String::new();
    push_section(&mut out, "Dependencies", &deps, MAX_DEPS);
    push_section(&mut out, "Dev", &dev_deps, MAX_DEV_DEPS);
    Ok(out)
}

/// `name (source)` for each entry of a dependency table: its version, else
/// where it comes from (`path`, `git`, Poetry's `url`, `workspace`).
fn cargo_deps(table: Option<&Item>, out: &mut Vec<String>) {
    let Some(table) = table.and_then(Item::as_table_like) else {
        return;
    };
    for (name, spec) in table.iter() {
        let field = |key: &str| spec.get(key).and_then(Item::as_str);
        let source = if let Some(version) = spec.as_str().or_else(|| field("version")) {
            version.to_string()
        } else if let Some(path) = field("path") {
            format!("path {path}")
        } else if let Some(git) = field("git") {
            format!("git {git}")
        } else if let Some(url) = field("url") {
            format!("url {url}")
        } else if spec.get("workspace").and_then(Item::as_bool) == Some(true) {
            "workspace".to_string()
        } else {
            "*".to_string()
        };
        out.push(format!("{name} ({source})"));
    }
}

fn read_toml(path: &Path) -> Result<DocumentMut> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    content
        .parse()
        .with_context(|| format!("Failed to parse {}", path.display()))
}

/// `  <title> (<n>):` and up to `max` items, then a count of the rest.
fn push_section(out: &mut String, title: &str, items: &[String], max: usize) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("  {title} ({}):\n", items.len()));
    for item in items.iter().take(max) {
        out.push_str(&format!("    {item}\n"));
    }
    if items.len() > max {
        out.push_str(&format!("    ... +{} more\n", items.len() - max));
    }
}

fn summarize_package_json_str(path: &Path) -> Result<String> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let json: serde_json::Value = crate::core::utils::from_json_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    let mut out = String::new();

    if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
        let version = json.get("version").and_then(|v| v.as_str()).unwrap_or("?");
        out.push_str(&format!("  {} @ {}\n", name, version));
    }
    if let Some(deps) = json.get("dependencies").and_then(|v| v.as_object()) {
        out.push_str(&format!("  Dependencies ({}):\n", deps.len()));
        for (i, (name, version)) in deps.iter().enumerate() {
            if i >= MAX_DEPS {
                out.push_str(&format!("    ... +{} more\n", deps.len() - MAX_DEPS));
                break;
            }
            out.push_str(&format!(
                "    {} ({})\n",
                name,
                version.as_str().unwrap_or("*")
            ));
        }
    }
    if let Some(dev_deps) = json.get("devDependencies").and_then(|v| v.as_object()) {
        out.push_str(&format!("  Dev Dependencies ({}):\n", dev_deps.len()));
        for (i, (name, _)) in dev_deps.iter().enumerate() {
            if i >= MAX_DEV_DEPS {
                out.push_str(&format!(
                    "    ... +{} more\n",
                    dev_deps.len() - MAX_DEV_DEPS
                ));
                break;
            }
            out.push_str(&format!("    {}\n", name));
        }
    }
    Ok(out)
}

fn summarize_requirements_str(path: &Path) -> Result<String> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let dep_re = Regex::new(r"^([a-zA-Z0-9_-]+)([=<>!~]+.*)?$").unwrap();
    let mut deps = Vec::new();
    let mut out = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(caps) = dep_re.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            deps.push(format!("{}{}", name, version));
        }
    }

    out.push_str(&format!("  Packages ({}):\n", deps.len()));
    for d in deps.iter().take(MAX_DEPS) {
        out.push_str(&format!("    {}\n", d));
    }
    if deps.len() > MAX_DEPS {
        out.push_str(&format!("    ... +{} more\n", deps.len() - MAX_DEPS));
    }
    Ok(out)
}

fn summarize_pyproject_str(path: &Path) -> Result<String> {
    let doc = read_toml(path)?;
    let mut deps = Vec::new();
    let mut optional = Vec::new();
    // PEP 621 and PEP 735.
    let project = doc.get("project");
    pep508_list(project.and_then(|p| p.get("dependencies")), &mut deps);
    for extra in entries(project.and_then(|p| p.get("optional-dependencies")))
        .chain(entries(doc.get("dependency-groups")))
    {
        pep508_list(Some(extra), &mut optional);
    }
    // Poetry.
    let poetry = doc.get("tool").and_then(|t| t.get("poetry"));
    poetry_deps(poetry.and_then(|p| p.get("dependencies")), &mut deps);
    poetry_deps(
        poetry.and_then(|p| p.get("dev-dependencies")),
        &mut optional,
    );
    for group in entries(poetry.and_then(|p| p.get("group"))) {
        poetry_deps(group.get("dependencies"), &mut optional);
    }

    let mut out = String::new();
    push_section(&mut out, "Dependencies", &deps, MAX_DEPS);
    push_section(&mut out, "Dev and optional", &optional, MAX_DEV_DEPS);
    Ok(out)
}

/// The requirement strings of a PEP 508 array; anything else is skipped.
fn pep508_list(list: Option<&Item>, out: &mut Vec<String>) {
    if let Some(list) = list.and_then(Item::as_array) {
        out.extend(list.iter().filter_map(|v| v.as_str()).map(str::to_string));
    }
}

/// A Poetry dependency table, without its `python` constraint.
fn poetry_deps(table: Option<&Item>, out: &mut Vec<String>) {
    let mut found = Vec::new();
    cargo_deps(table, &mut found);
    out.extend(found.into_iter().filter(|dep| !dep.starts_with("python (")));
}

/// The values of a table, or nothing.
fn entries(item: Option<&Item>) -> impl Iterator<Item = &Item> {
    item.and_then(Item::as_table_like)
        .into_iter()
        .flat_map(|t| t.iter().map(|(_, v)| v))
}

fn summarize_gomod_str(path: &Path) -> Result<String> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut module_name = String::new();
    let mut go_version = String::new();
    let mut deps = Vec::new();
    let mut in_require = false;
    let mut out = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("module ") {
            module_name = line.trim_start_matches("module ").to_string();
        } else if line.starts_with("go ") {
            go_version = line.trim_start_matches("go ").to_string();
        } else if line == "require (" {
            in_require = true;
        } else if line == ")" {
            in_require = false;
        } else if in_require && !line.starts_with("//") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                deps.push(format!("{} {}", parts[0], parts[1]));
            }
        } else if line.starts_with("require ") && !line.contains("(") {
            deps.push(line.trim_start_matches("require ").to_string());
        }
    }

    if !module_name.is_empty() {
        out.push_str(&format!("  {} (go {})\n", module_name, go_version));
    }
    push_section(&mut out, "Dependencies", &deps, MAX_DEPS);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE_JSON: &str =
        r#"{"name": "demo", "version": "1.0.0", "dependencies": {"left-pad": "^1.3.0"}}"#;

    fn summarize(content: &str) -> Result<String> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("package.json");
        fs::write(&path, content).expect("write package.json");
        summarize_package_json_str(&path)
    }

    #[test]
    fn test_package_json_plain_parses() {
        let out = summarize(PACKAGE_JSON).expect("plain package.json must parse");
        assert!(out.contains("demo @ 1.0.0"));
        assert!(out.contains("left-pad"));
    }

    #[test]
    fn test_package_json_bom_parses() {
        let bom = format!("\u{feff}{}", PACKAGE_JSON);
        let out = summarize(&bom).expect("BOM-prefixed package.json must parse");
        assert_eq!(out, summarize(PACKAGE_JSON).unwrap());
    }

    #[test]
    fn test_package_json_invalid_has_path_context() {
        let err = summarize("not json").unwrap_err();
        assert!(err.to_string().contains("package.json"));
    }
}
