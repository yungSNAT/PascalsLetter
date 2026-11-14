//! Filters environment variables, compacting noise.

use crate::core::guard::never_worse;
use crate::core::tracking;
use crate::core::truncate::{CAP_LIST, CAP_WARNINGS};
use anyhow::Result;
use std::env;
use std::fmt::Write;

/// Show filtered environment variables
pub fn run(filter: Option<&str>, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Environment variables:");
    }

    let mut vars: Vec<(String, String)> = env::vars().collect();
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    let body = render(&vars, filter);
    let raw = raw_lines(&vars, filter);
    let shown_body = never_worse(&raw, &body);
    print!("{}", shown_body);
    timer.track("env", "rtk env", &raw, shown_body);
    Ok(())
}

/// `vars` as `env` prints them, limited to those `filter` names: what the
/// listing replaces, so what it must not exceed.
fn raw_lines(vars: &[(String, String)], filter: Option<&str>) -> String {
    vars.iter()
        .filter(|(key, _)| matches_filter(key, filter))
        .fold(String::new(), |mut output, (k, v)| {
            let _ = writeln!(output, "{}={}", k, v);
            output
        })
}

fn matches_filter(key: &str, filter: Option<&str>) -> bool {
    filter.is_none_or(|f| key.to_lowercase().contains(&f.to_lowercase()))
}

/// The filtered listing of `vars`, sorted by name.
fn render(vars: &[(String, String)], filter: Option<&str>) -> String {
    // Interesting categories
    let mut path_vars = Vec::new();
    let mut lang_vars = Vec::new();
    let mut cloud_vars = Vec::new();
    let mut tool_vars = Vec::new();
    let mut other_vars = Vec::new();

    for (key, value) in vars {
        if !matches_filter(key, filter) {
            continue;
        }

        // PATH is split into entries before anything is shortened.
        let display_value = if is_path_key(key) {
            value.clone()
        } else {
            shorten(value)
        };

        let entry = (key.clone(), display_value);

        // Categorize
        if is_path_var(key) {
            path_vars.push(entry);
        } else if is_lang_var(key) {
            lang_vars.push(entry);
        } else if is_cloud_var(key) {
            cloud_vars.push(entry);
        } else if is_tool_var(key) {
            tool_vars.push(entry);
        } else if filter.is_some() || is_interesting_var(key) {
            other_vars.push(entry);
        }
    }

    let mut body = String::new();
    if !path_vars.is_empty() {
        let _ = writeln!(body, "PATH Variables:");
        for (k, v) in &path_vars {
            if is_path_key(k) {
                write_path(&mut body, v);
            } else {
                let _ = writeln!(body, "  {}={}", k, v);
            }
        }
    }

    if !lang_vars.is_empty() {
        let _ = writeln!(body, "\nLanguage/Runtime:");
        for (k, v) in &lang_vars {
            let _ = writeln!(body, "  {}={}", k, v);
        }
    }

    if !cloud_vars.is_empty() {
        let _ = writeln!(body, "\nCloud/Services:");
        for (k, v) in &cloud_vars {
            let _ = writeln!(body, "  {}={}", k, v);
        }
    }

    if !tool_vars.is_empty() {
        let _ = writeln!(body, "\nTools:");
        for (k, v) in &tool_vars {
            let _ = writeln!(body, "  {}={}", k, v);
        }
    }

    if !other_vars.is_empty() {
        const MAX_OTHER_VARS: usize = CAP_LIST;
        let _ = writeln!(body, "\nOther:");
        for (k, v) in other_vars.iter().take(MAX_OTHER_VARS) {
            let _ = writeln!(body, "  {}={}", k, v);
        }
        if other_vars.len() > MAX_OTHER_VARS {
            let _ = writeln!(body, "  ... +{} more", other_vars.len() - MAX_OTHER_VARS);
        }
    }

    let total = vars.len();
    let shown = path_vars.len()
        + lang_vars.len()
        + cloud_vars.len()
        + tool_vars.len()
        + other_vars.len().min(20);
    if filter.is_none() {
        let _ = writeln!(body, "\nTotal: {} vars (showing {} relevant)", total, shown);
    }
    body
}

