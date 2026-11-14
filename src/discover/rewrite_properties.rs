// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: Apache-2.0

//! Property checks for `rewrite_command`. A rewrite may change the words of a
//! command but never its shell structure: every operator, pipe, redirect and
//! shellism the lexer sees in the original must appear, in order, in the
//! rewrite; otherwise a hook would run something other than what the agent
//! asked for. And a command the hook rewrites never gets Allow while a deny
//! rule names one of the commands in it, or the hook would approve that command;
//! OpenCode, which applies its own rules, never gets a rewrite that lifts one.

use super::lexer::{TokenKind, gate_segments, tokenize};
use super::registry::rewrite_command;
use crate::hooks::hook_cmd::opencode_answer;
use crate::hooks::permissions::{PermissionVerdict, check_command_with_rules};
use crate::hooks::permissions_opencode::{Action, Rule, evaluate};

/// Commands RTK rewrites, commands it leaves alone, and words that hold
/// operator characters inside quotes.
const COMMANDS: &[&str] = &[
    "git status",
    "git log --oneline -5",
    "git diff HEAD~1",
    "cargo test",
    "cargo build --release",
    "ls -la",
    "grep -rn foo src",
    "grep \"a && b\" notes.txt",
    "rg 'x | y' src",
    "cat README.md",
    "head -n 5 file.txt",
    "tail -20 app.log",
    "find . -name '*.rs'",
    "npm test",
    "pytest -q",
    "docker ps",
    "echo \"done; ok\"",
    "git commit -m \"fix: a || b\"",
    "FOO=1 cargo check",
    "rtk git status",
    "sleep 1",
    "wc -l src/main.rs",
];

const REDIRECTS: &[&str] = &[
    "",
    "",
    "",
    " 2>&1",
    " > out.txt",
    " 2>/dev/null",
    " >> log.txt",
    " < in.txt",
    " &> all.log",
];

/// `&!` and `&|` are one operator to zsh (and `&|` to fish) but two tokens to
/// the lexer; glued or spaced, a rewrite must keep both.
const OPERATORS: &[&str] = &[
    " && ", " || ", " ; ", " | ", " |& ", " & ", " &! ", " &!", " &| ", " &|",
];

/// The non-word tokens of `cmd`, in order.
fn structure(cmd: &str) -> Vec<(TokenKind, String)> {
    tokenize(cmd)
        .into_iter()
        .filter(|t| t.kind != TokenKind::Arg)
        .map(|t| (t.kind, t.value))
        .collect()
}

/// SplitMix64: a fixed seed makes every run check the same commands.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

fn segment(rng: &mut Rng) -> String {
    format!("{}{}", rng.pick(COMMANDS), rng.pick(REDIRECTS))
}

