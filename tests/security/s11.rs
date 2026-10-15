//! S11: idle or slow connections cannot lock a token holder out.
//!
//! Before the fix every accepted socket counted against a 32-connection cap and
//! waited 30 s for its first byte, and a socket that had sent one byte held a
//! worker for as long as it kept dribbling. 32 idle sockets were enough to get
//! a token holder a 503.

#[allow(unused_imports)]
use super::*;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How many idle sockets the flood opens: 300, the audit's number, unless this
/// process may not hold that many descriptors. macOS gives a process launched
/// from the GUI or by launchd a soft limit of 256, and a test that died of
/// EMFILE would say nothing about the daemon. Scaled to what the limit leaves
/// after a margin for the test binary's own files; anything above eight (the
/// per-peer cap on idle connections) and 32 (the old global cap) still
/// exercises both. Windows has no such limit on sockets.
fn flood_size() -> usize {
    #[cfg(unix)]
    {
        let soft = Command::new("sh")
            .args(["-c", "ulimit -n"])
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<usize>()
                    .ok()
            })
            .unwrap_or(usize::MAX);
        let n = 300.min(soft.saturating_sub(64));
        if n < 300 {
            eprintln!("descriptor limit {soft}: flooding with {n} sockets instead of 300");
        }
        n
    }
    #[cfg(not(unix))]
    300
}

fn daemon() -> (tempfile::TempDir, Daemon) {
    let home = tempfile::tempdir().unwrap();
    let d = Daemon::start(home.path(), &[]);
    assert!(
        until(Duration::from_secs(30), || d.api("/api/about")
            != Value::Null),
        "the daemon never answered"
    );
    (home, d)
}

/// The acceptance criterion: 300 idle sockets, a few of them dribbling a
/// partial request head, and a token holder is still answered within 2 s.
#[test]
fn a_flood_of_idle_sockets_leaves_a_token_holder_answered() {
    let (_home, d) = daemon();
    let n = flood_size();

    let mut idle = Vec::with_capacity(n);
    for _ in 0..n {
        if let Ok(s) = TcpStream::connect(("127.0.0.1", d.port)) {
            idle.push(s);
        }
    }
    // Every tenth socket sends the start of a head and then one byte at a
    // time, which is what holds a worker rather than a waiting thread.
    let mut slow = Vec::new();
    for s in idle.iter().step_by(10) {
        let mut s = s.try_clone().unwrap();
        let _ = s.write_all(b"GET /api/about HTTP/1.1\r\nX-Slow: ");
        slow.push(s);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let dribbler = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for s in &mut slow {
                    let _ = s.write_all(b"a");
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    };

    let started = Instant::now();
    let (status, _) = d.send(
        "GET",
        "/api/about",
        &format!("Semlith-Token: {}\r\n", d.token),
        "",
    );
    let took = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let _ = dribbler.join();
    drop(idle);

    assert_eq!(
        status, 200,
        "with {n} idle sockets open the token holder got {status} after {took:?}"
    );
    assert!(
        took < Duration::from_secs(2),
        "with {n} idle sockets open the token holder waited {took:?}"
    );
}

/// A header line longer than 16 KiB is refused as soon as it passes the cap,
/// and the connection closes, rather than being read until a newline arrives.
#[test]
fn an_overlong_header_line_is_refused_and_closed() {
    let (_home, d) = daemon();
    let mut s = TcpStream::connect(("127.0.0.1", d.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut request = format!(
        "GET /api/about HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Long: ",
        d.port
    )
    .into_bytes();
    request.extend(std::iter::repeat_n(b'A', 20 * 1024));
    let _ = s.write_all(&request);

    let mut raw = Vec::new();
    let ended = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    assert!(
        status == 431 || status == 400,
        "an over-long header line got {status:?} ({ended:?}): {text:?}"
    );
    assert!(ended.is_ok(), "the connection was not closed: {ended:?}");
}

/// A head that never finishes is cut off at the deadline however steadily its
/// bytes arrive: one byte every 300 ms would otherwise never trip the 30 s
/// per-read timeout.
#[test]
fn a_dribbled_head_is_cut_off_at_the_deadline() {
    let (_home, d) = daemon();
    let s = TcpStream::connect(("127.0.0.1", d.port)).unwrap();
    let mut writer = s.try_clone().unwrap();
    let _ = writer.write_all(b"GET /api/about HTTP/1.1\r\nX-Slow: ");
    let stop = Arc::new(AtomicBool::new(false));
    let dribbler = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) && writer.write_all(b"a").is_ok() {
                std::thread::sleep(Duration::from_millis(300));
            }
        })
    };

    let started = Instant::now();
    let mut reader = s;
    reader
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let mut raw = Vec::new();
    let ended = reader.read_to_end(&mut raw);
    let took = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let _ = dribbler.join();

    // Closed (with a 408, or a reset because the dribbler was mid-write) well
    // before the read timeout, not left open for as long as bytes keep coming.
    assert!(
        took < Duration::from_secs(15),
        "a dribbled head was still open after {took:?} ({ended:?})"
    );
}
