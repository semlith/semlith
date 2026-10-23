//! SEC-003 (2026-10-23 audit, 2026-10-23): `add` never goes through a proxy
//! named in the environment, so the address it checks is the address it
//! connects to.

#[allow(unused_imports)]
use super::*;

use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A listener standing in for a proxy, counting every connection it is
/// offered and dropping each, so a client that did connect fails fast rather
/// than waiting on an answer.
fn proxy() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            count.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    (port, seen)
}

/// `semlith add <url>` with `HTTPS_PROXY` at `proxy` and no other proxy
/// variable, so whatever the test runner's own environment says, the one
/// variable set is the one the test set.
fn add_through(proxy: u16, url: &str, allow_private: bool) -> (bool, String) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut cmd = semlith(&home);
    cmd.args(["add", url, "--name", "web"]);
    for var in [
        "ALL_PROXY",
        "all_proxy",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "NO_PROXY",
        "no_proxy",
        "SEMLITH_ADD_ALLOW_PRIVATE",
    ] {
        cmd.env_remove(var);
    }
    cmd.env("HTTPS_PROXY", format!("http://127.0.0.1:{proxy}"));
    if allow_private {
        cmd.env("SEMLITH_ADD_ALLOW_PRIVATE", "1");
    }
    let out = cmd.output().expect("semlith runs");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), said)
}

#[test]
fn sec003_a_private_name_is_refused_and_the_proxy_never_hears_of_it() {
    let (port, seen) = proxy();
    let (ok, said) = add_through(port, "https://localhost:9/notes.md", false);
    assert!(!ok, "add fetched a private name: {said}");
    assert!(said.contains("not on the public internet"), "{said}");
    assert_eq!(seen.load(Ordering::SeqCst), 0, "the proxy was connected to");
}

/// The case v0.45.1 got wrong. The connection's resolver refuses a loopback
/// proxy, so a proxy is only ever reached when its address passes; the
/// private opt-in lets the loopback listener play one that does. The target
/// is a port nothing listens on, so the direct connection fails at once and
/// the error has to say why the proxy was not tried.
#[test]
fn sec003_a_proxy_variable_is_never_connected_to() {
    let (port, seen) = proxy();
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (ok, said) = add_through(port, &format!("https://127.0.0.1:{closed}/notes.md"), true);
    assert!(!ok, "add fetched from a closed port: {said}");
    assert_eq!(seen.load(Ordering::SeqCst), 0, "add went through the proxy");
    assert!(
        said.contains("HTTPS_PROXY") && said.contains("directly"),
        "the failure does not say the proxy variable was ignored: {said}"
    );
}