#[test]
fn rewrites_keep_every_operator_and_redirect() {
    let mut rng = Rng(0x5EED_2026_1103);
    let mut rewritten = 0;
    let mut failures = Vec::new();
    for _ in 0..20_000 {
        let mut cmd = segment(&mut rng);
        for _ in 0..rng.next() % 4 {
            cmd.push_str(rng.pick(OPERATORS));
            cmd.push_str(&segment(&mut rng));
        }
        let Some(out) = rewrite_command(&cmd, &[], &[]) else {
            continue;
        };
        if out != cmd {
            rewritten += 1;
        }
        if structure(&out) != structure(&cmd) {
            failures.push(format!("{cmd:?}\n  -> {out:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} rewrites changed the shell structure; first 10:\n{}",
        failures.len(),
        failures[..failures.len().min(10)].join("\n")
    );
    // Guard against a generator that stops reaching the rewriter.
    assert!(
        rewritten > 5_000,
        "only {rewritten} of 20000 commands were rewritten"
    );
}

/// What a command runs behind: assignments and the wrappers a deny rule reads
/// past, grammar that opens a command, and a leading redirect.
const PREFIXES: &[&str] = &[
    "",
    "",
    "",
    "HUSKY=0 ",
    "timeout 30 ",
    "env FOO=1 ",
    "nohup ",
    "command ",
    "time ",
    "! ",
    "2>/dev/null ",
];

/// Where a command sits: as written, in a group, in a subshell or in the
/// script of a shell wrapper. `%` stands for the command.
const PLACES: &[&str] = &["%", "%", "%", "{ %; }", "( % )", "bash -c '%'", "sh -c '%'"];

/// The two words a deny rule names a command by, past its assignments; `None`
/// when one of them is quoted text.
fn rule_name(cmd: &str) -> Option<String> {
    let words: Vec<&str> = cmd
        .split_whitespace()
        .skip_while(|word| word.contains('='))
        .take(2)
        .collect();
    (words.len() == 2 && !words.iter().any(|word| word.contains(['"', '\''])))
        .then(|| words.join(" "))
}

/// The prefixes and places OpenCode still reads as the command itself: it
/// matches a rule against a command's whole text, so behind `env` or inside
/// `bash -c '…'` a rule naming the command does not match there either.
const OPENCODE_PREFIXES: &[&str] = &["", "", "! "];
const OPENCODE_PLACES: &[&str] = &["%", "%", "{ %; }", "( % )"];

/// The first two words of a command, by which an OpenCode rule names it;
/// `None` when one of them is quoted text.
fn opencode_rule_name(cmd: &str) -> Option<String> {
    let words: Vec<&str> = cmd.split_whitespace().take(2).collect();
    (words.len() == 2 && !words.iter().any(|word| word.contains(['"', '\''])))
        .then(|| words.join(" "))
}

/// A random chain of commands from `prefixes` and `places`, with the name
/// `name` gives each command for a deny rule.
fn named_chain(
    rng: &mut Rng,
    prefixes: &[&str],
    places: &[&str],
    name: fn(&str) -> Option<String>,
) -> (String, Vec<String>) {
    let mut segments = vec![named_segment(rng, prefixes, places, name)];
    for _ in 0..rng.next() % 4 {
        segments.push(named_segment(rng, prefixes, places, name));
    }
    let mut cmd = segments[0].0.clone();
    for (text, _) in &segments[1..] {
        cmd.push_str(rng.pick(OPERATORS));
        cmd.push_str(text);
    }
    if rng.next().is_multiple_of(5) {
        cmd.push_str(" # note; ok");
    }
    (
        cmd,
        segments.into_iter().filter_map(|(_, name)| name).collect(),
    )
}

/// One command in a random prefix, place and line continuation, with the
/// deny rule that names it.
fn named_segment(
    rng: &mut Rng,
    prefixes: &[&str],
    places: &[&str],
    name: fn(&str) -> Option<String>,
) -> (String, Option<String>) {
    let command = rng.pick(COMMANDS);
    let mut text = format!("{}{}", rng.pick(prefixes), command);
    // A line continuation inside the command, which the shell joins.
    if rng.next().is_multiple_of(6) {
        text = text.replacen(' ', " \\\n  ", 1);
    }
    text.push_str(rng.pick(REDIRECTS));
    let place = rng.pick(places);
    // A single-quoted script cannot hold a single quote.
    let text = if place.contains('\'') && text.contains('\'') {
        text
    } else {
        place.replace('%', &text)
    };
    (text, name(command))
}

#[test]
fn a_rewritten_command_never_gets_allow_under_a_deny_rule_naming_it() {
    let mut rng = Rng(0x5EED_2026_1114);
    let allow = ["*".to_string()];
    let mut checked = 0;
    let mut failures = Vec::new();
    for _ in 0..10_000 {
        let (cmd, names) = named_chain(&mut rng, PREFIXES, PLACES, rule_name);
        if rewrite_command(&cmd, &[], &[]).is_none_or(|out| out == cmd) {
            continue;
        }
        for name in &names {
            checked += 1;
            let deny = [format!("{name}:*")];
            if check_command_with_rules(&cmd, &deny, &[], &allow) == PermissionVerdict::Allow {
                failures.push(format!("deny {name:?} allowed {cmd:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} rewritten commands got Allow under a deny rule naming one of their commands; \
         first 10:\n{}",
        failures.len(),
        failures[..failures.len().min(10)].join("\n")
    );
    // Guard against a generator that stops reaching the gate.
    assert!(
        checked > 5_000,
        "only {checked} deny rules were checked against a rewritten command"
    );
}

/// Whether OpenCode's own rules deny `cmd`, as OpenCode reads it: each command
/// on its own, including one inside a group, subshell or negation, matched
/// against its text as written.
fn opencode_denies(cmd: &str, rules: &[Rule]) -> bool {
    gate_segments(cmd).iter().any(|segment| {
        let bare = segment.trim_start_matches(['{', '(', '!', ' ']);
        [*segment, bare]
            .iter()
            .any(|text| evaluate(text, rules) == Some(Action::Deny))
    })
}

/// OpenCode applies its own rules to the command the hook hands back, so a
/// rewrite that un-matched a deny rule would lift it. For every command an
/// OpenCode deny rule names, the hook hands back nothing or a command the rule
/// still denies.
#[test]
fn opencode_still_denies_every_rewrite_the_hook_hands_back() {
    let mut rng = Rng(0x5EED_2026_1115);
    let mut checked = 0;
    let mut failures = Vec::new();
    for _ in 0..10_000 {
        let (cmd, names) = named_chain(
            &mut rng,
            OPENCODE_PREFIXES,
            OPENCODE_PLACES,
            opencode_rule_name,
        );
        for name in &names {
            let rules = [
                opencode_rule("*", Action::Allow),
                opencode_rule(&format!("{name} *"), Action::Deny),
            ];
            if !opencode_denies(&cmd, &rules) {
                continue;
            }
            let answer = opencode_answer(&cmd, &rules);
            let Some(rewritten) = answer.get("command").and_then(|c| c.as_str()) else {
                continue;
            };
            checked += 1;
            if !opencode_denies(rewritten, &rules) {
                failures.push(format!("deny {name:?} lifted: {cmd:?} -> {rewritten:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} rewrites the hook hands OpenCode lift a deny rule the typed command matched; \
         first 10:\n{}",
        failures.len(),
        failures[..failures.len().min(10)].join("\n")
    );
    // Guard against a generator that stops reaching a rewrite: the hook hands
    // back none for most denied commands, and 123 of these chains at this seed.
    assert!(
        checked > 60,
        "only {checked} rewrites of a denied command were checked"
    );
}

fn opencode_rule(pattern: &str, action: Action) -> Rule {
    Rule {
        permission: "bash".to_string(),
        pattern: pattern.to_string(),
        action,
    }
}
