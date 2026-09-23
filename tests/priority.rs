//! The daemon's priority follows its work: background while nothing embeds,
//! normal the moment something does, background again once the count has sat
//! at zero for the grace period.
//!
//! In-process, because `priority::manage` is what the daemon calls and the
//! state it changes is this process's own. One test, so nothing else in the
//! binary takes a guard while it measures.

#[cfg(any(target_os = "macos", windows))]
#[test]
fn priority_follows_the_embedding_count() {
    use std::time::{Duration, Instant};

    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = std::sync::Arc::clone(&lines);
    assert!(semlith::priority::manage(move |line| seen
        .lock()
        .unwrap()
        .push(line.to_string())));
    assert!(background(), "the daemon starts in background state");

    let lifted = Instant::now();
    let guard = semlith::priority::embedding();
    assert!(!background(), "an embed pass lifts the process at once");
    assert!(lifted.elapsed() < Duration::from_millis(50));

    // A second pass inside the first changes nothing.
    let inner = semlith::priority::embedding();
    drop(inner);
    assert!(!background(), "dropped while an outer pass was still going");

    drop(guard);
    assert!(!background(), "dropped inside the grace period");
    let deadline = Instant::now() + Duration::from_secs(1);
    while !background() {
        assert!(
            Instant::now() < deadline,
            "still normal a second after the last embed ended"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let log = lines.lock().unwrap().join("\n");
    assert!(
        log.contains("priority: background (idle at start)"),
        "{log}"
    );
    assert!(log.contains("priority: normal (embedding)"), "{log}");
    assert!(log.contains("priority: background (idle)"), "{log}");
    let snapshot = semlith::priority::snapshot();
    assert_eq!(snapshot["state"], "background");
    assert_eq!(snapshot["switches"], 3);
}

/// Read the way an outside observer would: `ps` on macOS, which is what the
/// acceptance reads, and the priority class on Windows.
#[cfg(target_os = "macos")]
fn background() -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "pri=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim() == "4"
}

#[cfg(windows)]
fn background() -> bool {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {}).PriorityClass", std::process::id()),
        ])
        .output()
        .expect("powershell");
    String::from_utf8_lossy(&out.stdout).trim() == "BelowNormal"
}

/// Linux is not managed, and says so rather than pretending.
#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn linux_is_left_at_normal_priority() {
    assert!(!semlith::priority::manage(|_| {}));
    let _guard = semlith::priority::embedding();
    assert_eq!(semlith::priority::snapshot()["managed"], false);
}
