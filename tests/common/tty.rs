//! Runs a command with its standard input and output on a pseudo-terminal,
//! so code that checks for a terminal takes its terminal paths.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Run {
    pub screen: String,
    pub stderr: String,
    pub success: bool,
}

#[allow(unsafe_code)]
fn open() -> (File, File) {
    // SAFETY: plain libc calls on a descriptor this function owns; the
    // name buffer is large enough and NUL-terminated by `ptsname_r`.
    // nosemgrep: unsafe-block
    unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(fd >= 0, "posix_openpt");
        let master = File::from_raw_fd(fd);
        assert_eq!(libc::grantpt(fd), 0, "grantpt");
        assert_eq!(libc::unlockpt(fd), 0, "unlockpt");
        let size = libc::winsize {
            ws_row: 40,
            ws_col: 160,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(libc::ioctl(fd, libc::TIOCSWINSZ, &size), 0, "set size");
        let mut name = [0 as libc::c_char; 128];
        assert_eq!(libc::ptsname_r(fd, name.as_mut_ptr(), name.len()), 0);
        let path = std::ffi::CStr::from_ptr(name.as_ptr())
            .to_str()
            .expect("a UTF-8 terminal name")
            .to_owned();
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(path)
            .expect("open the terminal");
        (master, slave)
    }
}

pub fn run(cmd: Command, input: &[u8]) -> Run {
    run_answering(cmd, input, &[])
}

/// [`run`], answering in turn: once the screen shows `answers[i].0` after
/// the previous answer, it types `answers[i].1`.
pub fn run_answering(mut cmd: Command, input: &[u8], answers: &[(&str, &[u8])]) -> Run {
    let (mut master, slave) = open();
    cmd.stdin(Stdio::from(slave.try_clone().expect("clone terminal")))
        .stdout(Stdio::from(slave))
        .stderr(Stdio::piped())
        .env("CLICOLOR_FORCE", "1");
    let mut child = cmd.spawn().expect("spawn on a terminal");
    // The command holds this process's copies of the terminal; with them
    // closed, reading the master ends once the child exits.
    drop(cmd);
    master.write_all(input).expect("type the input");
    let mut answers: std::collections::VecDeque<(Vec<u8>, Vec<u8>)> = answers
        .iter()
        .map(|(seen, keys)| (seen.as_bytes().to_vec(), keys.to_vec()))
        .collect();
    let mut typist = master.try_clone().expect("clone the terminal");
    let reader = std::thread::spawn(move || {
        let mut screen = Vec::new();
        let mut from = 0;
        let mut buf = [0u8; 8192];
        while let Ok(n) = master.read(&mut buf) {
            if n == 0 {
                break;
            }
            screen.extend_from_slice(&buf[..n]);
            while let Some((seen, keys)) = answers.front() {
                let Some(at) = screen[from..]
                    .windows(seen.len())
                    .position(|w| w == seen.as_slice())
                else {
                    break;
                };
                typist.write_all(keys).expect("type the answer");
                from += at + seen.len();
                answers.pop_front();
            }
        }
        screen
    });
    let mut pipe = child.stderr.take().expect("stderr");
    let stderr = std::thread::spawn(move || {
        let mut text = String::new();
        pipe.read_to_string(&mut text).expect("read stderr");
        text
    });
    // A prompt that never shows leaves the child waiting on the terminal for
    // good: stop it, and fail with what it drew, rather than hang the suite.
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for rtk") {
            break Some(status);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let screen = reader.join().expect("terminal reader");
    let screen = String::from_utf8_lossy(&screen).replace("\r\n", "\n");
    let stderr = stderr.join().expect("stderr reader");
    let Some(status) = status else {
        panic!("still running after 120s, stopped; screen:\n{screen}\nstderr:\n{stderr}");
    };
    Run {
        screen,
        stderr,
        success: status.success(),
    }
}
