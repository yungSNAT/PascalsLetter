//! Gemini agent: hook install/uninstall helpers.

use super::*;
use crate::hooks::constants::{
    BEFORE_TOOL_KEY, GEMINI_DIR, GEMINI_HOOK_FILE, HOOKS_SUBDIR, SETTINGS_JSON,
};

const GEMINI_MD: &str = "GEMINI.md";

// Gemini CLI support

/// Gemini hook wrapper script — delegates to `rtk hook gemini`
const GEMINI_HOOK_SCRIPT: &str = r#"#!/bin/bash
exec rtk hook gemini
"#;

/// Matches RTK's Gemini hook command, alone: the wrapper script `rtk init`
/// writes (at any path) or `rtk hook gemini`. `rtk init` writes the script's
/// path unquoted, and the shell reading of that text drops a Windows path's
/// separators and splits a path with a space, so `installed` (the path this
/// install writes) also matches as raw text.
fn is_gemini_hook_command(command: &str, installed: &str) -> bool {
    let is_script = |path: &str| path.rsplit(['/', '\\']).next() == Some(GEMINI_HOOK_FILE);
    command.trim() == installed
        || crate::hooks::is_rtk_hook_command(command, "gemini")
        || crate::hooks::simple_command_words(command)
            .is_some_and(|words| matches!(words.as_slice(), [word] if is_script(word) || is_script(command.trim())))
}

/// True when a BeforeTool group runs RTK's hook among its commands.
fn group_has_rtk_hook(group: &serde_json::Value, installed: &str) -> bool {
    group
        .get("hooks")
        .and_then(|hooks| hooks.as_array())
        .is_some_and(|hooks| hooks.iter().any(|hook| is_rtk_hook_entry(hook, installed)))
}

fn is_rtk_hook_entry(hook: &serde_json::Value, installed: &str) -> bool {
    hook.get("command")
        .and_then(|c| c.as_str())
        .is_some_and(|command| is_gemini_hook_command(command, installed))
}

fn resolve_gemini_dir() -> Result<PathBuf> {
    resolve_home_subdir(GEMINI_DIR)
}

/// Entry point for `rtk init --gemini`
pub fn run_gemini(
    global: bool,
    hook_only: bool,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<()> {
    let InitContext { dry_run, .. } = ctx;
    if !global {
        anyhow::bail!("Gemini support is global-only. Use: rtk init -g --gemini");
    }

    let gemini_dir = resolve_gemini_dir()?;
    if !dry_run {
        fs::create_dir_all(&gemini_dir).with_context(|| {
            format!(
                "Failed to create Gemini config dir: {}",
                gemini_dir.display()
            )
        })?;
    }

    // 1. Install hook script
    let hook_dir = gemini_dir.join("hooks");
    if !dry_run {
        fs::create_dir_all(&hook_dir)
            .with_context(|| format!("Failed to create hook dir: {}", hook_dir.display()))?;
    }
    let hook_path = hook_dir.join(GEMINI_HOOK_FILE);
    write_if_changed(&hook_path, GEMINI_HOOK_SCRIPT, "Gemini hook", ctx)?;

    #[cfg(unix)]
    if !dry_run {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("Failed to set hook permissions: {}", hook_path.display()))?;
    }

    // Store integrity baseline for tamper detection (skip in dry-run)
    if !dry_run {
        integrity::store_hash(&hook_path).with_context(|| {
            format!("Failed to store integrity hash for {}", hook_path.display())
        })?;
    }

    // 2. Install GEMINI.md (RTK awareness for Gemini)
    if !hook_only {
        let gemini_md_path = gemini_dir.join(GEMINI_MD);
        write_if_changed(
            &gemini_md_path,
            awareness_content(ctx.awareness),
            GEMINI_MD,
            ctx,
        )?;
    }

    // 3. Patch ~/.gemini/settings.json
    let settings_parse_failed = patch_gemini_settings(&gemini_dir, &hook_path, patch_mode, ctx)?;

    if dry_run {
        print_dry_run_footer();
    } else {
        println!("\nGemini CLI hook installed (global).\n");
        println!("  Hook: {}", hook_path.display());
        if !hook_only {
            println!("  GEMINI.md: {}", gemini_dir.join(GEMINI_MD).display());
        }
        if settings_parse_failed {
            println!(
                "  settings.json: NOT patched (existing file could not be parsed; see warning above)"
            );
        }
        println!("  Restart Gemini CLI. Test with: git status\n");
    }
    Ok(())
}

