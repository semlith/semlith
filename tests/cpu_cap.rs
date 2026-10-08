//! The CPU cap holds semlith's own CPU under it while it indexes
//! (0.37.0-rc.6), measured from outside the process the way a person watching
//! `top` would: its CPU time read once a second, as a share of every core.
//!
//! Ignored: it needs the embedding model in the model cache and about two
//! minutes. Run it with the other measurements:
//!
//! ```sh
//! cargo test --release --test cpu_cap -- --ignored --nocapture --test-threads=1
//! ```
#![cfg(unix)]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A child's CPU time so far, in seconds.
fn cpu_seconds(pid: u32) -> Option<f64> {
    if cfg!(target_os = "linux") {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // After the command name, which is in parentheses and may hold spaces.
        let rest = &stat[stat.rfind(')')? + 2..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        // utime and stime are fields 14 and 15 of the whole line.
        let ticks: f64 =
            fields.get(11)?.parse::<f64>().ok()? + fields.get(12)?.parse::<f64>().ok()?;
        // SAFETY: sysconf reads a constant.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        Some(ticks / hz)
    } else {
        // `ps` prints [[dd-]hh:]mm:ss.cc on macOS.
        let out = Command::new("ps")
            .args(["-o", "time=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut seconds = 0.0;
        for part in text.trim().split(':') {
            seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
        }
        Some(seconds)
    }
}

fn index(home: &Path, folder: &Path, cap: Option<&str>) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
    command
        .args(["index", &folder.display().to_string()])
        .env("SEMLITH_HOME", home)
        // A CPU-only run: on a Mac the Neural Engine would take the work and
        // leave the CPU lane nothing to pace.
        .env("SEMLITH_ACCEL", "cpu")
        // Every core, so an uncapped run would go past any cap tested here.
        .env(
            "SEMLITH_EMBED_THREADS",
            std::thread::available_parallelism()
                .map_or(4, |n| n.get())
                .to_string(),
        )
        .env("SEMLITH_VECTOR_CACHE_MB", "0")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match cap {
        Some(cap) => command.env("SEMLITH_CPU_CAP", cap),
        None => command.env_remove("SEMLITH_CPU_CAP"),
    };
    command.spawn().unwrap()
}

/// Shares of all cores, one a second, after a five-second warm-up, for at
/// most `samples` seconds or until the child exits.
fn shares(child: &mut Child, samples: usize) -> Vec<f64> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    std::thread::sleep(Duration::from_secs(5));
    let mut out = Vec::new();
    let (mut at, mut from) = (Instant::now(), cpu_seconds(child.id()).unwrap_or(0.0));
    while out.len() < samples && child.try_wait().unwrap().is_none() {
        std::thread::sleep(Duration::from_secs(1));
        let Some(now) = cpu_seconds(child.id()) else {
            break;
        };
        let span = at.elapsed().as_secs_f64();
        out.push((now - from) / (span * cores) * 100.0);
        (at, from) = (Instant::now(), now);
    }
    out
}

fn corpus() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
#[ignore = "needs the embedding model; a measurement"]
fn an_index_stays_under_its_cpu_cap() {
    for cap in [25u32, 50] {
        let home = tempfile::tempdir().unwrap();
        let mut child = index(home.path(), &corpus(), Some(&cap.to_string()));
        let got = shares(&mut child, 30);
        let _ = child.kill();
        let _ = child.wait();
        assert!(got.len() >= 10, "cap {cap}: only {} samples", got.len());
        let under = got.iter().filter(|s| **s <= f64::from(cap) + 5.0).count();
        eprintln!(
            "cap {cap} %: {under} of {} samples under cap + 5: {got:.0?}",
            got.len()
        );
        assert!(
            under * 100 >= got.len() * 95,
            "cap {cap} %: {under} of {} samples under cap + 5: {got:.0?}",
            got.len()
        );
    }
}

#[test]
#[ignore = "needs the embedding model; a measurement"]
fn a_zero_cap_pauses_and_a_raised_cap_resumes() {
    let home = tempfile::tempdir().unwrap();
    // Saved rather than from the environment, so it can be raised mid-run.
    std::fs::write(
        home.path().join("settings.json"),
        r#"{"cpu_cap_percent":0}"#,
    )
    .unwrap();
    let mut child = index(home.path(), &corpus(), None);
    let paused = shares(&mut child, 5);
    eprintln!("cap 0 %: {paused:.1?}");
    assert!(paused.iter().all(|s| *s < 5.0), "paused: {paused:.1?}");
    std::fs::write(
        home.path().join("settings.json"),
        r#"{"cpu_cap_percent":100}"#,
    )
    .unwrap();
    let resumed = shares(&mut child, 5);
    let _ = child.kill();
    let _ = child.wait();
    eprintln!("cap 100 %: {resumed:.1?}");
    assert!(resumed.iter().any(|s| *s > 20.0), "resumed: {resumed:.1?}");
}

#[test]
#[ignore = "needs the embedding model; a measurement"]
fn a_capped_run_makes_the_same_store_as_an_uncapped_one() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/common");
    let mut counts = Vec::new();
    for cap in [None, Some("50")] {
        let home = tempfile::tempdir().unwrap();
        let mut child = index(home.path(), &folder, cap);
        assert!(child.wait().unwrap().success());
        let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg("stats")
            .env("SEMLITH_HOME", home.path())
            .output()
            .unwrap();
        // The files, chunks and variants lines.
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let lines: Vec<String> = text
            .lines()
            .filter(|l| {
                ["files", "chunks", "variants"]
                    .iter()
                    .any(|k| l.starts_with(k))
            })
            .map(str::to_string)
            .collect();
        counts.push(lines);
    }
    assert_eq!(counts[0].len(), 3, "{counts:?}");
    assert_eq!(counts[0], counts[1]);
}
