//! The numbers behind 0.4.0's non-functional claims.
//!
//! "Event-driven, not polling" and "memory does not grow" are exactly the
//! claims that are true in the design and false in the code, so they are
//! measured here rather than asserted in a document. Run with:
//!
//! ```sh
//! cargo test --release --test measure -- --ignored --nocapture
//! ```
//!
//! Every measurement prints what it saw. The assertions are the contract's
//! thresholds; the printed numbers are the evidence.

#![cfg(unix)]

use semlith::Semlith;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Files in the measurement corpus. Large enough that a polling watcher would
/// be obvious in the CPU numbers and a whole-index rewrite is a real write.
const CORPUS_FILES: usize = 1000;

/// How long the watcher is left alone to prove it costs nothing when nothing
/// happens.
const IDLE_WINDOW: Duration = Duration::from_secs(60);

/// The contract's ceiling for a save becoming searchable, model already warm.
const LATENCY_BUDGET: Duration = Duration::from_secs(5);

#[test]
#[ignore = "takes minutes and downloads an embedding model on first run"]
fn measure_the_watcher() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();

    for i in 0..CORPUS_FILES {
        fs::write(
            corpus.path().join(format!("note_{i:04}.md")),
            format!(
                "Note {i}. Fermentation, ownership, retries and backoff, \
                 described at moderate length so the chunk is worth embedding.\n"
            ),
        )
        .unwrap();
    }

    println!("\n--- corpus: {CORPUS_FILES} files");

    let indexed = Instant::now();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_| {})
            .unwrap();
    }
    println!("initial index: {:.1}s", indexed.elapsed().as_secs_f32());

    let index_bytes = fs::metadata(store.path().join("index.tv")).unwrap().len();
    println!("index.tv: {} KB", index_bytes / 1024);

    let mut watcher = Watcher::start(store.path(), corpus.path());
    watcher.wait_until_watching();

    // --- idle cost -------------------------------------------------------
    let cpu_before = watcher.cpu_seconds();
    let rss_after_start = watcher.rss_kb();
    thread::sleep(IDLE_WINDOW);
    let idle_cpu = watcher.cpu_seconds() - cpu_before;
    println!(
        "idle: {idle_cpu:.2}s CPU over {}s, RSS {} MB",
        IDLE_WINDOW.as_secs(),
        rss_after_start / 1024
    );
    assert!(
        idle_cpu <= 1.0,
        "an idle watcher burned {idle_cpu:.2}s of CPU in {}s — that is a poller",
        IDLE_WINDOW.as_secs()
    );

    // --- edit to searchable ----------------------------------------------
    // The reader is warmed first: the criterion is about the watcher's
    // latency, not about a cold ONNX model in the process asking.
    let mut reader = Semlith::open(store.path(), None).unwrap();
    reader.quiet = true;
    reader.warm().unwrap();
    let _ = reader.search("fermentation and backoff", 3).unwrap();

    let query = "single owner freed when it goes out of scope";
    let started = Instant::now();
    fs::write(
        corpus.path().join("ownership.md"),
        "Ownership means each value has a single owner, and the compiler \
         frees it when that owner goes out of scope.\n",
    )
    .unwrap();

    let mut latency = None;
    while started.elapsed() < Duration::from_secs(60) {
        if reader
            .search(query, 5)
            .unwrap()
            .iter()
            .any(|h| h.path.ends_with("ownership.md"))
        {
            latency = Some(started.elapsed());
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let latency = latency.expect("the edit never became searchable");
    println!("edit to searchable: {:.2}s", latency.as_secs_f32());
    assert!(
        latency <= LATENCY_BUDGET,
        "an edit took {latency:?}, over the {LATENCY_BUDGET:?} budget"
    );

    // --- index writes per burst ------------------------------------------
    // Settle first. The latency loop above returns the instant the hit
    // appears, so a trailing batch from that same save would otherwise be
    // counted against the burst and read as a debounce failure.
    thread::sleep(Duration::from_secs(5));
    let before = generation(store.path());
    let burst = Instant::now();
    for i in 0..10 {
        fs::write(
            corpus.path().join("ownership.md"),
            format!("Ownership, revision {i}. Each value has one owner.\n"),
        )
        .unwrap();
    }
    thread::sleep(Duration::from_secs(6));
    let writes = generation(store.path()) - before;
    println!(
        "10 saves in {:.0}ms cost {writes} index write(s) of {} KB",
        burst.elapsed().as_millis(),
        index_bytes / 1024
    );
    assert!(
        writes <= 1,
        "a burst inside one debounce window cost {writes} index rewrites"
    );

    // --- memory across many events ---------------------------------------
    let rss_before_churn = watcher.rss_kb();
    for i in 0..100 {
        fs::write(
            corpus.path().join("churn.md"),
            format!("Revision {i} of a file being saved over and over.\n"),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(120));
    }
    thread::sleep(Duration::from_secs(5));
    let rss_after_churn = watcher.rss_kb();
    println!(
        "RSS: {} MB at start, {} MB after 100 edits",
        rss_before_churn / 1024,
        rss_after_churn / 1024
    );
    assert!(
        rss_after_churn < rss_before_churn + 200 * 1024,
        "RSS grew from {} MB to {} MB across 100 edits",
        rss_before_churn / 1024,
        rss_after_churn / 1024
    );

    watcher.stop();
}

/// The `index_generation` meta key, which counts index.tv rewrites.
fn generation(store: &Path) -> i64 {
    let s = Semlith::open(store, None).unwrap();
    semlith::store::get_meta(s.db(), "index_generation")
        .unwrap()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

struct Watcher {
    child: Child,
    store: std::path::PathBuf,
}

impl Watcher {
    fn start(store: &Path, corpus: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg("--store")
            .arg(store)
            .arg("watch")
            .arg(corpus)
            .arg("--quiet")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            child,
            store: store.to_path_buf(),
        }
    }

    /// The catch-up pass is over once the model is loaded and nothing more is
    /// being written — measured from the corpus being unchanged, so this is
    /// just a settle.
    fn wait_until_watching(&mut self) {
        let target = generation(&self.store);
        let deadline = Instant::now() + Duration::from_secs(120);
        while Instant::now() < deadline {
            thread::sleep(Duration::from_secs(2));
            if generation(&self.store) == target {
                thread::sleep(Duration::from_secs(3));
                return;
            }
        }
    }

    fn cpu_seconds(&self) -> f32 {
        let out = Command::new("ps")
            .args(["-o", "cputime=", "-p", &self.child.id().to_string()])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        parse_cputime(text.trim())
    }

    fn rss_kb(&self) -> u64 {
        let out = Command::new("ps")
            .args(["-o", "rss=", "-p", &self.child.id().to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0)
    }

    fn stop(mut self) {
        let _ = Command::new("kill")
            .arg("-INT")
            .arg(self.child.id().to_string())
            .status();
        let _ = self.child.wait();
    }
}

/// `ps -o cputime` gives `MM:SS.ss`, or `HH:MM:SS` once it is old enough.
fn parse_cputime(text: &str) -> f32 {
    let mut seconds = 0.0;
    for part in text.split(':') {
        seconds = seconds * 60.0 + part.parse::<f32>().unwrap_or(0.0);
    }
    seconds
}

#[test]
fn cputime_parses_both_shapes() {
    assert!((parse_cputime("0:02.13") - 2.13).abs() < 1e-3);
    assert!((parse_cputime("1:00:00") - 3600.0).abs() < 1e-3);
}
