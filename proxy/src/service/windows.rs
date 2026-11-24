//! Windows Task Scheduler, user-level (no admin): an "at log on" task running
//! with limited rights, plus a repeating trigger that relaunches a proxy
//! that died (a no-op while it is alive). `schtasks` has no native way to pass
//! individual environment variables to a task, so the task runs a small
//! `cmd /C` wrapper that sets `RTK_PROXY_CONFIG` and redirects stdout/stderr
//! into the log file itself (there's no journald/launchd-style log capture
//! here to rely on).

use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

pub const MECHANISM: &str = "Task Scheduler (log-on trigger, relaunched when it exits)";

use super::windows_task::{Account, LEGACY_TASK_NAME, decode_output, ours, sid_from_whoami};

fn run_command(program: &str, args: &[&OsStr]) -> Result<Output> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {program}"))
}

fn command_detail(out: &Output) -> String {
    let msg = decode_output(if out.stderr.is_empty() {
        &out.stdout
    } else {
        &out.stderr
    });
    let msg = msg.trim();
    if msg.is_empty() {
        String::new()
    } else {
        format!(": {msg}")
    }
}

fn current_user() -> Option<String> {
    let name = std::env::var("USERNAME").ok().filter(|n| !n.is_empty())?;
    Some(match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
        _ => name,
    })
}

/// This account's task name. Without `USERNAME` it would be the shared legacy
/// name, which may be another account's task, so nothing is installed or
/// removed.
fn task_name() -> Result<String> {
    let user = current_user()
        .context("USERNAME is not set, so the task cannot be named for this account")?;
    Ok(super::windows_task::task_name(Some(&user)))
}

/// Whether this user can see a task called `name`. Another account's task
/// answers "access denied", which reads as absent: it is not ours to manage.
fn task_exists(name: &str) -> Result<bool> {
    task_exists_with(name, &mut run_command)
}

fn task_exists_with(
    name: &str,
    run: &mut impl FnMut(&str, &[&OsStr]) -> Result<Output>,
) -> Result<bool> {
    let out = run(
        "schtasks",
        &[OsStr::new("/Query"), OsStr::new("/TN"), OsStr::new(name)],
    )?;
    Ok(out.status.success())
}

fn delete_task(name: &str) -> Result<()> {
    delete_task_with(name, &mut run_command)
}

