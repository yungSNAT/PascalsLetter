//! What the output-tokens run and `ab` share: the corpus file, label and token helpers,
//! and when a fact stands whole in a line (the facts check and the `ab` grader).

use serde::Deserialize;

/// How many times `fact` stands whole in `line`: no letter, digit, `_`, `.` or `-` just
/// before it and no letter, digit or `_` just after, so `io.rs` is in neither `old.io.rs`
/// nor `io.rsx`. A short hash may run on into more hex digits, as its full hash does; git
/// abbreviates a hash only as far as it stays unique in the repository.
pub fn copies(line: &str, fact: &str) -> usize {
    matches(line, fact).count()
}

/// The text after each copy of `fact` that [`copies`] counts (after any hex digits a short
/// hash runs on into).
pub fn matches<'a>(line: &'a str, fact: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let short_hash = fact.len() >= 7 && fact.chars().all(|c| c.is_ascii_hexdigit());
    line.match_indices(fact).filter_map(move |(at, _)| {
        let mut after = &line[at + fact.len()..];
        if short_hash {
            after = after.trim_start_matches(|c: char| c.is_ascii_hexdigit());
        }
        (!line[..at].ends_with(|c: char| word(c) || c == '.' || c == '-')
            && !after.starts_with(word))
        .then_some(after)
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corpus {
    pub repo: Vec<Repo>,
    pub command: Vec<CorpusCommand>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repo {
    pub name: String,
    pub url: String,
    /// A full commit hash, so the corpus cannot drift with a moved tag.
    pub rev: String,
    /// Directories, relative to the checkout, put first on PATH.
    #[serde(default)]
    pub path: Vec<String>,
    /// Shell commands run once after checkout.
    #[serde(default)]
    pub setup: Vec<String>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CorpusCommand {
    pub repo: String,
    pub run: String,
    /// Run before each side's run, so every side starts from the files and
    /// build artefacts it resets.
    pub before: Option<String>,
    /// What an agent asks of this output; `keep` names the facts its answer needs.
    pub question: Option<String>,
    pub keep: Option<Keep>,
}

/// The facts an answer needs, all taken from the raw output: listed, or by a rule.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Keep {
    Rule(Rule),
    Facts(Vec<String>),
}

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Rule {
    /// Every line.
    Lines,
    /// Every grep or rg match line's text, after its `path:N:` or `N:`.
    Matches,
    /// Every added or removed line of a patch, with its sign.
    Diff,
    /// The last path component of every line.
    Basenames,
}

pub fn label_value(arg: &str) -> Result<(String, String), String> {
    match arg.split_once('=') {
        Some((label, value)) if !label.trim().is_empty() && !value.is_empty() => {
            Ok((label.trim().to_string(), value.to_string()))
        }
        _ => Err(format!("expected LABEL=VALUE, got '{arg}'")),
    }
}

/// A label as a file name.
pub fn slug(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

pub fn count_tokens(text: &str) -> usize {
    tiktoken_rs::o200k_base_singleton()
        .encode_ordinary(text)
        .len()
}
