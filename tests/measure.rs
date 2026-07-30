//! The numbers behind the non-functional claims: 0.4.0's watcher, and what
//! 0.5.0's multi-store search costs.
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
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }
    println!("initial index: {:.1}s", indexed.elapsed().as_secs_f32());

    let index_bytes = index_bytes(store.path());
    println!("vector index: {} KB", index_bytes / 1024);

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

/// Files per store in the multi-store fixture. Three of these is the same order
/// of magnitude as the watcher corpus above, so the two sets of numbers are
/// comparable.
const STORE_FILES: usize = 300;

/// Searches per configuration. Enough that one slow first query does not decide
/// the number.
const SEARCHES: usize = 20;

/// What adding a store costs: query embeds, latency, and resident memory.
///
/// "One embed per model, not one per store" and "not three copies of the model"
/// are both true in the design and cheap to get wrong in the code, so they are
/// counted and measured rather than claimed.
#[test]
#[ignore = "takes minutes and downloads an embedding model on first run"]
fn measure_multi_store_search() {
    let mut corpora = Vec::new();
    let mut stores = Vec::new();
    for (n, topic) in ["fermentation", "ownership", "retries"].iter().enumerate() {
        let corpus = tempfile::tempdir().unwrap();
        for i in 0..STORE_FILES {
            fs::write(
                corpus.path().join(format!("note_{i:04}.md")),
                format!(
                    "Note {i} about {topic}. Described at moderate length so the \
                     chunk is worth embedding, and so a store of {STORE_FILES} \
                     files is a realistic size.\n"
                ),
            )
            .unwrap();
        }
        let store = corpus.path().join(".semlith");
        {
            let mut s = semlith::Semlith::open(&store, None).unwrap();
            s.quiet = true;
            s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
                .unwrap();
        }
        println!("store {n}: {STORE_FILES} files of {topic}");
        stores.push(store);
        corpora.push(corpus);
    }

    println!("\n--- query embeds per search");
    let mut fleet = semlith::fleet::Fleet::open(&stores).unwrap();
    fleet.quiet = true;
    fleet.warm().unwrap();
    let before = fleet.query_embeds();
    let _ = fleet.search("how does this work", 8).unwrap();
    let embeds = fleet.query_embeds() - before;
    println!("3 same-model stores, 1 search: {embeds} query embed(s)");
    assert_eq!(
        embeds, 1,
        "a search embedded the query {embeds} times over 3 stores that share a model"
    );

    println!("\n--- latency by store count");
    let mut previous: Option<Duration> = None;
    for count in 1..=stores.len() {
        let mut fleet = semlith::fleet::Fleet::open(&stores[..count]).unwrap();
        fleet.quiet = true;
        fleet.warm().unwrap();
        // Warm query: the first search of a process pays for the tokenizer's
        // first allocation, and that is not what is being measured.
        let _ = fleet.search("a warming query", 8).unwrap();

        let mut times = Vec::new();
        for i in 0..SEARCHES {
            let started = Instant::now();
            let _ = fleet
                .search(&format!("how is retry backoff described {i}"), 8)
                .unwrap();
            times.push(started.elapsed());
        }
        times.sort();
        let median = times[times.len() / 2];
        // Signed, because at a few milliseconds the difference between two and
        // three stores is inside the noise and can come out negative. Duration
        // subtraction panics on that, which is how this was found.
        let increment = match previous {
            Some(p) => format!(
                ", {:+.1}ms against {} store(s)",
                (median.as_secs_f64() - p.as_secs_f64()) * 1000.0,
                count - 1
            ),
            None => String::new(),
        };
        println!(
            "{count} store(s): median {:.1}ms over {SEARCHES} searches{increment}",
            median.as_secs_f64() * 1000.0
        );
        previous = Some(median);
    }

    println!("\n--- resident memory of a reader");
    // Real server processes, because the claim is about what an agent's MCP
    // server costs, and one loaded model is most of it.
    let mut one = McpServer::start(&stores[..1]);
    let mut three = McpServer::start(&stores);
    // Measured only after each server has answered a real query. A timer would
    // read whatever the process happened to have allocated by then: the first
    // version of this slept 15 seconds and reported 17 MB for both, because
    // under load neither had finished loading its model.
    one.answer_one_query();
    three.answer_one_query();
    let one_rss = one.rss_kb();
    let three_rss = three.rss_kb();
    println!(
        "mcp on 1 store: {} MB; on 3 stores: {} MB (+{} MB)",
        one_rss / 1024,
        three_rss / 1024,
        three_rss.saturating_sub(one_rss) / 1024,
    );
    // Three copies of the weights would be roughly three times a one-store
    // server. Half again as much is the ceiling this asserts: the extra is two
    // more SQLite connections and two more vector indexes, not two more models.
    assert!(
        three_rss < one_rss + one_rss / 2,
        "3 stores cost {} MB against {} MB for one — that looks like three models",
        three_rss / 1024,
        one_rss / 1024,
    );
    one.stop();
    three.stop();
}

