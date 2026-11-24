# Claude Code Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- Native `PreToolUse` hook: `rtk init -g` registers `rtk hook claude` (`src/hooks/hook_cmd.rs`); no shell or `jq` needed
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know RTK is involved)
- Exits silently (exit 0) when no rewrite applies or on any failure
- `rtk init -g` removes a legacy `~/.claude/hooks/rtk-rewrite.sh` (before 0.37.2) and registers the native hook
- `../rtk-awareness.md` (or `-high` / `-full`, per `awareness.level` in config.toml) is the instructions file embedded into CLAUDE.md by `rtk init`
