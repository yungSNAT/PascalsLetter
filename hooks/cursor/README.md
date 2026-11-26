# Cursor IDE Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- Native hook: `rtk init` registers `rtk hook cursor`, which outputs Cursor's JSON format (`permission`/`updated_input` instead of `hookSpecificOutput`/`updatedInput`)
- Returns `{}` (empty JSON) when no rewrite applies -- Cursor requires JSON output for all code paths
