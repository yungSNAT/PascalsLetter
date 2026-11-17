# output-tokens

Counts the tokens a coding agent would read from each tool's version of a command's output, over a fixed corpus of commands on pinned public repositories. It measures **command output only**: not billed agent sessions, not answer quality, and not what an agent does with fewer tokens. [`ab`](#model-answers-ab) asks a model the questions over the same commands, and [`../end-to-end`](../end-to-end) runs billed coding sessions.

Published runs: [`results/`](results/).

## Method

- **Corpus:** [`corpus.toml`](corpus.toml): 20 commands (git, grep, rg, find, ls, cat, cargo build, clippy and test, pytest) on [ripgrep](https://github.com/BurntSushi/ripgrep) 14.1.1 and [click](https://github.com/pallets/click) 8.1.7, each pinned to a full commit. Setup adds one failing test to each, so the test runs, `git status` and `git diff` have a failure and a change to report.
- **Sides:** each command runs once per side, in the same clone of each repository; a corpus `before` command, such as `touch` or a build, runs before every side, so each side starts from the same files and build artefacts as far as the corpus resets them.
  - `raw`: the command as written.
  - each rtk build: asked `rtk rewrite <command>`, as its hook asks. A rewritten command runs with that build first on `PATH`; a command it leaves alone keeps the raw output.
  - [Tokenade](https://github.com/pi-infected/tokenade): `tokenade wrap '<command>'` for every command.
  - [snip](https://github.com/edouard-claude/snip): `snip <command>` for every command.
  - [Chisle](https://github.com/JayPokale/Chisle): its PostToolUse hook, `hooks/chisle-compress-output.js` at the v3.12.1 commit, given the raw output as a Copilot CLI bash result (the payload that needs no on/off flag file; Claude Code's payload reaches the same compressor). No reply means the raw output stands.
  - Claude Code plugins ([context-mode](https://github.com/mksglu/context-mode) and [token-optimizer-mcp](https://github.com/ooples/token-optimizer-mcp)), installed from npm at fixed versions without install scripts: the hooks file the plugin's `.claude-plugin/plugin.json` names (else `hooks/hooks.json`), run as Claude Code runs it for a Bash call. Its SessionStart hooks run once before the corpus. For each command, its PreToolUse hooks may refuse the command (the agent reads the reason instead), rewrite it (the new command runs, after `before`) or leave it (the raw output stands). A command that exits 0 then goes to its PostToolUse hooks, which may replace the output; any other goes to its PostToolUseFailure hooks, which can only add to it, as Claude Code treats a non-zero exit as a failed call (it lets exit 1 pass from grep, rg, find, diff and test; no corpus command exits so). Context any hook adds is counted with the output; what SessionStart adds is reported once and not counted. A hook still running at its timeout is killed and its reply dropped. Each hook's reply is in `hooks/<side>.jsonl`.
  - `raw, run again`: the raw command once more, after every side that runs commands, as a control (Chisle, which only reads the first raw capture, and the plugins, which run only commands their hooks rewrite, come after it). Where it differs from `raw`, the command's own output varies (timings) or an earlier side changed the state.
- **What is counted:** stdout and stderr together, as an agent's shell tool returns them, counted with OpenAI's `o200k_base` vocabulary through [tiktoken-rs](https://crates.io/crates/tiktoken-rs), which ships the vocabulary inside the crate, so no network is used. Claude's tokeniser is not published; `o200k_base` stands in for it, so absolute counts differ by model, whilst the ratios between sides are the point.
- **Isolation:** every side runs with a home directory and a `TMPDIR` of its own in the run's folder (rtk's history, Tokenade's caches, Chisle's spill files and the plugins' data and state land there), no `XDG_*` directories and `RTK_TELEMETRY_DISABLED=1`. Cargo keeps the user's `CARGO_HOME`. The clones and their build artefacts are shared, so what one side leaves there reaches the next unless a `before` command resets it.
- **Facts kept:** a command with a `question` names facts its answer needs in `keep`, a chosen set rather than all an answer could use: a list of strings, or a rule over the raw output (`lines`; `matches`, each grep or rg match's text after its `path:N:`, without its location; `diff`, each added or removed line; `basenames`, each file's name without its directory). A rule takes a fact once for each time it finds it, and drops lines with no letter or digit. Every fact must be in the raw output. An output keeps a fact when one of its lines holds it whole: no letter, digit, `_`, `.` or `-` just before it and no letter, digit or `_` just after it, so `io.rs` is in none of `radio.rs`, `old.io.rs` and `io.rsx`, though a short hash may run on into more hex digits, as its full hash does. Whitespace aside, it must be held as many times as the raw output has it. `facts.md` gives each side's share, which commands lost a fact, whether that output names a way to recover the rest (`rtk recall`, a `[full output: ...]` path, `tokenade expand-ref`, snip's `Full patch:`; recognised by text, not tried) and the first facts lost. No model is asked, so a fact a side reworded counts as lost: read the lost facts before calling a loss a defect.
- **Also reported:** per side, how many commands came out larger than raw and how many exited with a different code from raw (an exit code is part of what the agent reads).

Fewer tokens is not better on its own: a side that drops the line the agent needed scores well on tokens and badly on facts. Read the saved outputs (`outputs/NNN-<side>.txt`) beside both.

## Run it

```bash
cargo run --release --locked --manifest-path bench/output-tokens/Cargo.toml --bin published -- ~/output-tokens-work
```

[`published`](src/bin/published.rs) downloads each published release and checks it against its published checksum, clones Chisle at its tag, installs the plugins from npm, builds this checkout's rtk, then runs the corpus. Linux x86_64; needs git, curl, tar, sha256sum, cargo, node, npm, uv and rg. Each run writes `outputs/`, `results.csv`, `summary.md`, `facts.md` and `toolchain.txt` (the tool versions) to a new `run-<UTC time>` folder.

To measure other builds, run the binary directly:

```bash
cargo run --release --manifest-path bench/output-tokens/Cargo.toml -- \
  --corpus bench/output-tokens/corpus.toml --work /tmp/ot-repos --out /tmp/ot-run \
  --rtk "mine=$(command -v rtk)" --wrap "other tool=other-tool run {qcmd}"
```

`{cmd}` puts the command into a template as written; `{qcmd}` puts it in as one quoted shell word.

## Model answers (`ab`)

[`ab`](src/bin/ab.rs) runs dtk's A/B test (`scripts/ab-test.sh` in [amaye15/dtk](https://github.com/amaye15/dtk)) over this corpus: for each case in [`ab-cases.toml`](ab-cases.toml), a headless Claude Code session gets 'Run this bash command: `<command>`   Then answer in one short line: `<question>`', with Bash as its only tool. A reply is right when it holds every word of the case's answer, case aside, each word whole by the fact rule above, so `250` is not in `1250`. No model grades the replies.

- **Cases:** 11 corpus commands. 8 list commands ask for something read from their raw output; 3 ask for a count over a list rtk caps (`grep -rn 'fn new'`, `rg -n 'unwrap\(\)'` and `find crates -name '*.rs'`), so the answer needs what the cap leaves out.
- **Sides:** `raw`, with no hook, and each `--rtk` build behind rtk's Claude Code hook (`rtk hook claude` in the session's settings, the build first on `PATH`).
- **Sessions:** each has its own home and Claude Code configuration folder in the run's folder, loads only those settings (`--setting-sources user`) and inherits no `CLAUDE*` or `XDG_*` variables. The corpus `before` command runs first, and the side order rotates each run. `--budget-usd` starts no session once the sessions so far cost that much. A session that ends without Claude Code's `result` event (a timeout, or a crash), or with one that gives no cost, has no cost to count, so the plan stops after it and its cost is left blank.
- **Recorded:** the reply and whether it is right, turns, the Bash commands, the tool results' characters and `o200k_base` tokens, the API usage and Claude Code's `total_cost_usd`, in `results.csv`, `summary.md` and each session's transcript.

```bash
ANTHROPIC_API_KEY=... cargo run --release --locked --manifest-path bench/output-tokens/Cargo.toml --bin ab -- \
  --corpus bench/output-tokens/corpus.toml --cases bench/output-tokens/ab-cases.toml \
  --repos ~/output-tokens-work/repos --out ~/ab-run --claude "$(command -v claude)" \
  --model claude-haiku-5-5 --runs 5 --budget-usd 2 \
  --rtk "rtk 1.2.0=$HOME/output-tokens-work/tools/rtk/rtk" \
  --rtk "PascalsLetter=$HOME/output-tokens-work/target-rtk/release/rtk"
```

It needs the clones a `published` run sets up in `<work>/repos`.
