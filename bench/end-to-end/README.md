# end-to-end

Billed Claude Code sessions on coding tasks: no tool, then rtk's release and PascalsLetter, each behind rtk's own Claude Code hook. It measures what a task costs at the API's prices and whether the task was done, which [`output-tokens`](../output-tokens) does not. It runs [THOL](https://github.com/pi-infected/token-harness-optimizer-leaderboard) (token-harness-optimizer-leaderboard, MIT), unchanged apart from 2 competitor manifests.

Published runs: [`results/`](results/), the first on [17 November 2026](results/2026-11-17.md).

## Method

- **Harness:** THOL at commit `5ee6ea7f81dc21db1aae8d359cda3e1f7ded6dde`. Each run is one headless Claude Code session in a fresh sandbox (its own `HOME`, `CLAUDE_CONFIG_DIR` and task workspace), with the Claude Code version and model THOL pins: Claude Code 2.1.206 and `claude-sonnet-4-6` (`bench.config.json`). The task's `verify.py` scores the workspace afterwards; no model grades anything. A run succeeds when THOL's success rule says so.
- **Arms:**
  - `control`: Claude Code alone.
  - `rtk-1.2.0`: rtk's release v1.2.0 (`rtk-x86_64-unknown-linux-musl`, checked against the release's `checksums.txt`).
  - `pascalsletter`: a release build of this repository's `develop` at the commit the results file names (full edition).

  Both rtk arms use THOL's own rtk manifest ([`thol/`](thol/)): `rtk init -g` in the sandbox's home, then rtk's PreToolUse Bash hook (`rtk hook claude`). They differ only in which `rtk` is first on `PATH`.
- **Tasks:** 8 of THOL's 18, chosen before the run from THOL's published control rows: no web access, and a control cost coefficient of variation of at most 0.20, so 10 repetitions can separate a difference of about 10%. They are `code-bugfix-py`, `code-debug-pipeline-py`, `code-feature-js`, `code-iterate-tests`, `code-migration-py-xl`, `code-refactor-split-py`, `log-needle-zh` and `seo-audit`. A result covers these 8 only, not THOL's long sessions or its web task.
- **Repetitions:** 10 per arm and task. THOL's runner interleaves the arms within each task and repetition.
- **Cost:** each run's `total_cost_usd` from Claude Code, which matches the API's per-request usage priced at the model's list prices (fresh input, 5-minute cache writes, cache reads and output).
- **Statistic:** THOL's `leaderboard.py`: per task, the mean cost of an arm's successful runs over control's, then the geometric mean over tasks, with a 95% bootstrap interval (10,000 draws, seed 1234) that resamples tasks and the runs within each. An interval that excludes 1.00 differs from control. PascalsLetter against rtk is the same script on a copy of the database with rtk's rows standing as control.
- **Hook use:** THOL's `competitor_tool_calls` counts Bash commands the model itself writes with `rtk `. The hook rewrites a command after the model writes it, so the count misses every rewrite; it is 0 in all 384 rtk rows of THOL's published database. Each run's own rtk history in the sandbox (`~/.local/share/rtk/history.db`) is read instead: `hook_decisions` holds every Bash call the hook saw, and `commands` every one that ran through rtk, with rtk's estimate of the tokens before and after.

## Run it

Linux x86_64, with an Anthropic API key. The 17 November run of the 8 tasks cost US$49.02 at the list prices; `--budget-usd` starts no run once the runs so far cost that much.

```bash
# The 2 rtk binaries: rtk 1.2.0 checked against its checksums, and this checkout built, so
# check out the commit to measure first (578b0c0e for the 17 November results) and set
# version_pin in thol/pascalsletter.json to match.
git checkout --detach 578b0c0eb28e29c6b1e085b5f039395aed470eb8
cargo run --release --locked --manifest-path bench/output-tokens/Cargo.toml --bin published -- ~/ot-work
mkdir -p ~/arms/rtk-1.2.0 ~/arms/pascalsletter
ln -s ~/ot-work/tools/rtk/rtk ~/arms/rtk-1.2.0/rtk
ln -s ~/ot-work/target-rtk/release/rtk ~/arms/pascalsletter/rtk

# THOL at its commit, with its fixtures. Its own database moves aside, or the runner would
# count its published control runs as done.
git clone https://github.com/pi-infected/token-harness-optimizer-leaderboard.git ~/thol
cd ~/thol && git checkout 5ee6ea7f81dc21db1aae8d359cda3e1f7ded6dde
mv results.sqlite results.published.sqlite
python3 fixtures/generate_fixtures.py && python3 fixtures/gen_longtasks.py && python3 fixtures/gen_megatasks.py

# Claude Code 2.1.206 where THOL looks for it.
npm install --prefix ~/cc206 @anthropic-ai/claude-code@2.1.206
mkdir -p .claude-pin && ln -s ~/cc206/node_modules/.bin/claude .claude-pin/claude

# The 2 arms: each manifest with that arm's folder first on PATH.
for arm in rtk-1.2.0 pascalsletter; do
  mkdir -p competitors/$arm
  sed -e "s|@RTK_DIR@|$HOME/arms/$arm|" -e "s|@PIN@|$PWD/.claude-pin|" -e "s|@PATH@|$PATH|" \
    /path/to/PascalsLetter/bench/end-to-end/thol/$arm.json > competitors/$arm/manifest.json
done

# The runs. An empty HOME keeps your own Claude Code login out of the sandboxes.
mkdir -p ~/empty-home
HOME=~/empty-home THOL_CAMPAIGN=2.1.206 ANTHROPIC_API_KEY=... python3 runner.py run \
  -c control,rtk-1.2.0,pascalsletter \
  -t code-bugfix-py,code-debug-pipeline-py,code-feature-js,code-iterate-tests,code-migration-py-xl,code-refactor-split-py,log-needle-zh,seo-audit \
  --reps 10 --keep-workspaces --budget-usd 55

# The statistic, against control and against rtk.
THOL_CAMPAIGN=2.1.206 python3 leaderboard.py
mkdir ../thol-vs-rtk && cp -r leaderboard.py CAMPAIGN tasks results.sqlite ../thol-vs-rtk/
sqlite3 ../thol-vs-rtk/results.sqlite "delete from runs where competitor = 'control'; update runs set competitor = 'control' where competitor = 'rtk-1.2.0'"
(cd ../thol-vs-rtk && THOL_CAMPAIGN=2.1.206 python3 leaderboard.py)

# Hook use in one kept run (runs_root in bench.config.json).
sqlite3 <run>/home/.local/share/rtk/history.db "select decision, count(*) from hook_decisions group by decision"
```

If `npm install` leaves Claude Code without its native binary, run `node install.cjs` in `~/cc206/node_modules/@anthropic-ai/claude-code`.
