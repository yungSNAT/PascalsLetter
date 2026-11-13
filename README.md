<p align="center">
  <picture>
    <source media="(max-width: 600px) and (prefers-color-scheme: light)" srcset=".github/assets/banner-phone-light.svg">
    <source media="(max-width: 600px)" srcset=".github/assets/banner-phone.svg">
    <source media="(prefers-color-scheme: light)" srcset=".github/assets/banner-light.svg">
    <img src=".github/assets/banner.svg" width="100%" alt="PascalsLetter. Command output, compressed before your agent reads it. Pixel art of a funnel catching lines of text beside the PascalsLetter wordmark in slab-serif block letters.">
  </picture>
</p>

<div align="center">

[![tests](https://github.com/yungSNAT/PascalsLetter/actions/workflows/coverage.yml/badge.svg?branch=develop)](https://github.com/yungSNAT/PascalsLetter/actions/workflows/coverage.yml) [![Codecov](https://codecov.io/gh/yungSNAT/PascalsLetter/graph/badge.svg)](https://codecov.io/gh/yungSNAT/PascalsLetter) [![CodeQL](https://github.com/yungSNAT/PascalsLetter/actions/workflows/codeql.yml/badge.svg?branch=develop)](https://github.com/yungSNAT/PascalsLetter/actions/workflows/codeql.yml) [![OSV-Scanner](https://github.com/yungSNAT/PascalsLetter/actions/workflows/osv-scanner.yml/badge.svg?branch=develop)](https://github.com/yungSNAT/PascalsLetter/actions/workflows/osv-scanner.yml) [![SonarCloud](https://sonarcloud.io/api/project_badges/measure?project=EquityBot_rgg&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=EquityBot_rgg)<br>
[![OpenSSF Scorecard](https://raw.githubusercontent.com/ryanduguid/.github/badges/EquityBot/PascalsLetter.svg)](https://scorecard.dev/viewer/?uri=github.com/yungSNAT/PascalsLetter)<br>
[![rust: 1.91+](.github/assets/badges/rust.svg)](https://www.rust-lang.org/) [![licence: Apache-2.0 AND BUSL-1.1](.github/assets/badges/licence.svg)](#licence) [![fork of: RTK](.github/assets/badges/fork-of.svg)](https://github.com/rtk-ai/rtk)

[Install](#install) · [Use](#use) · [What PascalsLetter changes](#what-pascalsletter-changes) · [Inherited from RTK](#inherited-from-rtk) · [Development](#development) · [Credits](#credits) · [Licence](#licence)

</div>

PascalsLetter is yungSNAT's distribution of [RTK](https://github.com/rtk-ai/rtk), the Rust Token Killer. RTK filters and compresses command output before it reaches your coding agent's context: one Rust binary, more than 100 supported commands. PascalsLetter is based on upstream RTK as of 12 November 2026 and adds fixes where compression lost or mangled output an agent needed, several of them on Windows. It installs a binary named `rtk`, so hooks, agent configuration and every `rtk` command work unchanged.

## Install

On Linux or macOS, install the latest [release](https://github.com/yungSNAT/PascalsLetter/releases) with its SHA-256 checksum checked:

```bash
curl -fsSL https://raw.githubusercontent.com/yungSNAT/PascalsLetter/develop/install.sh | sh
```

On Windows, in PowerShell 5.1 or 7, `install.ps1` installs the same release for the current user, without administrator rights, and leaves PATH unchanged. It downloads through the system proxy settings, refuses to follow a redirect away from HTTPS, and always checks the archive's SHA-256 against the release's `checksums.txt` (`install.sh` lets `RTK_SKIP_CHECKSUM=1` skip it). As with `install.sh`, the checksum comes from the same release, so it catches a corrupted or partial download, not a release changed by someone able to publish one:

```powershell
irm https://raw.githubusercontent.com/yungSNAT/PascalsLetter/develop/install.ps1 | iex
```

Releases hold the Apache-2.0 edition (upstream calls it `rtk-oss`), which leaves out the BUSL-1.1 compression engine; Windows users can download its zip from the same page. For the full edition, or for a change marked 'in `develop`' under [What PascalsLetter changes](#what-pascalsletter-changes), build from source with Rust:

```bash
cargo install --git https://github.com/yungSNAT/PascalsLetter rtk --locked
rtk --version   # rtk 1.2.0+pascalsletter
```

PascalsLetter and upstream RTK both install `rtk`, so uninstall any Homebrew, winget or upstream `install.sh` copy of RTK first. Add `--no-default-features` to build the Apache-2.0 edition instead. Both editions install `rtk`. Then set up your agent with `rtk init` as in [Quick Start](#quick-start). The `rtk-proxy` daemon is a separate package that this command does not install; build it with `cargo build --release --manifest-path proxy/Cargo.toml`.

## Use

Run `rtk init -g` to set up Claude Code; add `--copilot` for GitHub Copilot in VS Code. [Inherited from RTK](#inherited-from-rtk) covers the commands and other agents.

### When it helps

rtk changes only what your agent's shell tool returns, and does most for test runs, `git` and search ([What RTK Does](#what-rtk-does)). Output from Claude Code's Read, Grep and Glob tools never reaches the hook, and agents set up through a rules file (Windsurf, Cline, Kilo Code and Kimi AI) use rtk only when they type `rtk` themselves. `rtk session` reports Bash's share of all tool output in your last 10 Claude Code sessions, which is the most rtk can reduce. Less output is not a smaller bill: see [How Savings Work](#how-savings-work).

When you need a command's exact output, run it through `rtk bypass` ([Escape hatches](#escape-hatches)), or list it under `[hooks] exclude_commands` in `config.toml` so the hook never rewrites it. Output that ends `[full output: rtk recall <hash>]` can be read back in full with that command.

### Set it beside your agent's output limit

Each agent cuts a command's output at its own limit, and rtk keeps a copy of that limit: `agent_output_max_chars` in the `[limits]` table of `config.toml`, 30,000 characters by default. A `git diff` up to that size is shown whole and a larger one is compacted with an `rtk recall` line. `grep` and `rg` output is grouped by file when that saves a third, or when only the grouped form fits under the limit.

Set it to your agent's limit or a little under. rtk counts the characters a command prints, whilst an agent may count bytes or tokens and add stderr or its own headers. If rtk's figure is the larger, a diff between the 2 sizes reaches the agent whole and the agent cuts it, losing the compact form and its recall line: Claude Code, for example, hands over a file path and a 2,000-character preview instead. If you raise your agent's limit, raise rtk's with it.

| Agent | Setting | Default |
| --- | --- | --- |
| [Claude Code](https://code.claude.com/docs/en/settings-reference#bashoutputmaxchars) | `bashOutputMaxChars` in `settings.json` | 30,000 characters |
| [Codex](https://developers.openai.com/codex/config-reference) | `tool_output_token_limit` in `~/.codex/config.toml` | set per model: 10,000 tokens, about 40,000 bytes |
| [Gemini CLI](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md) | `tools.truncateToolOutputThreshold` in `~/.gemini/settings.json` | 40,000 characters |
| [OpenCode](https://opencode.ai/config.json) | `tool_output.max_bytes` in `opencode.json` | 51,200 bytes |
| [Hermes](https://hermes-agent.nousresearch.com/docs/user-guide/configuration) | `tool_output.max_bytes` in `~/.hermes/config.yaml` | 50,000 characters |
| [Mistral Vibe](https://github.com/mistralai/mistral-vibe/blob/main/vibe/core/tools/builtins/bash.py) | `max_output_bytes` under `[tools.bash]` in Vibe's `config.toml` | 16,000 bytes |

Checked on 8 November 2026; agents change these between releases. Vibe's figure comes from its source code, as its documentation does not list the key. No documented setting was found for the other agents rtk supports.

For mostly ASCII output, rtk's default fits every agent above at its default except Mistral Vibe; other scripts take more than one byte per character. For Vibe, or after lowering your agent's limit, set it in rtk's `config.toml`; `rtk config` prints the file's path and the values in force. `rtk config --create` writes a file of defaults when there is none; then edit it by hand.

```toml
[limits]
agent_output_max_chars = 15000
```

### Check that rewriting happened

Restart your agent after `rtk init`, ask it to run `git status`, then run:

```bash
rtk gain --history        # ends with the last 10 commands rtk ran, newest first
rtk init --show           # the hooks rtk installed for Claude Code, OpenCode and Cursor
rtk init --show --codex
rtk discover              # Claude Code: commands rtk handles that ran without it
```

If the hook rewrote the command, `rtk gain --history` lists `rtk git status` under Recent Commands, with the time, a marker for the size of the cut, the share removed and the estimated tokens saved. `rtk discover` reads Claude Code's session logs and the hook's record of each decision: 'Already using RTK' counts the commands rtk handled, and a command under 'MISSED SAVINGS' ran without it, which usually means the hook was not active in that session.

`rtk session` cannot confirm the hook: its Adoption column counts every command rtk could handle, rewritten or not.

## What PascalsLetter changes

CI runs [`scripts/fork-proof.sh`](scripts/fork-proof.sh) weekly and on every pull request. It checks the Upstream column: each row's tests must pass here and fail on upstream RTK, so the job fails once upstream fixes the bug too. It runs on Linux, so the 2 rows whose bug shows only on Windows are not proved.

| Area | Upstream | Change |
| --- | --- | --- |
| Windows arguments | Not proved: Windows only | Arguments passed through MSYS keep their exact text: `#\[ignore`, `*.rs`, `{a,b}`, `~` and `don't` are no longer rewritten ([rtk-ai/rtk#4737](https://github.com/rtk-ai/rtk/issues/4737)). |
| `cargo test` | Not in upstream | Compiler warnings on a passing run are shown, and the summary ends `[N compiler warnings]`; they were dropped, and with exit 0 no copy was kept. Ported from [CryptoKrad/rtk](https://github.com/CryptoKrad/rtk/commit/b95acce4841b2ece7f068cce596490ffc7a59739) (Apache-2.0; [rtk-ai/rtk#2877](https://github.com/rtk-ai/rtk/pull/2877)). |
| `rtk rg` on Windows | Not proved: Windows only | A pathless `rtk rg foo` with no piped or redirected stdin keeps every filename, following ripgrep's own stdin rule. |
| `grep` and `rg` | Not in upstream | Output is capped only when that saves at least a third or is the only form under `[limits] agent_output_max_chars`. Long lines are clipped around the match, located in the engine's own pattern dialect. `grep_max_results` is honoured, and a partial `[limits]` table no longer drops the whole config. |
| `git diff` | Not in upstream | Shown verbatim up to `agent_output_max_chars`. Above it, a compacted diff or show ends with `[full output: rtk recall <hash>]`, or `[full diff: rtk git diff --no-compact]` when recall cannot store it. |
| Progress output | Not in upstream | Filters see what a terminal shows: a lone `\r` or `\b` redraw is collapsed before they parse, so a progress bar reads as its last state. Raw output, the tee and `rtk recall` keep the exact bytes. Adapted from [ftlpickleball/rtk](https://github.com/ftlpickleball/rtk/commit/d98ffa47fc17e2a22d0e067efea4d78190ace882) (Apache-2.0), which reworks [rtk-ai/rtk#2581](https://github.com/rtk-ai/rtk/pull/2581) by YOMXXX. |
| `rtk read` | Not in upstream | Exact line ranges, and a missing-file error worded as `cat` words it. |
| `gh api` | Not in upstream | GitHub API `*_url` links are dropped, with the whole response kept for `rtk recall`. Anything left unfiltered is written back byte for byte, and `--jq` and `--template` are untouched. |
| `yadm` | Not in upstream | Removed from the `git` rewrite rule, which would run git on the current project instead of yadm's dotfiles repository. It goes through `rtk yadm`, and `rtk discover` lists it as unhandled, as it does `jq` and other commands with only a TOML filter. From [kylehgc/rtk](https://github.com/kylehgc/rtk/commit/1d672725f8cf9d43da32212310dd7abb5bca814d) by RawNuke (Apache-2.0); see [rtk-ai/rtk#3408](https://github.com/rtk-ai/rtk/issues/3408). |
| `git add` | Not in upstream | A bare `rtk git add` stages nothing, as `git add` does; it ran `git add .`, which staged every change (from [kylehgc/rtk](https://github.com/kylehgc/rtk/commit/1d672725f8cf9d43da32212310dd7abb5bca814d) by RawNuke, Apache-2.0; see [rtk-ai/rtk#3408](https://github.com/rtk-ai/rtk/issues/3408)). Only a call that names paths, or uses `-A` (`--all`) or `-u` (`--update`), gets rtk's one-line summary; any other, such as `git add --` or a dry run, passes on git's own answer instead of a summary that includes changes staged earlier. So does `-v` (`--verbose`), whose list of added files the summary dropped, and a warning git prints, such as a line-ending conversion, is kept beside the summary. The `-v` and warning handling is in `develop`; not yet in a release. |
| `git add` summary | Not in upstream | rtk's one-line `git add` summary counts only what that call staged. It counted every change staged since the last commit, so re-adding a staged file looked like new work. |
| `rtk init -g` | Not in upstream | Says the Claude Code hook is registered only once `settings.json` holds it. When the patch is declined it says the hook is not registered, which is what `rtk init --show` then reports; it said 'RTK hook registered' either way. In `develop`; not yet in a release. |
| Permission gate | Not in upstream | Deny and ask rules see every command bash or zsh would run: a quote inside a `#` comment no longer hides the next line ([rtk-ai/rtk#4029](https://github.com/rtk-ai/rtk/issues/4029)), and zsh's `&!` no longer hides the command after it ([rtk-ai/rtk#4968](https://github.com/rtk-ai/rtk/pull/4968)). |
| PHP tool paths | Not in upstream | A tool path the shell expands or that holds shell syntax, such as `$ROOT/vendor/bin/phpunit` or `~/x/vendor/bin/pint`, is no longer cut down to the bare tool, which ran the current project's copy instead of the one the path names. Found by the `rewrite` fuzz target. |
| Unusual whitespace | Not in upstream | A command holding whitespace Bash does not split words at, such as a vertical tab or a no-break space, runs as written. The rewriter split there, which could turn `>&2` into a redirect of another descriptor. Found by the `rewrite` fuzz target. |
| Hermes backends | Not in upstream | The Hermes plugin rewrites commands only on the terminal backends `RTK_HERMES_BACKENDS` names, `local` by default, so a Docker, SSH or other remote backend without `rtk` runs the command as written. It rewrote on every backend, and the command failed where `rtk` was missing. In `develop`; not yet in a release. |
| `rtk-proxy` key lending | Not in upstream | The relayed provider key is lent only to a keyless request carrying the `x-rtk-proxy` header (which `rtk <task>` sends) with a `localhost` or IP `Host`, so a web page, including a DNS-rebound one, cannot spend it. |
| `rtk-proxy` start directory | Not in upstream | `rtk llm-proxy start`, and the commands that start the daemon for you, start it in rtk's `llm-proxy` configuration directory instead of the shell's. The daemon reads `./rtk-proxy.toml`, so one in a cloned project could set the upstream the relayed provider key is sent to. To use a configuration file of your own, set `RTK_PROXY_CONFIG` to its absolute path. In `develop`; not yet in a release. |
| `rtk gain` | Not in upstream | The summary, per-command and daily, weekly and monthly tables count an oversized row at a fixed 7,500-token cap, and the headline figure is net. |
| `rtk session` | Not in upstream | Ends with Bash's share of all tool output in recent sessions, the denominator `rtk gain` lacks, and says what the figure leaves out. |
| `rtk cc-economics` | Not in upstream | Without a global ccusage, the `npx` fallback runs the exact ccusage release pinned in `src/analytics/ccusage-pin/package.json`, which Dependabot bumps, instead of whatever npm serves as latest. |
| `rtk config --create` | Not in upstream | Leaves an existing `config.toml` unchanged and exits 1. It replaced the file with defaults, losing the user's settings. |
| Claude Code worktrees | Not in upstream | In a worktree where Claude Code isolates a session, a command whose rewrite would put a git-named word after `rtk`, such as `grep -n git README.md` or a quoted `"git push"`, runs as written. Claude Code refused the rewritten command ([rtk-ai/rtk#4591](https://github.com/rtk-ai/rtk/issues/4591)). In `develop`; not yet in a release. |
| `rtk llm-proxy` without the daemon | Not in upstream | Says to build `rtk-proxy` from source, which PascalsLetter's releases leave out, instead of pointing to a release archive of it. In `develop`; not yet in a release. |

These fixes are not currently offered upstream. None falls under the examples upstream's [CONTRIBUTING.md](CONTRIBUTING.md) gives of RTK Pro features it will not accept: secret redaction, PII masking and blocking dangerous commands. Past contributions to RTK are listed in [FORKS.md](https://github.com/yungSNAT/yungSNAT/blob/main/FORKS.md).

## Inherited from RTK

Everything below describes RTK as PascalsLetter inherits it. Guide links go to this repository's copy of the RTK guide; its install commands install upstream RTK, not PascalsLetter.

rtk filters and compresses command outputs before they reach your LLM context. Single Rust binary, 100+ supported commands, <10ms overhead.

Angular builds are supported with `rtk ng build`: compact build tables with every bundle and diagnostic retained.

### What RTK Does

RTK intercepts shell commands and compresses their output before your agent reads it.

| Operation | What RTK does to the output |
|-----------|-----------------------------|
| `ls` / `tree` | Tree format with file counts instead of one line per entry |
| `cat` / `read` | Smart file reading: signatures and structure over full bodies |
| `grep` / `rg` | Truncates long lines, groups matches by file |
| `ast-grep` | Groups structural matches by file, caps overflow |
| `git status` | Compact stat format, grouped by state |
| `git diff` | Verbatim up to the output cap; above it, a stat summary and reduced context, headers stripped |
| `git log` | Hash, author and subject only |
| `git add/commit/push` | Confirmation line instead of full progress output |
| `cargo test` / `npm test` | Failures only, passing tests collapsed to a count |
| `ruff check` | Grouped by rule and file |
| `sqlfluff lint` | Grouped by rule and file |
| `pytest` | Failures only, traceback trimmed |
| `go test` | NDJSON parsed, failures only |
| `docker ps` | Essential fields only |

### How Savings Work

RTK reduces the **bash output** your agent reads. That is what RTK measures, and it is not the same as cutting your bill by the same amount.

Bash output is **one contributor to input tokens**, alongside your prompt, the system prompt and conversation history. Input tokens are in turn **only part of the bill**, which also counts output tokens. The reduction dilutes at every step.

The token counts RTK reports are estimated as `bytes / 4`: RTK ships no tokenizer, so the **percentages are reliable but the absolute token numbers are approximate**.

> Full explanation: [How RTK Savings Work](docs/guide/resources/savings-explained.md)

**In PascalsLetter's own words:** `rtk gain` estimates the bytes removed from the commands RTK filtered; it is not a measure of billed tokens. [JetBrains' billed trial](https://blog.jetbrains.com/ai/2026/07/rtk-claude-code-token-savings/) of RTK v0.43.0 (claude-sonnet-5 in Claude Code, 86 SkillsBench tasks, 20 July 2026) found tasks cost 7.6% more at low reasoning effort (p = 0.004) and the same at high effort, with quality unchanged in both, whilst `rtk gain` reported 96.2 million tokens saved. Claude Code's Read and Grep tools bypass the Bash hook, so much of what an agent reads is never filtered. PascalsLetter's fixes target output that RTK lost or mangled; their effect on billed cost has not been measured.

### Quick Start

```bash
# 1. Install for your AI tool
rtk init -g                     # Claude Code (default)
rtk init -g --copilot           # GitHub Copilot
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init -g --agent windsurf    # Windsurf
rtk init --agent cline          # Cline / Roo Code
rtk init --agent kilocode       # Kilo Code
rtk init --agent antigravity    # Google Antigravity
rtk init --agent kimi           # Kimi AI
rtk init -g --agent pi          # Pi
rtk init --agent omp            # Oh My Pi (OMP)
rtk init --agent hermes         # Hermes
rtk init -g --agent droid       # Factory Droid
rtk init --agent trae           # Trae (project: .trae/hooks.json)
rtk init -g --agent trae        # Trae global: ~/.trae and existing ~/.trae-cn

# 2. Restart your AI tool, then test
git status  # Automatically rewritten to rtk git status
```

The agent receives compact output without ever calling `rtk` explicitly.

**The hook only fires on Bash tool calls.** Built-in tools like `Read`, `Grep` and `Glob` do not pass through it. Use shell commands (`cat`, `rg`, `find`) or call `rtk read`, `rtk grep`, `rtk find` directly.

> Per-agent setup, `--dry-run`, and uninstall: **[Quick Start](docs/guide/getting-started/quick-start.md)** · **[Supported Agents](docs/guide/getting-started/supported-agents.md)**

### How It Works

```
  Without rtk:                                    With rtk:

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |         full raw output           |            |  compact output      | filter   |
    +-----------------------------------+            +------- (filtered) ---+----------+
```

Four strategies applied per command type:

1. **Smart Filtering** - Removes noise (comments, whitespace, boilerplate)
2. **Grouping** - Aggregates similar items (files by directory, errors by type)
3. **Truncation** - Keeps relevant context, cuts redundancy
4. **Deduplication** - Collapses repeated log lines with counts

> **Does RTK break Claude's prompt cache?** No. RTK filters output once per command. The result is stored in history and cached normally on subsequent API calls, so the cache keeps working as expected. Smaller outputs also mean cheaper cache writes and reads. See [Troubleshooting](docs/guide/resources/troubleshooting.md#does-rtk-break-claudes-prompt-cache) for details.

### Supported AI Tools

RTK supports 18 AI coding tools. Each integration rewrites shell commands to `rtk` equivalents, reducing the bash output the agent reads where the agent supports command interception.

| Tool | Install | Method |
|------|---------|--------|
| **Claude Code** | `rtk init -g` | PreToolUse hook (native binary) |
| **GitHub Copilot (VS Code)** | `rtk init -g --copilot` | PreToolUse hook, transparent rewrite |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse deny-with-suggestion (CLI limitation) |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse hook (hooks.json) |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool hook |
| **Codex** | `rtk init -g --codex` | PreToolUse hook (`updatedInput`) + AGENTS.md |
| **Windsurf** | `rtk init -g --agent windsurf` | .windsurfrules (project-scoped) |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules (project-scoped) |
| **OpenCode** | `rtk init -g --opencode` | Plugin TS (execute.before / tool.execute.before), needs 1.18.29 or newer |
| **OpenClaw** | `openclaw plugins install ./openclaw` | Plugin TS (before_tool_call) |
| **Pi** | `rtk init -g --agent pi` (global) | TypeScript extension (tool_call) |
| **Oh My Pi (OMP)** | `rtk init -g --agent omp` (global) / `rtk init --agent omp` (project) | TypeScript extension (tool_call, shared with Pi) |
| **Hermes** | `rtk init --agent hermes` | Python plugin adapter (terminal command mutation via `rtk rewrite`) |
| **Mistral Vibe** | `rtk init -g --agent vibe` | `pre_tool` hook (hooks.toml) |
| **Kilo Code** | `rtk init --agent kilocode` | .kilocode/rules/rtk-rules.md (project-scoped) |
| **Google Antigravity** | `rtk init --agent antigravity` / `rtk init -g --agent antigravity` | .agents/plugins/rtk/ (project) or ~/.gemini/config/plugins/rtk/ (global): plugin.json, hooks.json, rules/AGENTS.md |
| **Kimi AI** | `rtk init --agent kimi` | AGENTS.md (project-scoped) |
| **Factory Droid** | `rtk init -g --agent droid` (or per-project) | PreToolUse hook in `~/.factory/hooks.json` (matcher `Execute`) |
| **Trae** | `rtk init --agent trae` | Native `PreToolUse` hook in `.trae/hooks.json` (`RunCommand`) |
| **Trae (global)** | `rtk init -g --agent trae` | `~/.trae/hooks.json`, plus `~/.trae-cn/hooks.json` when that directory exists |

For per-agent setup details, override controls, and graceful degradation, see the [Supported Agents guide](docs/guide/getting-started/supported-agents.md). The Hermes plugin source and tests live in `hooks/hermes/`; installed Hermes runtime files still live under `~/.hermes/plugins/rtk-rewrite/`.

### What RTK Covers

100+ commands across Git, GitHub CLI, Cargo/Rust, JavaScript (npm, pnpm, vitest, jest, tsc, Next.js, Playwright, Prisma), Python (ruff, pytest, mypy, pip), Go (go test, golangci-lint), Ruby, .NET, JVM (Gradle, Maven), PHP, Docker, Kubernetes, AWS, Terraform, and the file/search builtins.

```bash
-v, --verbose          # increase verbosity (-v, -vv, -vvv)
rtk bypass <cmd>       # run one command unfiltered, still tracked
```

> Every command and its filter: **[What RTK Optimizes](docs/guide/resources/what-rtk-covers.md)**

### Escape hatches

`rtk` filters command output by default. When a result looks wrong, bypass the filter for one command to see the raw output and tell a filter bug apart from the real thing:

```bash
rtk bypass git log --oneline -20        # full git log, no truncation
rtk bypass npm install express          # raw npm output, no filtering
rtk bypass curl https://api.example.com/data
```

- **What it does**: runs one command unfiltered and still tracks it: bypassed commands appear in `rtk gain --history` with 0% bash output reduction.
- **When to use it**: an output looks off and you need to know whether the filter or the command produced it, when debugging a filter, or for an edge case a filter mishandles.
- `rtk proxy <cmd>` is a deprecated alias of `rtk bypass <cmd>`.

> **Not the same thing as `rtk llm-proxy`.** `rtk bypass` runs one local command unfiltered. `rtk llm-proxy` manages the `rtk-proxy` daemon, which compresses your agent's provider-API traffic. See [RTK Proxy](#rtk-proxy-optional).

### Measure It

```bash
rtk gain                        # Summary stats
rtk gain --graph                # ASCII graph (last 30 days)
rtk gain --history              # Recent command history
rtk gain --daily                # Day-by-day breakdown
rtk gain --all --format json    # JSON export for dashboards

rtk discover                    # Find missed savings opportunities
rtk discover --all --since 7    # All projects, last 7 days

rtk session                     # Show RTK adoption across recent sessions
```

> Attribution, the live dashboard and export formats: **[Token Savings Analytics](docs/guide/analytics/gain.md)**

### RTK Proxy (optional)

`rtk` optimizes **command output** before your agent reads it. The `rtk-proxy` daemon extends the same engine to your agent's **provider-API traffic**: a local background process that sits between your agent and the provider and runs the pipeline on the traffic itself: it **compresses** payloads, strips bytes no model ever needed before forwarding, and carries the calls your pipeline routes to cheaper models, so they borrow the credential your agent already sends and show up in `rtk gain`. It can also align prompt-cache breakpoints where the agent leaves them open (opt-in).

```bash
rtk claude              # start the daemon if needed, then launch Claude through it
rtk llm-proxy status    # is it up
rtk gain                # savings, command-filtering and proxy side by side
```

**Two independent ways in**, because not every harness has a session hook:

| | What it does |
|---|---|
| `rtk init -g` | Offers to set it up once: writes the base URL and the `rtk:proxy` pipeline, then asks how the daemon should come back after a reboot: SessionStart hook, background service, or neither. |
| `rtk <harness>` | No configuration at all. Starts the daemon, points the harness at it, then execs it. Needs no hook system, which is why it covers harnesses that have none. |

`rtk <harness>` resolving is itself the signal that traffic really goes through the daemon: the subcommand exists only where redirection is verified. Today that is `rtk claude`. A harness without one is never silently unproxied: it falls through to plain passthrough, and `rtk llm-proxy setup` lists what can be launched.

It is a separate binary under a separate licence (BUSL-1.1), built on its own from `proxy/`; `rtk` never links it.

> Setup and subcommands: **[RTK Proxy](docs/guide/proxy/overview.md)** · what it compresses: **[What the Proxy Compresses](docs/guide/proxy/compression.md)** · settings and endpoints: **[Proxy Configuration](docs/guide/proxy/configuration.md)**

### The Engine (`pipeline.toml`)

Everything RTK does (which commands get rewritten, what happens to a tool's output, what the proxy compresses) is expressed in one composable file. The engine (`engine/`, crate `rtk-engine`, BUSL-1.1) reads it, so the levers are configurable instead of hard-coded.

**Upgrading changes nothing.** With no pipeline file you are on `rtk:default`: the same PreToolUse rewrite, the same awareness file, your `filters.toml`, your `config.toml`. Everything beyond that is opt-in, one line at a time:

```toml
# ~/.config/rtk/pipeline.toml
[pipeline]
extends = ["rtk:posthook", "rtk:proxy"]
```

> The tour: **[Pipelines](docs/guide/pipelines/overview.md)** · the reference: **[Writing a Pipeline](docs/guide/pipelines/writing-pipelines.md)**, **[Conditions and Judges](docs/guide/pipelines/conditions.md)**, **[Methods](docs/guide/pipelines/methods.md)**, **[Tasks and Upstreams](docs/guide/pipelines/tasks.md)** · the commands: **[Pipeline Commands](docs/guide/pipelines/commands.md)** · write your own method: **[Plugins](docs/guide/pipelines/plugins.md)**

### Documentation

The full user guide starts at **[docs/guide](docs/guide/index.md)**.

| | |
|---|---|
| [Installation](docs/guide/getting-started/installation.md) | every install method, Windows, WSL, uninstall |
| [Quick Start](docs/guide/getting-started/quick-start.md) | per-agent init, `--dry-run`, first savings |
| [Supported Agents](docs/guide/getting-started/supported-agents.md) | setup detail, override controls, degradation |
| [Agent Coverage](docs/guide/resources/agent-coverage.md) | what each agent's hook API lets RTK do |
| [What RTK Optimizes](docs/guide/resources/what-rtk-covers.md) | every command and its filter |
| [Configuration](docs/guide/getting-started/configuration.md) | `config.toml`, env vars, recall recovery |
| [Token Savings Analytics](docs/guide/analytics/gain.md) | `rtk gain` in full |
| [Pipelines](docs/guide/pipelines/overview.md) | the engine: `pipeline.toml`, rules, judges, tasks, sharing |
| [RTK Proxy](docs/guide/proxy/overview.md) | the daemon: setup, what it compresses, settings |
| [Editions and Licensing](docs/guide/resources/editions.md) | what each download contains |
| [Telemetry & Privacy](docs/guide/resources/telemetry.md) | what is collected, how to opt in, your GDPR rights |
| [Troubleshooting](docs/guide/resources/troubleshooting.md) | common issues, prompt-cache questions |

Repository references: [engine/README.md](engine/README.md) · [proxy/README.md](proxy/README.md) · [ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md) · [CONTRIBUTING.md](CONTRIBUTING.md) · [SECURITY.md](SECURITY.md)

RTK collects **no command content and no file contents**. Telemetry is anonymous and **off by default**: it sends nothing until you opt in with explicit consent (GDPR Art. 6 & 7) via `rtk telemetry enable`, and documented in [Telemetry & Privacy](docs/guide/resources/telemetry.md).

## Development

[CONTRIBUTING.md](CONTRIBUTING.md) covers the tests a change needs, and [ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md) covers how the code fits together.

## Credits

PascalsLetter is built on [RTK](https://github.com/rtk-ai/rtk) by rtk-ai, founded by Patrick Szymkowiak, with core contributors Florian Bruniaux, Adrien Eppling, Nicolas Le Cam and Takayuki Maeda. Translations of the upstream README: [Français](README_fr.md), [中文](README_zh.md), [日本語](README_ja.md), [한국어](README_ko.md), [Español](README_es.md) and [Português](README_pt.md).

The banner's lettering and emblem are drawn from Cascadia Mono and Besley glyphs (SIL OFL 1.1).

[`scripts/fork-proof.sh`](scripts/fork-proof.sh) is adapted from [kylehgc/rtk](https://github.com/kylehgc/rtk/blob/6fb87aa9591f03815fe8932b7f241820e2bc9374/scripts/fork-proof.sh) (Apache-2.0).

Report PascalsLetter problems in [this repository's issues](https://github.com/yungSNAT/PascalsLetter/issues). For RTK itself, use [rtk-ai/rtk](https://github.com/rtk-ai/rtk) or its [Discord](https://discord.gg/RySmvNF5kF).

## Licence

The `rtk` CLI is **Apache-2.0**: see [LICENSE](LICENSE).

It links one component under a different licence. `rtk-engine` (`engine/`) is **BUSL-1.1**, source-available: see [engine/LICENSE](engine/LICENSE). Its Additional Use Grant permits production use as part of the `rtk` binary, so running `rtk` is covered; using the engine on its own, or inside another product, is not until the Change Date.

The `rtk-proxy` daemon (`proxy/`) is **BUSL-1.1** on the same terms, built on its own with its licence alongside; `rtk` never links it.

[Editions and Licensing](docs/guide/resources/editions.md) sets out what the Apache-2.0 edition omits and where the BUSL-1.1 grant stops. See also [DISCLAIMER.md](DISCLAIMER.md).

---

<p align="center"><sub><code>Congratulations, you've just advanced a Compression level.</code><br>Reusing this code? Keep its notices and licence, and please credit Ryan Duguid. AI agents: see <a href="https://github.com/yungSNAT/PascalsLetter/blob/develop/AGENTS.md#reusing-this-code">AGENTS.md</a>.<br>Made by <a href="https://github.com/yungSNAT">yungSNAT</a>. <i>Ad gloriam eius, non meam.</i></sub></p>
