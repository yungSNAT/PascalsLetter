# output-tokens

Counts the tokens a coding agent would read from each tool's version of a command's output, over a fixed corpus of commands on pinned public repositories. It measures **command output only**: not billed agent sessions, not answer quality, and not what an agent does with fewer tokens. Billed end-to-end comparisons such as [THOL](https://tokenade.net/en/thol) and TokenBench need paid model runs and are not repeated here.

## Method

- **Corpus:** [`corpus.toml`](corpus.toml): 20 commands (git, grep, rg, find, ls, cat, cargo build, clippy and test, pytest) on [ripgrep](https://github.com/BurntSushi/ripgrep) 14.1.1 and [click](https://github.com/pallets/click) 8.1.7, each pinned to a full commit. Setup adds one failing test to each, so the test runs, `git status` and `git diff` have a failure and a change to report.
- **Sides:** each command runs once per side, from the same state (a corpus `before` command, such as `touch`, runs before every side).
  - `raw`: the command as written.
  - each rtk build: asked `rtk rewrite <command>`, as its hook asks. A rewritten command runs with that build first on `PATH`; a command it leaves alone keeps the raw output.
  - [Tokenade](https://github.com/pi-infected/tokenade): `tokenade wrap '<command>'` for every command.
  - [snip](https://github.com/edouard-claude/snip): `snip <command>` for every command.
  - [Chisle](https://github.com/JayPokale/Chisle): its PostToolUse hook, `hooks/chisle-compress-output.js` at the v3.12.1 commit, given the raw output as a Copilot CLI bash result (the payload that needs no on/off flag file; Claude Code's payload reaches the same compressor). No reply means the raw output stands.
  - `raw, run again`: the raw command once more, after every tool side has run, as a control. Where it differs from `raw`, the command's own output varies (timings) or an earlier side changed the state.
- **What is counted:** stdout and stderr together, as an agent's shell tool returns them, counted with OpenAI's `o200k_base` vocabulary through [tiktoken-rs](https://crates.io/crates/tiktoken-rs), which ships the vocabulary inside the crate, so no network is used. Claude's tokeniser is not published; `o200k_base` stands in for it, so absolute counts differ by model, whilst the ratios between sides are the point.
- **Isolation:** every side runs with a home directory of its own (rtk's history, Tokenade's caches and Chisle's spill files land there), no `XDG_*` directories and `RTK_TELEMETRY_DISABLED=1`. Cargo keeps the user's `CARGO_HOME`.
- **Also reported:** per side, how many commands came out larger than raw and how many exited with a different code from raw (an exit code is part of what the agent reads).

Fewer tokens is not better on its own: a side that drops the line the agent needed scores well here. Read the saved outputs (`outputs/NNN-<side>.txt`) beside the counts.

## Run it

```bash
bench/output-tokens/run.sh ~/output-tokens-work
```

`run.sh` downloads each published release and checks it against its published checksum, clones Chisle at its tag, builds this checkout's rtk, then runs the corpus. Linux x86_64; needs git, curl, cargo, node, uv and rg. Each run writes `outputs/`, `results.csv` and `summary.md` to a new `run-<UTC time>` folder.

To measure other builds, run the binary directly:

```bash
cargo run --release --manifest-path bench/output-tokens/Cargo.toml -- \
  --corpus bench/output-tokens/corpus.toml --work /tmp/ot-repos --out /tmp/ot-run \
  --rtk "mine=$(command -v rtk)" --wrap "other tool=other-tool run {qcmd}"
```

`{cmd}` puts the command into a template as written; `{qcmd}` puts it in as one quoted shell word.