/// Print the manual-setup instructions for ~/.gemini/settings.json, shared by
/// PatchMode::Skip and the unparseable-settings fallback in
/// `patch_gemini_settings`.
fn print_gemini_manual_setup(settings_path: &Path) {
    println!(
        "\nManual setup needed: add RTK hook to {}\n\
         See: https://github.com/rtk-ai/rtk#gemini-cli",
        settings_path.display()
    );
}

/// Patch ~/.gemini/settings.json with the BeforeTool hook.
///
/// Returns `Ok(true)` when the existing settings.json could not be parsed
/// (so the caller can tell an honest "hook installed, settings.json NOT
/// patched" apart from every other reason nothing changed — already
/// patched, `PatchMode::Skip`, declined at the `Ask` prompt, dry-run).
fn patch_gemini_settings(
    gemini_dir: &Path,
    hook_path: &Path,
    patch_mode: PatchMode,
    ctx: InitContext,
) -> Result<bool> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let settings_path = gemini_dir.join(SETTINGS_JSON);
    let hook_cmd = hook_path.to_string_lossy().to_string();

    // Read or create settings.json
    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read {}", settings_path.display()))?;
        let content = strip_leading_bom(&content);

        if content.trim().is_empty() {
            serde_json::json!({})
        } else {
            match from_json_str(content) {
                Ok(v) => v,
                Err(e) => {
                    // A parse failure must not abort the whole `rtk init` run
                    // (run_gemini has already written the hook script,
                    // GEMINI.md, and the integrity baseline by this point).
                    // Treat it like PatchMode::Skip: warn, tell the user how
                    // to patch it themselves, and leave the file untouched.
                    eprintln!(
                        "Warning: failed to parse {} as JSON: {}",
                        settings_path.display(),
                        e
                    );
                    print_gemini_manual_setup(&settings_path);
                    return Ok(true);
                }
            }
        }
    } else {
        serde_json::json!({})
    };

    let before_tool_pointer = format!("/hooks/{}", BEFORE_TOOL_KEY);
    if let Some(hooks) = settings.pointer(&before_tool_pointer)
        && let Some(arr) = hooks.as_array()
        && arr.iter().any(|group| group_has_rtk_hook(group, &hook_cmd))
    {
        if verbose > 0 {
            eprintln!("Gemini settings.json already has RTK hook");
        }
        return Ok(false);
    }

    // Ask user before patching
    if patch_mode == PatchMode::Skip {
        print_gemini_manual_setup(&settings_path);
        return Ok(false);
    }

    if patch_mode == PatchMode::Ask {
        if dry_run {
            println!(
                "[dry-run] would prompt before patching {}",
                settings_path.display()
            );
        } else {
            print!("Patch {} with RTK hook? [y/N] ", settings_path.display());
            std::io::Write::flush(&mut std::io::stdout())?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !answer.trim().eq_ignore_ascii_case("y") {
                println!("Skipped. Add hook manually later.");
                return Ok(false);
            }
        }
    }

    // Build hook entry matching Gemini CLI format
    let hook_entry = serde_json::json!({
        "matcher": "run_shell_command",
        "hooks": [{
            "type": "command",
            "command": hook_cmd
        }]
    });

    // Insert into settings
    let hooks = settings
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("hooks")
        .or_insert(serde_json::json!({}));

    let before_tool = hooks
        .as_object_mut()
        .context("hooks is not an object")?
        .entry(BEFORE_TOOL_KEY)
        .or_insert(serde_json::json!([]));

    before_tool
        .as_array_mut()
        .context("BeforeTool is not an array")?
        .push(hook_entry);

    let content = serde_json::to_string_pretty(&settings)?;

    if dry_run {
        println!(
            "[dry-run] would patch Gemini settings.json: {}",
            settings_path.display()
        );
        if verbose > 0 {
            println!("[dry-run] content:\n{}", content);
        }
        return Ok(false);
    }

    // Write atomically
    let tmp = NamedTempFile::new_in(gemini_dir)?;
    fs::write(tmp.path(), &content)?;
    tmp.persist(&settings_path)
        .with_context(|| format!("Failed to write {}", settings_path.display()))?;

    if verbose > 0 {
        eprintln!("Patched {}", settings_path.display());
    }

    Ok(false)
}

