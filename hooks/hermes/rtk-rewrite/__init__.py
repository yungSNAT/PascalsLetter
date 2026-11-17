"""Hermes plugin adapter for RTK command rewriting.

All rewrite logic lives in RTK's Rust ``rtk rewrite`` command; this module
only bridges Hermes ``pre_tool_call`` payloads to that command and fails open.
"""

import math
import os
import subprocess
import sys


ACCEPTED_REWRITE_RETURN_CODES = {0, 3}
EXPECTED_PASSTHROUGH_RETURN_CODES = {1, 2}
DEFAULT_REWRITE_TIMEOUT = 5
DEFAULT_REWRITE_BACKENDS = "local"
_rtk_path = None  # the rtk to run once looked up; "" when there is none
_rtk_missing_warned = False


def register(ctx):
    """Register the Hermes pre-tool callback."""
    if not _check_rtk():
        return

    ctx.register_hook("pre_tool_call", _pre_tool_call)


def _check_rtk():
    """Return whether the rtk binary is in PATH, warning once when missing."""
    global _rtk_path, _rtk_missing_warned

    if _rtk_path is None:
        _rtk_path = _find_rtk() or ""

    if not _rtk_path and not _rtk_missing_warned:
        _warn("rtk binary not found in PATH; Hermes hook not registered")
        _rtk_missing_warned = True

    return bool(_rtk_path)


def _find_rtk():
    """The rtk in the first absolute PATH entry that holds one, or None.

    Not a bare "rtk" or shutil.which: on Windows both search the working folder
    before PATH, so a checkout's rtk.exe would run, and a relative PATH entry is
    resolved against the working folder everywhere.
    """
    name = "rtk.exe" if os.name == "nt" else "rtk"
    for folder in os.environ.get("PATH", "").split(os.pathsep):
        candidate = os.path.join(folder, name)
        if os.path.isabs(folder) and os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    return None


def _rewrite_timeout():
    try:
        timeout = float(os.environ.get("RTK_HERMES_TIMEOUT", DEFAULT_REWRITE_TIMEOUT))
    except ValueError:
        return DEFAULT_REWRITE_TIMEOUT
    return timeout if math.isfinite(timeout) and timeout > 0 else DEFAULT_REWRITE_TIMEOUT


def _terminal_backend():
    """The backend Hermes runs this terminal call on, read as Hermes reads it."""
    try:
        from tools.terminal_scope import terminal_env
    except ImportError:  # Hermes releases before per-profile terminal scopes
        return os.environ.get("TERMINAL_ENV", "local")
    return terminal_env("TERMINAL_ENV", "local")


def _rewrite_backends():
    """Backends whose machine has rtk: RTK_HERMES_BACKENDS, comma-separated."""
    value = os.environ.get("RTK_HERMES_BACKENDS", "").strip() or DEFAULT_REWRITE_BACKENDS
    return {backend.strip().lower() for backend in value.split(",") if backend.strip()}


def _pre_tool_call(tool_name=None, args=None, **_kwargs):
    """Update the shared payload without overriding other hooks' directives."""
    try:
        if tool_name != "terminal" or not isinstance(args, dict):
            return

        command = args.get("command")
        if not isinstance(command, str) or not command.strip():
            return

        # A Docker, SSH or other sandbox runs the command where rtk may be missing.
        if _terminal_backend().strip().lower() not in _rewrite_backends():
            return

        try:
            result = subprocess.run(
                [_rtk_path, "rewrite", command],
                shell=False,
                stdin=subprocess.DEVNULL,
                timeout=_rewrite_timeout(),
                capture_output=True,
                text=True,
            )
        except subprocess.TimeoutExpired:
            _warn("rtk rewrite timed out")
            return

        if result.returncode not in ACCEPTED_REWRITE_RETURN_CODES:
            if result.returncode not in EXPECTED_PASSTHROUGH_RETURN_CODES:
                details = f"rtk rewrite failed with exit {result.returncode}"
                stderr = result.stderr.strip()
                if stderr:
                    details = f"{details}: {stderr}"
                _warn(details)
            return

        rewritten = result.stdout.strip()
        if rewritten and rewritten != command:
            # Hermes executes this shared payload, including on releases before
            # modify directives were supported. Newer releases then apply queued
            # directives in hook order. Returning a command directive here would
            # overwrite an earlier plugin's command that this hook cannot see.
            args["command"] = rewritten
    except Exception as e:
        _warn(str(e))
        return


def _warn(message):
    print(f"rtk: hermes plugin warning: {message}", file=sys.stderr)
