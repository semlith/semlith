//! What `semlith start` does about a store something else already holds.
//!
//! Two answers, and the whole of this file is keeping them apart: a store a
//! live daemon holds is the daemon the user wanted and exits 0 with its URL, a
//! store held by anything else is the conflict it always was and keeps its
//! error chain and its non-zero exit.
//!
//! No daemon is spawned and no model is downloaded. The test process itself
//! takes the store's write lock and writes the discovery file beside it, which
//! is exactly the pair `daemon::held_by_daemon` reads — and it lets the dead
//! pid be the *only* difference between the two halves.
//!
//! `#![cfg(unix)]`, for two reasons that are both about the second half: the
//! discovery file has to be written 0600 or it fails the owner check instead of
//! the liveness one, and `daemon::alive` has no pid to ask on Windows and
//! answers `true` for every pid there is.
#![cfg(unix)]

use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use semlith::lock::StoreLock;

/// A dead pid. The same one `lock.rs`'s own stale-file test uses; above
/// `kern.maxproc` on macOS and `pid_max` on stock Linux alike.
const DEAD: u32 = 999_999;

/// A store home with one store directory in it, and nothing of the developer's.
///
/// The store lives under the home because `home::Registry::trusts` trusts what
/// it finds there without a registry entry, and `Discovery::read` refuses to
/// read a `daemon.json` in a store this user has not trusted.
fn sandbox(tag: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("semlith-start-{tag}-"))
        .tempdir()
        .expect("a temporary directory");
    let home = dir.path().join("home");
    let store = home.join("stores").join(tag);
    std::fs::create_dir_all(&store).unwrap();
    (dir, home, store)
}

/// Write the discovery file a daemon leaves beside the lock.
///
/// 0600 as the daemon writes it, so the owner check passes and `pid` is the
/// only thing either half of this file changes.
fn discovery(store: &Path, pid: u32, port: u16, token: &str, version: &str) {
    use std::io::Write;
    let path = store.join("daemon.json");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .expect("a discovery file");
    write!(
        file,
        r#"{{"pid":{pid},"port":{port},"token":"{token}","version":"{version}"}}"#
    )
    .unwrap();
}

fn start(home: &Path, store: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_semlith"))
        .env("SEMLITH_HOME", home)
        .env("HOME", home)
        .env_remove("SEMLITH_STORE")
        .env_remove("SEMLITH_PORT")
        .arg("--store")
        .arg(store)
        .arg("start")
        .arg("--port")
        .arg("0")
        .output()
        .expect("semlith start runs")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_store_a_live_daemon_holds_is_not_a_failure() {
    let (_dir, home, store) = sandbox("live");
    let token = "a".repeat(64);
    let held = StoreLock::acquire(&store).expect("the test process takes the lock");
    // This process: alive by construction, which is the point.
    discovery(&store, std::process::id(), 7365, &token, "9.9.9");

    let out = start(&home, &store);
    let all = text(&out);
    drop(held);

    assert!(out.status.success(), "exit was not 0:\n{all}");
    assert!(
        !all.contains("Error:") && !all.contains("Caused by:"),
        "an error chain was printed for a daemon that is running:\n{all}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout
            .lines()
            .any(|l| l.trim() == format!("http://127.0.0.1:7365/?token={token}")),
        "the portal URL is not on a line of its own:\n{all}"
    );
    for fact in [
        store.display().to_string(),
        format!("pid {}", std::process::id()),
        "port 7365".to_string(),
        "semlith 9.9.9".to_string(),
    ] {
        assert!(
            stdout.contains(&fact),
            "{fact} is not in the output:\n{all}"
        );
    }
}

#[test]
fn a_dead_daemon_keeps_the_error_it_always_had() {
    let (_dir, home, store) = sandbox("dead");
    let held = StoreLock::acquire(&store).expect("the test process takes the lock");
    // Everything a live daemon writes except a pid that answers.
    discovery(&store, DEAD, 7365, &"b".repeat(64), "9.9.9");

    let out = start(&home, &store);
    let all = text(&out);
    drop(held);

    assert!(
        !out.status.success(),
        "a store held by no daemon exited 0:\n{all}"
    );
    assert!(
        all.contains("Error:") && all.contains("Caused by:"),
        "the error chain did not print:\n{all}"
    );
    assert!(
        all.contains("cannot be opened by the daemon")
            && all.contains("is held by a running `semlith start`"),
        "the wording changed:\n{all}"
    );
    assert!(
        !all.contains("?token="),
        "a URL was offered for a daemon that is not there:\n{all}"
    );
}