fn delete_task_with(
    name: &str,
    run: &mut impl FnMut(&str, &[&OsStr]) -> Result<Output>,
) -> Result<()> {
    let out = run(
        "schtasks",
        &[
            OsStr::new("/Delete"),
            OsStr::new("/TN"),
            OsStr::new(name),
            OsStr::new("/F"),
        ],
    )?;
    if !out.status.success() {
        bail!(
            "schtasks /Delete failed ({}){}",
            out.status,
            command_detail(&out)
        );
    }
    Ok(())
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn current_account_with(
    run: &mut impl FnMut(&str, &[&OsStr]) -> Result<Output>,
) -> Option<Account> {
    let sid = run(
        "whoami",
        &[
            OsStr::new("/user"),
            OsStr::new("/fo"),
            OsStr::new("csv"),
            OsStr::new("/nh"),
        ],
    )
    .ok()
    .filter(|out| out.status.success())
    .and_then(|out| sid_from_whoami(&String::from_utf8_lossy(&out.stdout)));
    Some(Account {
        sid,
        domain: env_nonempty("USERDOMAIN"),
        name: env_nonempty("USERNAME")?,
        computer: env_nonempty("COMPUTERNAME"),
    })
}

/// Whether the task called `name` runs as us. Being able to see it is not
/// enough: an elevated terminal sees every account's tasks, and two accounts'
/// names can map to one task name, so the task's principal must be us.
fn owns_task(name: &str) -> Result<bool> {
    owns_task_with(name, &mut run_command)
}

fn owns_task_with(
    name: &str,
    run: &mut impl FnMut(&str, &[&OsStr]) -> Result<Output>,
) -> Result<bool> {
    let out = run(
        "schtasks",
        &[
            OsStr::new("/Query"),
            OsStr::new("/TN"),
            OsStr::new(name),
            OsStr::new("/XML"),
        ],
    )?;
    let Some(xml) = out.status.success().then(|| decode_output(&out.stdout)) else {
        return Ok(false);
    };
    Ok(ours(Some(&xml), current_account_with(run).as_ref()))
}

/// Why a visible task with our name is left alone: its principal could not be
/// tied to this account (another account's, or `whoami` or the principal could
/// not be read), with the command that removes it if it is ours after all.
fn unconfirmed(name: &str) -> String {
    format!(
        "the task {name} could not be confirmed as this account's \
         (if it is yours, remove it with: schtasks /Delete /TN \"{name}\" /F)"
    )
}

/// Our own task under the pre-per-user name would launch a second proxy every
/// minute next to the new one, so it goes when we install or uninstall.
fn has_own_legacy_task(name: &str) -> Result<bool> {
    has_own_legacy_task_with(name, &mut run_command)
}

fn has_own_legacy_task_with(
    name: &str,
    run: &mut impl FnMut(&str, &[&OsStr]) -> Result<Output>,
) -> Result<bool> {
    Ok(name != LEGACY_TASK_NAME && owns_task_with(LEGACY_TASK_NAME, run)?)
}

pub fn install(bin: &Path, config_path: &Path, log_path: &Path) -> Result<PathBuf> {
    install_with(
        bin,
        config_path,
        log_path,
        &super::state_dir()?,
        &mut run_command,
    )
}

fn install_with(
    bin: &Path,
    config_path: &Path,
    log_path: &Path,
    state_dir: &Path,
    run: &mut impl FnMut(&str, &[&OsStr]) -> Result<Output>,
) -> Result<PathBuf> {
    let name = task_name()?;
    if task_exists_with(&name, run)? && !owns_task_with(&name, run)? {
        bail!("{}; not replacing it", unconfirmed(&name));
    }
    let xml =
        super::windows_task::definition(bin, config_path, log_path, current_user().as_deref());
    let xml_path = state_dir.join("rtk-proxy-task.xml");
    std::fs::write(&xml_path, super::windows_task::encode(&xml))
        .with_context(|| format!("writing {}", xml_path.display()))?;
    let args = [
        OsStr::new("/Create"),
        OsStr::new("/F"),
        OsStr::new("/TN"),
        OsStr::new(&name),
        OsStr::new("/XML"),
        xml_path.as_os_str(),
    ];
    let out = run("schtasks", &args);
    let _ = std::fs::remove_file(&xml_path);
    let out = out?;
    if !out.status.success() {
        bail!(
            "schtasks /Create failed ({}){}",
            out.status,
            command_detail(&out)
        );
    }
    if has_own_legacy_task_with(&name, run)? {
        delete_task_with(LEGACY_TASK_NAME, run)?;
    }
    // schtasks has no on-disk definition file the caller can inspect directly;
    // report the task name in its place for the CLI message.
    Ok(PathBuf::from(name))
}

pub fn uninstall() -> Result<bool> {
    let name = task_name()?;
    let mut removed = false;
    if owns_task(&name)? {
        delete_task(&name)?;
        removed = true;
    } else if task_exists(&name)? {
        bail!("{}; not removing it", unconfirmed(&name));
    }
    if has_own_legacy_task(&name)? {
        delete_task(LEGACY_TASK_NAME)?;
        removed = true;
    }
    Ok(removed)
}

pub fn is_installed() -> Result<bool> {
    let name = task_name()?;
    Ok(owns_task(&name)? || has_own_legacy_task(&name)?)
}

pub fn logs() -> Result<()> {
    let log_path = super::log_file_path()?;
    if !log_path.exists() {
        bail!(
            "no log file yet at {} — has the service been started?",
            log_path.display()
        );
    }
    poll_tail(&log_path)
}

/// Minimal `tail -f` fallback: Windows has no built-in equivalent. Prints
/// the current content, then polls for appended bytes until interrupted.
fn poll_tail(path: &Path) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).context("reading log file")?;
    print!("{buf}");

    let mut pos = file
        .stream_position()
        .context("reading log file position")?;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let len = file.metadata().context("reading log file metadata")?.len();
        if len > pos {
            file.seek(SeekFrom::Start(pos))
                .context("seeking log file")?;
            let mut chunk = String::new();
            file.read_to_string(&mut chunk)
                .context("reading appended log lines")?;
            print!("{chunk}");
            pos = file
                .stream_position()
                .context("reading log file position")?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn output(code: u32, stdout: &[u8]) -> Output {
        Output {
            status: ExitStatus::from_raw(code),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    fn run_install(
        state_dir: &Path,
        user: (&str, &str),
        run: &mut impl FnMut(&str, &[&OsStr]) -> Result<Output>,
    ) -> Result<PathBuf> {
        temp_env::with_vars(
            [
                ("USERNAME", Some(user.1)),
                ("USERDOMAIN", Some(user.0)),
                ("COMPUTERNAME", Some("DESK")),
            ],
            || {
                install_with(
                    Path::new(r"C:\bin\rtk-proxy.exe"),
                    Path::new(r"C:\cfg\rtk-proxy.toml"),
                    Path::new(r"C:\log\rtk-proxy.log"),
                    state_dir,
                    run,
                )
            },
        )
    }

    fn create_calls(calls: &[Vec<String>]) -> Vec<&[String]> {
        calls
            .iter()
            .filter(|call| call.get(1).is_some_and(|arg| arg == "/Create"))
            .map(Vec::as_slice)
            .collect()
    }

    fn record_call(calls: &mut Vec<Vec<String>>, program: &str, args: &[&OsStr]) {
        let mut entry = vec![program.to_string()];
        entry.extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));
        calls.push(entry);
    }

    #[test]
    fn install_omits_force_when_task_queries_fail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut calls = Vec::new();
        let res = run_install(dir.path(), ("AzureAD", "Patrick"), &mut |prog, args| {
            record_call(&mut calls, prog, args);
            let is_create = prog == "schtasks" && args.first() == Some(&OsStr::new("/Create"));
            Ok(output(u32::from(!is_create), b""))
        });

        assert_eq!(
            res.expect("install succeeds").as_path(),
            Path::new("rtk-proxy-AzureAD_Patrick")
        );
        let creates = create_calls(&calls);
        assert_eq!(creates.len(), 1);
        assert!(
            !creates[0].iter().any(|arg| arg == "/F"),
            "failed existence/ownership queries must not use /Create /F: {creates:?}"
        );
    }

    #[test]
    fn install_uses_force_only_when_task_ownership_is_confirmed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut calls = Vec::new();
        let res = run_install(dir.path(), ("AzureAD", "Patrick"), &mut |prog, args| {
            record_call(&mut calls, prog, args);
            if prog == "whoami" {
                return Ok(output(0, b"\"AzureAD\\Patrick\",\"S-1-12-1-111-222\"\r\n"));
            }
            if prog == "schtasks"
                && args.first() == Some(&OsStr::new("/Query"))
                && args.get(2) == Some(&OsStr::new("rtk-proxy-AzureAD_Patrick"))
            {
                return Ok(output(
                    0,
                    b"<Task><Principals><Principal><UserId>S-1-12-1-111-222</UserId></Principal></Principals></Task>",
                ));
            }
            let is_create = prog == "schtasks" && args.first() == Some(&OsStr::new("/Create"));
            Ok(output(u32::from(!is_create), b""))
        });

        assert!(res.is_ok(), "{res:?}");
        let creates = create_calls(&calls);
        assert_eq!(creates.len(), 1);
        assert!(
            creates[0].iter().any(|arg| arg == "/F"),
            "confirmed ownership should replace with /F: {creates:?}"
        );
    }

    #[test]
    fn install_refuses_visible_task_not_confirmed_as_ours() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut calls = Vec::new();
        let res = run_install(dir.path(), ("DESK", "a_b"), &mut |prog, args| {
            record_call(&mut calls, prog, args);
            if prog == "schtasks" && args.first() == Some(&OsStr::new("/Query")) {
                return Ok(output(
                    0,
                    b"<Task><Principals><Principal><UserId>DESK\\a b</UserId></Principal></Principals></Task>",
                ));
            }
            Ok(output(0, b""))
        });

        let msg = res.expect_err("visible foreign task must fail").to_string();
        assert!(msg.contains("could not be confirmed as this account's"), "{msg}");
        assert!(create_calls(&calls).is_empty(), "must not invoke /Create");
    }

    #[test]
    fn install_cleans_up_xml_and_errors_when_unforced_create_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut calls = Vec::new();
        let res = run_install(dir.path(), ("AzureAD", "Patrick"), &mut |prog, args| {
            record_call(&mut calls, prog, args);
            Ok(output(1, b""))
        });

        assert!(res.is_err());
        assert!(!dir.path().join("rtk-proxy-task.xml").exists());
        let creates = create_calls(&calls);
        assert_eq!(creates.len(), 1);
        assert!(!creates[0].iter().any(|arg| arg == "/F"));
    }
}