/// Remove Gemini artifacts during uninstall
pub(super) fn uninstall_gemini(ctx: InitContext) -> Result<Vec<String>> {
    let InitContext {
        verbose, dry_run, ..
    } = ctx;
    let mut removed = Vec::new();
    let gemini_dir = match resolve_gemini_dir() {
        Ok(d) => d,
        Err(_) => return Ok(removed),
    };

    // Remove hook
    let hook_path = gemini_dir.join(HOOKS_SUBDIR).join(GEMINI_HOOK_FILE);
    if hook_path.exists() {
        if dry_run {
            println!(
                "[dry-run] would remove Gemini hook: {}",
                hook_path.display()
            );
        } else {
            // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
            fs::remove_file(&hook_path)
                .with_context(|| format!("Failed to remove {}", hook_path.display()))?;
        }
        removed.push(format!("Gemini hook: {}", hook_path.display()));
    }

    // Remove GEMINI.md
    let gemini_md = gemini_dir.join(GEMINI_MD);
    if gemini_md.exists() {
        if dry_run {
            println!("[dry-run] would remove GEMINI.md: {}", gemini_md.display());
        } else {
            // nosemgrep: filesystem-deletion -- expected in hooks/init uninstall-path cleanup and tests.
            fs::remove_file(&gemini_md)
                .with_context(|| format!("Failed to remove {}", gemini_md.display()))?;
        }
        removed.push(format!("GEMINI.md: {}", gemini_md.display()));
    }

    // Remove hook from settings.json
    let settings_path = gemini_dir.join(SETTINGS_JSON);
    if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        if let Ok(mut settings) = from_json_str::<serde_json::Value>(&content)
            && remove_rtk_hooks(&mut settings, &hook_path.to_string_lossy())
        {
            if dry_run {
                println!(
                    "[dry-run] would remove RTK hook from Gemini settings.json: {}",
                    settings_path.display()
                );
            } else {
                let new_content = serde_json::to_string_pretty(&settings)?;
                fs::write(&settings_path, new_content)?;
            }
            removed.push("Gemini settings.json: removed RTK hook entry".to_string());
        }
    }

    if verbose > 0 && !removed.is_empty() {
        eprintln!("Gemini artifacts removed");
    }

    Ok(removed)
}

/// Takes RTK's hook out of the BeforeTool groups; true when one was removed.
fn remove_rtk_hooks(settings: &mut serde_json::Value, installed: &str) -> bool {
    let bt_pointer = format!("/hooks/{}", BEFORE_TOOL_KEY);
    let Some(arr) = settings
        .pointer_mut(&bt_pointer)
        .and_then(|v| v.as_array_mut())
    else {
        return false;
    };
    let mut removed = false;
    arr.retain_mut(|group| {
        let Some(hooks) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
            return true;
        };
        let before = hooks.len();
        hooks.retain(|hook| !is_rtk_hook_entry(hook, installed));
        if hooks.len() == before {
            return true;
        }
        removed = true;
        // A group that held only RTK's hook goes with it.
        !hooks.is_empty()
    });
    removed
}

/// Returns true if Gemini CLI RTK hook is configured in settings.json.
pub fn is_configured() -> bool {
    let Ok(gemini_dir) = resolve_gemini_dir() else {
        return false;
    };
    let settings_path = gemini_dir.join(SETTINGS_JSON);
    fs::read_to_string(&settings_path)
        .ok()
        .and_then(|content| from_json_str::<serde_json::Value>(&content).ok())
        .is_some_and(|settings| {
            let installed = gemini_dir.join(HOOKS_SUBDIR).join(GEMINI_HOOK_FILE);
            configured_in(&settings, &installed.to_string_lossy())
        })
}