/// Whether `key` names the search path: `PATH`, or on Windows, where names
/// ignore case and the variable is stored as `Path`, any spelling of it.
fn is_path_key(key: &str) -> bool {
    if cfg!(windows) {
        key.eq_ignore_ascii_case("PATH")
    } else {
        key == "PATH"
    }
}

/// Whether `key` belongs under 'PATH Variables': its name holds `PATH`, in any
/// case on Windows only, since Unix names are case-sensitive.
fn is_path_var(key: &str) -> bool {
    if cfg!(windows) {
        key.to_ascii_uppercase().contains("PATH")
    } else {
        key.contains("PATH")
    }
}

/// A value over 100 bytes, cut to its first 50 characters and its length.
fn shorten(value: &str) -> String {
    if value.len() > 100 {
        let preview: String = value.chars().take(50).collect();
        format!("{}... ({} chars)", preview, value.chars().count())
    } else {
        value.to_string()
    }
}

/// PATH one entry per line, split on the platform's separator (`:`, or `;` on
/// Windows, whose entries hold drive colons).
fn write_path(body: &mut String, value: &str) {
    let paths: Vec<_> = env::split_paths(value).collect();
    let _ = writeln!(body, "  PATH ({} entries):", paths.len());
    const MAX_PATH_ENTRIES: usize = CAP_WARNINGS;
    for p in paths.iter().take(MAX_PATH_ENTRIES) {
        let _ = writeln!(body, "    {}", shorten(&p.to_string_lossy()));
    }
    if paths.len() > MAX_PATH_ENTRIES {
        let _ = writeln!(body, "    ... +{} more", paths.len() - MAX_PATH_ENTRIES);
    }
}

fn is_lang_var(key: &str) -> bool {
    let patterns = [
        "RUST", "CARGO", "PYTHON", "PIP", "NODE", "NPM", "YARN", "DENO", "BUN", "JAVA", "MAVEN",
        "GRADLE", "GO", "GOPATH", "GOROOT", "RUBY", "GEM", "PERL", "PHP", "DOTNET", "NUGET",
    ];
    patterns.iter().any(|p| key.to_uppercase().contains(p))
}

fn is_cloud_var(key: &str) -> bool {
    let patterns = [
        "AWS",
        "AZURE",
        "GCP",
        "GOOGLE_CLOUD",
        "DOCKER",
        "KUBERNETES",
        "K8S",
        "HELM",
        "TERRAFORM",
        "VAULT",
        "CONSUL",
        "NOMAD",
    ];
    patterns.iter().any(|p| key.to_uppercase().contains(p))
}

fn is_tool_var(key: &str) -> bool {
    let patterns = [
        "EDITOR",
        "VISUAL",
        "SHELL",
        "TERM",
        "GIT",
        "SSH",
        "GPG",
        "BREW",
        "HOMEBREW",
        "XDG",
        "CLAUDE",
        "ANTHROPIC",
    ];
    patterns.iter().any(|p| key.to_uppercase().contains(p))
}

