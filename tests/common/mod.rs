//! Helpers shared by the test binaries that write a program and then run it.
//!
//! Each file includes this with `mod common;` and uses a subset of it.
#![allow(dead_code)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// Puts a copy of `from` at `to`, mode 0755, without this process ever holding
/// `to` open for writing.
///
/// Linux refuses to exec a file while any process holds it open for writing
/// (ETXTBSY, "Text file busy"). Tests in one binary run on parallel threads, and
/// a thread that spawns a process forks a child holding a copy of every open
/// descriptor until that child execs. A write handle open in this process at
/// that moment outlives our own `close`, so closing, syncing or renaming before
/// exec cannot help: the leaked handle is on the same inode. Having `cp` write
/// the file keeps the only write handle in a process nothing else forks from,
/// and it is closed once `cp` has exited. #135.
pub fn copy_executable(from: &Path, to: &Path) {
    let status = Command::new("cp")
        .arg(from)
        .arg(to)
        .status()
        .expect("running cp");
    assert!(
        status.success(),
        "cp {} {} failed",
        from.display(),
        to.display()
    );
    std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A script at `path` with `body`, written the way [`copy_executable`] says.
///
/// The body goes through a sibling file first. That file may be held open by
/// a forked child for a moment, but nothing execs it; `cp` only reads it.
pub fn write_executable(path: &Path, body: &str) {
    let mut source = path.as_os_str().to_owned();
    source.push(".body");
    std::fs::write(&source, body).unwrap();
    copy_executable(Path::new(&source), path);
    std::fs::remove_file(&source).unwrap();
}
