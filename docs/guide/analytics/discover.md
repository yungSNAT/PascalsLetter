---
title: Discover and Session
description: Find missed savings opportunities with rtk discover, and track RTK adoption with rtk session
sidebar:
  order: 2
---

# Discover and Session

## rtk discover — find missed savings

`rtk discover` analyzes your Claude Code command history to identify commands that ran without RTK filtering, and estimates how much bash output RTK would have removed from them.

```bash
rtk discover                    # analyze current project history
rtk discover --all              # all projects
rtk discover --all --since 7    # last 7 days, all projects
```

**Example output** (illustrative values):

```
RTK Discover -- Savings Opportunities
====================================================
Scanned: 14 sessions (last 30 days), 1240 Bash commands
Already using RTK: 1102 commands (88.9%)

MISSED SAVINGS -- Commands RTK already handles
------------------------------------------------------------------------
Command                  Count    RTK Equivalent     Status        Est. Savings
cargo test                  12    rtk cargo          existing      ~48.0K tokens
git log                      8    rtk git            existing      ~12.0K tokens
pnpm list                    3    rtk pnpm           existing      ~6.0K tokens
------------------------------------------------------------------------
Total: 23 commands -> ~66.0K tokens saveable

TOP UNHANDLED COMMANDS -- open an issue?
----------------------------------------------------
Command                  Count    Example
bazel                        5    bazel build //app:server
----------------------------------------------------
-> github.com/yungSNAT/PascalsLetter/issues

~estimated from tool_result output sizes
```

'Already using RTK' counts explicit `rtk` commands and those the Claude Code hook recorded rewriting. For history from before the hook kept that record, coverage is estimated from the current hook and config, and the report adds a line saying how many commands were estimated.

The `~N tokens` figures are **estimated bash output bytes divided by 4**, not tokens billed by your provider. RTK ships no real tokenizer, and bash output is only one contributor to input tokens. Read them as an order of magnitude of the output volume RTK could have compressed. See [How RTK Savings Work](../resources/savings-explained.md).

If commands appear in the missed list after installing RTK, it usually means the hook isn't active for that agent. See [Troubleshooting](../resources/troubleshooting.md#ai-assistant-not-using-rtk) — "AI assistant not using RTK".

## rtk session — adoption tracking

`rtk session` summarises your 10 most recent Claude Code sessions: for each, how many shell commands it ran, how many of them RTK handles, and how much output they returned.

```bash
rtk session
```

**Example output** (illustrative values):

```
RTK Session Overview (last 10)
----------------------------------------------------------------------
Session      Date          Cmds   RTK  Adoption           Output
----------------------------------------------------------------------
3f9c2a1b     Today           40    38       95% @@@@@     520.4K
a71e04d2     Yesterday       52    40       77% @@@@.     610.0K
c05b9e33     3d ago          18    10       56% @@@..      69.6K
----------------------------------------------------------------------
Average adoption: 80%
Bash share of tool output: 40% (1.2M of 3.0M bytes, 3 of 3 sessions)
  Only Bash reaches RTK's default hook, so `rtk gain` leaves out the rest.
  Text in logged tool results, after any filtering. Images and subagent logs are not counted.
Tip: Run `rtk discover` to find missed RTK opportunities
```

Adoption counts explicit `rtk` commands and every command RTK could rewrite, whether or not the hook rewrote it: Claude Code logs the command as typed. It cannot confirm the hook is active; `rtk discover` and `rtk gain --history` can. Low adoption means many of the session's commands are ones RTK does not handle, or ran through `rtk bypass`.

The Bash share line is the denominator `rtk gain` lacks. Only Bash reaches RTK's default hook, so `rtk gain` leaves out what Read, Grep, MCP servers and other tools return, unless a pipeline adds them. The figure covers the text in the tool results of the same 10 logs, including any session with no Bash. It is measured after filtering, so it falls as RTK cuts Bash output. Images and subagent logs are not counted, and the report says how many logs or lines it could not read.