fn is_interesting_var(key: &str) -> bool {
    let patterns = ["HOME", "USER", "LANG", "LC_", "TZ", "PWD", "OLDPWD"];
    patterns.iter().any(|p| key.to_uppercase().starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_lang_var_rust() {
        assert!(is_lang_var("RUST_LOG"));
        assert!(is_lang_var("CARGO_HOME"));
        assert!(is_lang_var("GOPATH"));
        assert!(is_lang_var("NODE_ENV"));
    }

    #[test]
    fn test_is_lang_var_negative() {
        assert!(!is_lang_var("HOME"));
        assert!(!is_lang_var("PATH"));
        assert!(!is_lang_var("USER"));
    }

    #[test]
    fn test_is_cloud_var() {
        assert!(is_cloud_var("AWS_ACCESS_KEY_ID"));
        assert!(is_cloud_var("AZURE_CLIENT_ID"));
        assert!(is_cloud_var("DOCKER_HOST"));
        assert!(is_cloud_var("KUBERNETES_SERVICE_HOST"));
    }

    #[test]
    fn test_is_cloud_var_negative() {
        assert!(!is_cloud_var("HOME"));
        assert!(!is_cloud_var("RUST_LOG"));
    }

    #[test]
    fn test_is_tool_var() {
        assert!(is_tool_var("EDITOR"));
        assert!(is_tool_var("GIT_AUTHOR_NAME"));
        assert!(is_tool_var("SSH_AUTH_SOCK"));
        assert!(is_tool_var("CLAUDE_API_KEY"));
    }

    #[test]
    fn test_is_interesting_var() {
        assert!(is_interesting_var("HOME"));
        assert!(is_interesting_var("USER"));
        assert!(is_interesting_var("LANG"));
        assert!(is_interesting_var("TZ"));
        assert!(is_interesting_var("PWD"));
    }

    #[test]
    fn test_is_interesting_var_negative() {
        assert!(!is_interesting_var("RANDOM_VAR"));
        assert!(!is_interesting_var("MY_CUSTOM_VAR"));
    }

    #[test]
    fn a_long_path_is_split_before_it_is_shortened() {
        let entries: Vec<String> = (0..8).map(|i| format!("/opt/tool-{i}/bin")).collect();
        let path = env::join_paths(&entries).unwrap().into_string().unwrap();
        assert!(path.len() > 100, "the value must be long enough to be cut");
        let body = render(&[("PATH".to_string(), path)], Some("PATH"));
        assert!(body.contains("PATH (8 entries):"), "{body}");
        for entry in &entries {
            assert!(body.contains(&format!("    {entry}\n")), "{entry}: {body}");
        }
    }

    /// `rtk env -f PATH` is held to what `env | grep PATH` prints, not to the
    /// whole environment, which a listing of every PATH entry can exceed.
    #[test]
    fn a_filtered_listing_is_held_to_the_variables_it_names() {
        let vars = vec![
            ("HOME".to_string(), "/home/me".to_string()),
            ("PATH".to_string(), "/a:/b".to_string()),
        ];
        let raw = raw_lines(&vars, Some("PATH"));
        assert_eq!(raw, "PATH=/a:/b\n");
        assert_eq!(never_worse(&raw, &render(&vars, Some("PATH"))), raw);
        assert_eq!(raw_lines(&vars, None), "HOME=/home/me\nPATH=/a:/b\n");
    }

    /// Unix names are case-sensitive, so a lower-case `path` keeps its category.
    #[cfg(not(windows))]
    #[test]
    fn a_lower_case_path_name_is_not_a_path_variable_on_unix() {
        let vars = vec![(
            "npm_execpath".to_string(),
            "/usr/lib/node_modules/npm/bin/npm-cli.js".to_string(),
        )];
        let body = render(&vars, None);
        assert!(
            body.contains("Language/Runtime:\n  npm_execpath="),
            "{body}"
        );
        assert!(!body.contains("PATH Variables:"), "{body}");
    }

    /// Windows stores the variable as `Path`, and its entries hold drive colons.
    #[cfg(windows)]
    #[test]
    fn a_windows_path_keeps_its_drive_letters() {
        let entries: Vec<String> = (0..5)
            .map(|i| format!(r"C:\Program Files\Tool{i}\bin"))
            .collect();
        let path = entries.join(";");
        assert!(path.len() > 100, "the value must be long enough to be cut");
        let body = render(&[("Path".to_string(), path)], None);
        assert!(body.contains("PATH (5 entries):"), "{body}");
        for entry in &entries {
            assert!(body.contains(&format!("    {entry}\n")), "{entry}: {body}");
        }
    }
}
