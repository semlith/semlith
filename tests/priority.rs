//! The daemon's priority follows its work: background while nothing embeds,
//! normal the moment something does, background again once the count has sat
//! at zero for the grace period.
//!
//! In-process, because `priority::manage` is what the daemon calls and the
//! state it changes is this process's own. One test, so nothing else in the
//! binary takes a guard while it measures.
//!
//! What "background" means differs: on macOS the process's scheduler priority,
//! read with `ps`; on Windows EcoQoS, at below-normal priority either way; on
//! Linux nothing process-wide, because the systemd unit sets the baseline, so
//! the edges are the manager's own state. From 0.32.0 an index thread also
//! takes its OS's bulk class, and on Linux that is `SCHED_BATCH`.

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
    below_normal_on_windows();

    // The lift alone is timed: reading the priority back spawns `ps` or
    // PowerShell, which on a Windows runner takes most of a second.
    let lifted = Instant::now();
    let guard = semlith::priority::embedding();
    let took = lifted.elapsed();
    assert!(took < Duration::from_millis(50), "the lift took {took:?}");
    assert!(!background(), "an embed pass lifts the process at once");
    below_normal_on_windows();

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

    index_threads_take_the_bulk_class();
}

/// Read the way an outside observer would: `ps` on macOS, which is what the
/// acceptance reads.
#[cfg(target_os = "macos")]
fn background() -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "pri=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim() == "4"
}

/// EcoQoS has no reader outside the process, so the manager's own state.
#[cfg(not(target_os = "macos"))]
fn background() -> bool {
    semlith::priority::snapshot()["state"] == "background"
}

/// Below-normal whether idle or embedding, read as Task Manager would.
#[cfg(windows)]
fn below_normal_on_windows() {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {}).PriorityClass", std::process::id()),
        ])
        .output()
        .expect("powershell");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "BelowNormal");
}

#[cfg(not(windows))]
fn below_normal_on_windows() {}

/// An index thread takes `SCHED_BATCH` and gives back what it had.
#[cfg(target_os = "linux")]
fn index_threads_take_the_bulk_class() {
    // SAFETY: 0 names this thread; the call only reads.
    let policy = || unsafe { libc::sched_getscheduler(0) };
    let before = policy();
    let guard = semlith::priority::indexing_thread();
    assert_eq!(policy(), libc::SCHED_BATCH);
    assert_eq!(
        semlith::priority::snapshot()["indexing_class"],
        "SCHED_BATCH"
    );
    drop(guard);
    assert_eq!(policy(), before);
}

#[cfg(not(target_os = "linux"))]
fn index_threads_take_the_bulk_class() {}
