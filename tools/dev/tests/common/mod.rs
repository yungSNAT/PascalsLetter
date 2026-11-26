//! Helpers shared by the parity tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub fn write_exe(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

pub fn real_tool(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| panic!("{name} not on PATH"))
}

/// Every file under `dir`, sorted, as `== path` followed by its bytes.
pub fn snapshot(dir: &Path) -> Vec<u8> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    let mut snap = Vec::new();
    for file in files {
        snap.extend_from_slice(
            format!("== {}\n", file.strip_prefix(dir).unwrap().display()).as_bytes(),
        );
        snap.extend_from_slice(&fs::read(&file).unwrap());
        snap.push(b'\n');
    }
    snap
}