/// What the tool list costs an agent, and what a server holds while it idles.
///
/// The tool list is loaded into an agent's context at the start of every
/// session, whether or not a single tool is called, so growing it spends the
/// context this product exists to save. 0.5.0 shipped two tools; 0.6.0 ships
/// five, and the size of that is a number rather than a shrug.
///
/// The lock is measured in the same test because both are properties of a
/// server sitting still: an MCP server must hold no store lock at rest, or
/// `semlith index` in another terminal fails for as long as the agent is open.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn measure_the_tool_list_and_what_an_idle_server_holds() {
    let corpus = tempfile::tempdir().unwrap();
    fs::write(
        corpus.path().join("ownership.md"),
        "Rust ownership gives every value exactly one owner.\n",
    )
    .unwrap();
    let store = corpus.path().join(".semlith");
    {
        let mut s = Semlith::open(&store, None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }

    let mut server = McpServer::start(std::slice::from_ref(&store));
    let listed = server.request(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#);

    let tools: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let count = tools["result"]["tools"].as_array().unwrap().len();
    let bytes = listed.trim_end().len();
    // Four characters to the token is the rule of thumb every tokenizer
    // roughly agrees with on English prose; it is an estimate and is printed
    // as one.
    println!("\n--- what the tool list costs every session");
    println!(
        "{count} tools: {bytes} bytes on the wire, ~{} tokens estimated at 4 bytes/token",
        bytes.div_ceil(4)
    );

    println!("\n--- what an idle server holds");
    let indexed = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(&store)
        .arg("index")
        .arg(corpus.path())
        .arg("--quiet")
        .output()
        .unwrap();
    println!(
        "`semlith index` against the store an MCP server is open on: exit {}",
        indexed.status
    );
    assert!(
        indexed.status.success(),
        "the idle MCP server was holding the store's write lock: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    server.stop();
}

/// Corpus sizes, in files of one chunk each: seven hundred, seven thousand and
/// seventy thousand vectors. An order of magnitude between each, which is what
/// it takes to tell a bounded curve from a linear one, and the largest more
/// than ten times the 6527-chunk corpus the project's other numbers come from.
///
/// One chunk per file, and a short one. What is being measured is what a vector
/// costs to hold, and there is one vector per chunk whatever its length — but
/// embedding time is not flat in chunk length, so full-length chunks would take
/// five times as long to reach the same vector count and measure the same
/// curve. The corpus is shaped for the measurement and says so.
const SCALE_FILES: [usize; 3] = [700, 7_000, 70_000];

/// Everything a searching process holds that is not vectors: the ONNX runtime,
/// the model weights, SQLite, the binary itself. Measured at the smallest
/// corpus, where the vectors are a rounding error, and then allowed for at
/// every larger one.
///
/// It is stated rather than derived because the claim is about the vectors: a
/// bound on the whole process would be a bound on fastembed's allocator, which
/// is not this release's to make.
const FIXED_OVERHEAD_MB: u64 = 200;

/// What a large store costs to open, to search, and to change one file in —
/// against the release before it, on the same corpora.
///
/// This is the release's central claim and the only place it is a number.
/// Point `OLD` at a 0.6.0 binary:
///
/// ```sh
/// OLD=/path/to/semlith-0.6.0 cargo test --release --test measure -- \
///     --ignored --nocapture measure_the_store_at_scale
/// ```
#[test]
#[ignore = "indexes a hundred thousand chunks twice over; takes tens of minutes"]
fn measure_the_store_at_scale() {
    let old = std::env::var("OLD").expect("set OLD to a 0.6.0 release binary");
    let new = env!("CARGO_BIN_EXE_semlith").to_string();
    println!("\n--- binaries");
    println!("old: {}", version(&old));
    println!("new: {}", version(&new));

    let root = tempfile::tempdir().unwrap();
    let mut rows = Vec::new();

    for files in SCALE_FILES {
        let corpus = root.path().join(format!("corpus_{files}"));
        fs::create_dir_all(&corpus).unwrap();
        for i in 0..files {
            // Ten paragraphs of roughly a chunk each, varied enough that the
            // embeddings are not all the same point.
            let body = format!(
                "Note {i}. Fermentation, ownership, retries and backoff, indexes and \
                 locks. This note is about topic {}, which is not what the note before \
                 it was about.\n",
                i % 997
            );
            fs::write(corpus.join(format!("note_{i:06}.md")), body).unwrap();
        }

        for (label, binary) in [("0.6.0", &old), ("0.7.0", &new)] {
            let store = root.path().join(format!("store_{label}_{files}"));
            let started = Instant::now();
            let index_rss = index_and_measure(binary, &store, &corpus, None);
            let indexed = started.elapsed();
            let chunks = chunk_count(&store);
            println!(
                "\n--- {label}: {files} files, {chunks} chunks, indexed in {:.0}s, \
                 peak RSS while indexing {} MB, index on disk {} KB",
                indexed.as_secs_f32(),
                index_rss / 1024,
                index_bytes(&store) / 1024,
            );

            let mut server = McpServer::start_with(binary, std::slice::from_ref(&store), None);
            server.handshake();
            let idle = server.rss_kb();
            let median = server.median_search();
            let busy = server.rss_kb();
            server.stop();
            println!(
                "{label}: idle RSS {} MB, searching RSS {} MB, median search {:.1}ms",
                idle / 1024,
                busy / 1024,
                median.as_secs_f64() * 1000.0,
            );
            rows.push((label, chunks, idle, busy, median, index_rss, store.clone()));
        }
    }

    // --- what a store past its budget costs ---------------------------------
    let biggest = rows
        .iter()
        .filter(|r| r.0 == "0.7.0")
        .max_by_key(|r| r.1)
        .unwrap()
        .clone();
    // Below what a single shard costs, so the store cannot hold all of itself
    // and every query pays to read a shard back. This is the cost side of the
    // trade, and it is measured on the store that exceeds its budget rather
    // than on one that comfortably fits.
    //
    // RSS is read three times, at widening query counts, because the question
    // is not only how much a churning store holds but whether that number
    // settles. Freed shard memory is not necessarily handed back to the
    // operating system, so a plateau is the honest form of "bounded" here.
    println!("\n--- 0.7.0 on {} chunks, budget cut to 8 MB", biggest.1);
    let mut squeezed = McpServer::start_with(&new, std::slice::from_ref(&biggest.6), Some("8"));
    squeezed.handshake();
    println!("idle RSS {} MB", squeezed.rss_kb() / 1024);
    let squeezed_median = squeezed.median_search();
    let mut churn = Vec::new();
    for round in 1..=3 {
        for i in 0..60 {
            let _ = squeezed.search_request(&format!("a churning question {round} {i}"));
        }
        let rss = squeezed.rss_kb();
        churn.push(rss);
        println!("after {} searches: RSS {} MB", 20 + round * 60, rss / 1024);
    }
    squeezed.stop();
    println!(
        "median search under an 8 MB budget: {:.1}ms",
        squeezed_median.as_secs_f64() * 1000.0,
    );

    // --- what changing one file costs ---------------------------------------
    println!("\n--- one file changed, on the largest corpus");
    for (label, binary) in [("0.6.0", &old), ("0.7.0", &new)] {
        let row = rows
            .iter()
            .filter(|r| r.0 == label)
            .max_by_key(|r| r.1)
            .unwrap();
        let corpus = root.path().join(format!("corpus_{}", SCALE_FILES[2]));
        let (rewritten, total, took) = change_one_file(binary, &row.6, &corpus, None);
        println!(
            "{label}: {} KB rewritten of a {} KB index, whole run {:.1}s",
            rewritten / 1024,
            total / 1024,
            took.as_secs_f32(),
        );
    }

    // At the shipped shard size a corpus of this size is two shards, and a
    // modified file necessarily touches two: the shard losing its old vector
    // and the open shard taking the new one. So the same measurement is taken
    // again on a store with many shards, where the property is visible rather
    // than hidden by the store being barely larger than a shard.
    println!("\n--- one file changed, on a store of many shards");
    let many = root.path().join("store_many_shards");
    let corpus = root.path().join(format!("corpus_{}", SCALE_FILES[1]));
    index_with_shards(&new, &many, &corpus, Some("512"));
    let shards = fs::read_dir(many.join("index"))
        .map(|d| d.flatten().count())
        .unwrap_or(0);
    let (rewritten, total, took) = change_one_file(&new, &many, &corpus, Some("512"));
    println!(
        "0.7.0, {} chunks in {shards} shards of 512: {} KB rewritten of a {} KB index, \
         whole run {:.1}s",
        chunk_count(&many),
        rewritten / 1024,
        total / 1024,
        took.as_secs_f32(),
    );
    assert!(
        rewritten * 3 < total,
        "changing one file rewrote {rewritten} of {total} bytes across {shards} shards"
    );

    println!("\n--- what a hundredfold corpus costs an open store");
    let mut slopes = Vec::new();
    for label in ["0.6.0", "0.7.0"] {
        let mine: Vec<_> = rows.iter().filter(|r| r.0 == label).collect();
        let (small, large) = (mine.first().unwrap(), mine.last().unwrap());
        let idle_growth = large.2 as i64 - small.2 as i64;
        let busy_growth = large.3 as i64 - small.3 as i64;
        println!(
            "{label}: {} to {} chunks — idle RSS {} MB to {} MB ({idle_growth:+} KB), \
             searching {} MB to {} MB ({busy_growth:+} KB), \
             {:.0} bytes per chunk while searching",
            small.1,
            large.1,
            small.2 / 1024,
            large.2 / 1024,
            small.3 / 1024,
            large.3 / 1024,
            busy_growth as f64 * 1024.0 / (large.1 - small.1).max(1) as f64,
        );
        slopes.push((label, idle_growth, busy_growth));
    }

    let new_rows: Vec<_> = rows.iter().filter(|r| r.0 == "0.7.0").collect();
    let (new_idle, old_idle) = (slopes[1].1, slopes[0].1);
    assert!(
        new_idle < old_idle,
        "an opened-but-unsearched 0.7.0 store grew by {new_idle} KB across a hundredfold \
         corpus against 0.6.0's {old_idle} KB — opening is still loading vectors"
    );
    assert!(
        new_idle < 16 * 1024,
        "an opened-but-unsearched store grew {new_idle} KB with the corpus"
    );

    let budget_mb = 512u64;
    let ceiling = (budget_mb + FIXED_OVERHEAD_MB) * 1024;
    assert!(
        new_rows.last().unwrap().3 < ceiling,
        "searching the largest store held {} MB, past the {budget_mb} MB budget plus \
         {FIXED_OVERHEAD_MB} MB of fixed overhead",
        new_rows.last().unwrap().3 / 1024,
    );
    // Not a bound on RSS: a store past its budget reloads shards constantly and
    // the allocator keeps what it frees. What must be true is that it settles.
    assert!(
        churn[2] < churn[1] + 32 * 1024,
        "RSS under continuous shard churn went {} MB then {} MB then {} MB — that is \
         not a plateau",
        churn[0] / 1024,
        churn[1] / 1024,
        churn[2] / 1024,
    );
}

/// Change one file in `corpus`, re-index it into `store`, and report
/// `(bytes rewritten, bytes of index, how long the run took)`.
fn change_one_file(
    binary: &str,
    store: &Path,
    corpus: &Path,
    shard_vectors: Option<&str>,
) -> (u64, u64, Duration) {
    let before = index_files(store);
    fs::write(
        corpus.join("note_000000.md"),
        "A note rewritten so exactly one file has changed.\n",
    )
    .unwrap();
    // A whole-index rewrite and a one-shard rewrite are indistinguishable
    // inside one filesystem timestamp tick.
    thread::sleep(Duration::from_millis(1100));
    let started = Instant::now();
    index_with_shards(binary, store, corpus, shard_vectors);
    let took = started.elapsed();
    let rewritten: u64 = index_files(store)
        .into_iter()
        .filter(|(p, _, when)| !before.iter().any(|(bp, _, bwhen)| bp == p && bwhen >= when))
        .map(|(_, len, _)| len)
        .sum();
    (rewritten, before.iter().map(|(_, len, _)| len).sum(), took)
}

/// Index with a shard size in force, for the measurements that need more
/// shards than a corpus a test can afford would otherwise produce.
fn index_with_shards(binary: &str, store: &Path, corpus: &Path, shard_vectors: Option<&str>) {
    let mut cmd = Command::new(binary);
    if let Some(n) = shard_vectors {
        cmd.env("SEMLITH_SHARD_VECTORS", n);
    }
    let out = cmd
        .arg("--store")
        .arg(store)
        .arg("index")
        .arg(corpus)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "indexing failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn version(binary: &str) -> String {
    let out = Command::new(binary).arg("--version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn chunk_count(store: &Path) -> i64 {
    let s = Semlith::open(store, None).unwrap();
    semlith::store::durable_chunks(s.db()).unwrap()
}

/// Index `corpus` into `store` with `binary`, returning the run's peak RSS in
/// kilobytes as the kernel reports it.
fn index_and_measure(binary: &str, store: &Path, corpus: &Path, budget: Option<&str>) -> u64 {
    let mut cmd = Command::new("/usr/bin/time");
    cmd.arg("-l").arg(binary);
    if let Some(mb) = budget {
        cmd.env("SEMLITH_INDEX_MEMORY", mb);
    }
    let out = cmd
        .arg("--store")
        .arg(store)
        .arg("index")
        .arg(corpus)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "indexing failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    peak_rss_kb(&String::from_utf8_lossy(&out.stderr))
}

/// `maximum resident set size` out of `/usr/bin/time -l`, in kilobytes.
///
/// macOS reports it in bytes on that line; GNU time prints kilobytes under a
/// different label, and this measurement is taken on the former.
fn peak_rss_kb(text: &str) -> u64 {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_suffix("maximum resident set size") {
            return rest.trim().parse::<u64>().unwrap_or(0) / 1024;
        }
    }
    0
}

/// Every file of a store's vector index, with its size and mtime.
fn index_files(store: &Path) -> Vec<(std::path::PathBuf, u64, std::time::SystemTime)> {
    let single = store.join("index.tv");
    let paths: Vec<std::path::PathBuf> = if single.exists() {
        vec![single]
    } else {
        fs::read_dir(store.join("index"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect()
    };
    paths
        .into_iter()
        .filter_map(|p| {
            let m = fs::metadata(&p).ok()?;
            Some((p, m.len(), m.modified().ok()?))
        })
        .collect()
}

/// The floor sharding has to clear, decided before the comparison was run.
///
/// turbovec fits its TQ+ calibration from the first batch added to an index, so
/// a store split across shards fits it once per shard rather than once for the
/// corpus. Two shards therefore score in slightly different coordinate systems,
/// and the merged ranking is not guaranteed to be the ranking one index would
/// have produced. 0.90 mean overlap at 10 is the line: below it, sharding is
/// buying bounded memory with recall, and the release would have to change
/// shape rather than be shipped with a footnote.
const RECALL_FLOOR: f32 = 0.90;

/// What splitting an index into shards costs the answers.
///
/// The same corpus, the same model, the same queries, indexed twice: once with
/// every vector in one shard, once with the shards small enough that a corpus
/// this size spans several. The only difference between the two stores is the
/// split, so the difference between their answers is what the split costs.
#[test]
#[ignore = "indexes this repository twice; downloads an embedding model on first run"]
fn measure_what_sharding_costs_recall() {
    // A real corpus rather than generated notes: prose, code and configuration
    // in the proportions a developer actually points semlith at.
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"));

    let whole = tempfile::tempdir().unwrap();
    let split = tempfile::tempdir().unwrap();
    // 1 shard against roughly a dozen. 128 is far below the shipped shard size,
    // which makes this the pessimistic case: the smaller the shard, the less
    // data each calibration is fitted on.
    index_with(whole.path(), corpus, "100000000");
    index_with(split.path(), corpus, "128");

    let mut one = Semlith::open(whole.path(), None).unwrap();
    one.quiet = true;
    let mut many = Semlith::open(split.path(), None).unwrap();
    many.quiet = true;

    let shards = fs::read_dir(split.path().join("index"))
        .map(|d| d.flatten().count())
        .unwrap_or(0);
    println!("\n--- recall, 1 shard against {shards}");
    assert!(shards > 4, "the split store has only {shards} shard(s)");
    assert_eq!(
        one.len(),
        many.len(),
        "the two stores did not index the same corpus"
    );

    // Questions in the shape an agent asks them: about meaning, not about a
    // literal identifier the keyword half would find on its own.
    let queries = [
        "how are vectors kept from filling memory",
        "what happens when an index run is interrupted",
        "how does the store decide a file has changed",
        "why does a search look deeper than k",
        "what stops two writers corrupting a store",
        "how is a query embedded once for several stores",
        "what does the watcher do when a file is deleted",
        "how are results from keyword and vector search combined",
        "what makes a store from an older version still readable",
        "how does an agent narrow a search to one subsystem",
        "what limits the size of a file that gets indexed",
        "how does a long tool call avoid timing out",
    ];

    let mut overlaps = Vec::new();
    let mut identical = 0;
    for query in queries {
        let a = ranking(&mut one, query);
        let b = ranking(&mut many, query);
        let shared = a.iter().filter(|hit| b.contains(hit)).count();
        let overlap = shared as f32 / a.len().max(1) as f32;
        if a == b {
            identical += 1;
        }
        println!("  {overlap:.2}  {query}");
        overlaps.push(overlap);
    }
    let mean = overlaps.iter().sum::<f32>() / overlaps.len() as f32;
    let worst = overlaps.iter().cloned().fold(f32::INFINITY, f32::min);
    println!(
        "mean overlap@10 {mean:.3}, worst {worst:.2}, {identical}/{} rankings identical",
        queries.len()
    );
    assert!(
        mean >= RECALL_FLOOR,
        "sharding cost recall: mean overlap@10 {mean:.3} is below the {RECALL_FLOOR} floor"
    );
}

/// Index `corpus` into `store` through the binary, so the shard size is in
/// force for the whole run.
fn index_with(store: &Path, corpus: &Path, shard_vectors: &str) {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store)
        .arg("index")
        .arg(corpus)
        .arg("--quiet")
        .env("SEMLITH_SHARD_VECTORS", shard_vectors)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "indexing failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The top ten hits as a comparable list. Line spans rather than chunk ids: two
/// stores built separately need not agree on ids, but they do on what a chunk
/// is.
fn ranking(store: &mut Semlith, query: &str) -> Vec<(String, u32)> {
    store
        .search(query, 10)
        .unwrap()
        .into_iter()
        .map(|h| (h.path, h.start_line))
        .collect()
}

/// A real `semlith mcp` process, for measuring what an agent's server costs.
struct McpServer {
    child: Child,
    /// Held for the life of the server, not made per request: a `BufReader`
    /// built fresh each time can swallow a response that arrived in the same
    /// read as the previous one, which turns into a hang several requests later.
    out: std::io::BufReader<std::process::ChildStdout>,
}

impl McpServer {
    fn start(stores: &[std::path::PathBuf]) -> Self {
        Self::start_with(env!("CARGO_BIN_EXE_semlith"), stores, None)
    }

    /// A server from a named binary, so one release can be measured beside
    /// another with the same client driving both.
    fn start_with(binary: &str, stores: &[std::path::PathBuf], budget: Option<&str>) -> Self {
        let mut cmd = Command::new(binary);
        for store in stores {
            cmd.arg("--store").arg(store);
        }
        if let Some(mb) = budget {
            cmd.env("SEMLITH_INDEX_MEMORY", mb);
        }
        let mut child = cmd
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = std::io::BufReader::new(child.stdout.take().unwrap());
        Self { child, out }
    }

    /// Open the session and list the tools — everything a client does before it
    /// asks a question, and nothing that touches a vector. What the process
    /// holds after this is what an idle server costs.
    fn handshake(&mut self) {
        let _ = self.request(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}"#,
        );
        let _ = self.request(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#);
        // Settle, so the reading is the server at rest rather than the server
        // still parsing what it was just sent.
        thread::sleep(Duration::from_millis(500));
    }

    /// Median of twenty warm searches. The first few are thrown away: they pay
    /// for the model load and the first shard reads, which is a startup cost
    /// and not what a session's queries cost.
    fn median_search(&mut self) -> Duration {
        for i in 0..3 {
            let _ = self.search_request(&format!("a warming question {i}"));
        }
        let mut times = Vec::new();
        for i in 0..20 {
            let started = Instant::now();
            let answer = self.search_request(&format!("how is retry backoff described {i}"));
            times.push(started.elapsed());
            assert!(answer.contains("result"), "a search failed: {answer}");
        }
        times.sort();
        times[times.len() / 2]
    }

    fn search_request(&mut self, query: &str) -> String {
        self.request(&format!(
            r#"{{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{{"name":"semlith_search","arguments":{{"query":"{query}","k":5}}}}}}"#
        ))
    }

    /// Drive one real search to completion, so the process being measured is one
    /// that has loaded its model and answered — not one that is still starting.
    fn answer_one_query(&mut self) {
        let _ = self.request(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        );
        let _ = self.request(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"semlith_search","arguments":{"query":"how is backoff described","k":3}}}"#,
        );
    }

    /// One request, one response line.
    fn request(&mut self, request: &str) -> String {
        use std::io::{BufRead, Write};
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();

        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "the server closed instead of answering");
        line
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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every byte of a store's vector index, in whichever layout it is written:
/// one `index.tv` for a store from before 0.7.0, a directory of shards for one
/// created by it.
fn index_bytes(store: &Path) -> u64 {
    let single = store.join("index.tv");
    if single.exists() {
        return fs::metadata(single).map(|m| m.len()).unwrap_or(0);
    }
    fs::read_dir(store.join("index"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// The `index_generation` meta key, which counts index rewrites.
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