/// True when a shell-matching BeforeTool group in `settings` runs RTK's hook.
fn configured_in(settings: &serde_json::Value, installed: &str) -> bool {
    let before_tool_pointer = format!("/hooks/{}", BEFORE_TOOL_KEY);
    settings
        .pointer(&before_tool_pointer)
        .and_then(|hooks| hooks.as_array())
        .is_some_and(|arr| {
            arr.iter().any(|h| {
                let matcher = h.get("matcher").and_then(|m| m.as_str());
                let matches_shell = matcher == Some("run_shell_command") || matcher == Some("*");
                matches_shell && group_has_rtk_hook(h, installed)
            })
        })
}

#[cfg(all(test, feature = "engine"))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_patch_gemini_settings_malformed_json_warns_without_overwriting() {
        // A parse failure (not a BOM) must not abort `rtk init` — by the time
        // patch_gemini_settings runs, run_gemini has already written the hook
        // script, GEMINI.md, and the integrity baseline, so aborting here
        // would leave a half-installed state with no summary. Treat it like
        // PatchMode::Skip: warn and continue, and critically still never
        // overwrite the unparsed file. Unlike Skip (and every other
        // non-patch outcome), this path returns Ok(true) so run_gemini's
        // final summary can honestly say settings.json was NOT patched
        // instead of implying full success.
        let tmp = TempDir::new().unwrap();
        let gemini_dir = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini_dir).unwrap();
        let settings_path = gemini_dir.join(SETTINGS_JSON);
        let original = "{\"model\": \"foo\", \"mcpServers\": {},"; // trailing comma
        fs::write(&settings_path, original).unwrap();

        let result = patch_gemini_settings(
            &gemini_dir,
            Path::new("/fake/hook/path"),
            PatchMode::Auto,
            InitContext::default(),
        );

        assert!(
            result.unwrap(),
            "parse failure must be signaled so the summary can say settings.json was NOT patched"
        );

        let content = fs::read_to_string(&settings_path).unwrap();
        assert_eq!(
            content, original,
            "settings.json must not be overwritten when parsing fails"
        );
    }

    const HOOK: &str = "/home/u/.gemini/hooks/rtk-hook-gemini.sh";

    fn group(commands: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "matcher": "run_shell_command",
            "hooks": commands
                .iter()
                .map(|c| serde_json::json!({"type": "command", "command": c}))
                .collect::<Vec<_>>(),
        })
    }

    fn settings_with(groups: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({ "hooks": { (BEFORE_TOOL_KEY): groups } })
    }

    fn before_tool(settings: &serde_json::Value) -> Vec<serde_json::Value> {
        settings
            .pointer(&format!("/hooks/{BEFORE_TOOL_KEY}"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn install_adds_its_hook_beside_a_user_command_that_mentions_rtk() {
        let tmp = TempDir::new().unwrap();
        let gemini_dir = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini_dir).unwrap();
        let settings_path = gemini_dir.join(SETTINGS_JSON);
        let user = group(&["audit; rtk hook gemini"]);
        fs::write(
            &settings_path,
            settings_with(vec![user.clone()]).to_string(),
        )
        .unwrap();

        patch_gemini_settings(
            &gemini_dir,
            Path::new(HOOK),
            PatchMode::Auto,
            InitContext::default(),
        )
        .unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
        assert_eq!(before_tool(&written), vec![user, group(&[HOOK])]);
    }

    #[test]
    fn install_leaves_settings_alone_when_its_hook_is_there() {
        let tmp = TempDir::new().unwrap();
        let gemini_dir = tmp.path().join(".gemini");
        fs::create_dir_all(&gemini_dir).unwrap();
        let settings_path = gemini_dir.join(SETTINGS_JSON);
        let original = settings_with(vec![group(&[HOOK])]).to_string();
        fs::write(&settings_path, &original).unwrap();

        patch_gemini_settings(
            &gemini_dir,
            Path::new(HOOK),
            PatchMode::Auto,
            InitContext::default(),
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);
    }

    #[test]
    fn uninstall_removes_only_rtks_hook() {
        let user = group(&["audit; rtk hook gemini"]);
        let mixed_user = serde_json::json!({"type": "command", "command": "rtk-audit.sh"});
        let mut mixed = group(&["rtk-audit.sh", HOOK]);
        let mut settings = settings_with(vec![user.clone(), group(&[HOOK]), mixed.clone()]);

        assert!(remove_rtk_hooks(&mut settings, HOOK));

        mixed["hooks"] = serde_json::json!([mixed_user]);
        assert_eq!(before_tool(&settings), vec![user, mixed]);
    }

    #[test]
    fn uninstall_reports_nothing_removed_for_user_commands_only() {
        let original = settings_with(vec![
            group(&["audit; rtk hook gemini"]),
            group(&["/opt/rtk/bin/lint"]),
        ]);
        let mut settings = original.clone();

        assert!(!remove_rtk_hooks(&mut settings, HOOK));
        assert_eq!(settings, original);
    }

    #[test]
    fn configured_only_by_rtks_own_hook() {
        assert!(!configured_in(
            &settings_with(vec![group(&["audit; rtk hook gemini"])]),
            HOOK
        ));
        assert!(configured_in(&settings_with(vec![group(&[HOOK])]), HOOK));
        assert!(configured_in(
            &settings_with(vec![group(&["rtk hook gemini"])]),
            HOOK
        ));
        assert!(configured_in(
            &settings_with(vec![group(&["notify-send hi", HOOK])]),
            HOOK
        ));
        assert!(!configured_in(
            &settings_with(vec![group(&["/opt/my-rtk-hook-gemini.sh"])]),
            HOOK
        ));
        assert!(!configured_in(
            &settings_with(vec![group(&[
                "/usr/bin/env bash /opt/x/rtk-hook-gemini.sh"
            ])]),
            HOOK
        ));
        // An unspaced Windows path from an earlier home is one shell word.
        assert!(configured_in(
            &settings_with(vec![group(&[r"D:\old\.gemini\hooks\rtk-hook-gemini.sh"])]),
            HOOK
        ));
        // rtk init writes its script's path unquoted; the shell reading drops
        // a Windows path's separators and splits a path with a space.
        for installed in [
            r"C:\Users\u\.gemini\hooks\rtk-hook-gemini.sh",
            r"C:\Users\Jo Bloggs\.gemini\hooks\rtk-hook-gemini.sh",
            "/home/jo bloggs/.gemini/hooks/rtk-hook-gemini.sh",
        ] {
            assert!(
                configured_in(&settings_with(vec![group(&[installed])]), installed),
                "{installed}"
            );
        }
    }

    /// An install under a home path with a space: install adds no second
    /// copy and uninstall removes it.
    #[test]
    fn spaced_home_path_install_and_uninstall() {
        let tmp = TempDir::new().unwrap();
        let gemini_dir = tmp.path().join("jo bloggs").join(".gemini");
        fs::create_dir_all(&gemini_dir).unwrap();
        let hook = gemini_dir.join(HOOKS_SUBDIR).join(GEMINI_HOOK_FILE);
        let installed = hook.to_string_lossy().to_string();
        let settings_path = gemini_dir.join(SETTINGS_JSON);
        let original = settings_with(vec![group(&[&installed])]).to_string();
        fs::write(&settings_path, &original).unwrap();

        patch_gemini_settings(&gemini_dir, &hook, PatchMode::Auto, InitContext::default()).unwrap();
        assert_eq!(fs::read_to_string(&settings_path).unwrap(), original);

        let mut settings: serde_json::Value = serde_json::from_str(&original).unwrap();
        assert!(remove_rtk_hooks(&mut settings, &installed));
        assert!(before_tool(&settings).is_empty());
    }
}

#[cfg(test)]
#[path = "gemini_coverage_gen.rs"]
mod coverage_gen_src_hooks_init_gemini;
