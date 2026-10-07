//! The remote lane against a `semlith worker` on this machine: the CPU-backed
//! worker lane stands in for the GPU, so everything but the hardware and the
//! attestation tokens is the real path. A run through the worker leaves the
//! store a CPU-only run leaves; a worker that refuses, cannot prove what the
//! policy asks, or dies mid-run leaves the run to the CPU, which finishes it.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TOKEN: &str = "test-token-0123456789abcdef";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Worker {
    child: Child,
    endpoint: String,
    log: mpsc::Receiver<String>,
}

impl Worker {
    fn start(home: &Path, tmp: &Path, extra_env: &[(&str, &str)]) -> Worker {
        let port = free_port();
        let endpoint = format!("127.0.0.1:{port}");
        let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
        command
            .args(["worker", "--listen", &endpoint, "--lane", "worker"])
            .env("SEMLITH_WORKER_TOKEN", TOKEN)
            .env("SEMLITH_HOME", home)
            .env("TMPDIR", tmp)
            .stderr(Stdio::piped())
            .stdout(Stdio::null());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, log) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let worker = Worker {
            child,
            endpoint,
            log,
        };
        // Its banner, bounded: it binds before it prints.
        let banner = worker.wait_for("semlith worker 0.", Duration::from_secs(30));
        assert!(banner.is_some(), "the worker never said it was listening");
        worker
    }

    fn wait_for(&self, needle: &str, within: Duration) -> Option<String> {
        let deadline = Instant::now() + within;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.log.recv_timeout(left) {
                Ok(line) if line.contains(needle) => return Some(line),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const TOPICS: [&str; 24] = [
    "lighthouse keeper",
    "sourdough starter",
    "glacier retreat",
    "violin bow rehair",
    "beehive swarm",
    "tram timetable",
    "orchid repotting",
    "kiln firing schedule",
    "saffron harvest",
    "falcon tracking",
    "submarine cable",
    "chess endgame",
    "vineyard frost",
    "telescope mirror",
    "monsoon flood barrier",
    "bonsai pruning",
    "tidal power turbine",
    "ice hockey skate",
    "printing press type",
    "desert solar farm",
    "coral reef survey",
    "subway tunnel boring",
    "maple syrup tapping",
    "volcano seismograph",
];

/// One file per topic, then stock ledgers, so a query names one file.
fn corpus(dir: &Path, files: usize) {
    for n in 0..files {
        // Each topic in one file only; the rest are stock ledgers, so no
        // query lands on a tie between two copies of one text.
        let Some(topic) = TOPICS.get(n) else {
            let text: String = (0..6)
                .map(|p| {
                    format!(
                        "Stock ledger page {n}, line {p}: shelf {} holds {} crates of item {}.\n\n",
                        n * 7 + p,
                        n * 13 + p,
                        n * 31 + p
                    )
                })
                .collect();
            std::fs::write(dir.join(format!("note-{n:03}.md")), text).unwrap();
            continue;
        };
        let text: String = (0..6)
            .map(|p| {
                format!(
                    "Notes on the {topic}, part {p} of file {n}: the {topic} needed attention in week {}, \
                     and the log for the {topic} records step {} as finished.\n\n",
                    p + n,
                    p * 3 + 1
                )
            })
            .collect();
        std::fs::write(dir.join(format!("note-{n:03}.md")), text).unwrap();
    }
}

fn policy(dir: &Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("policy.json");
    std::fs::write(&path, body).unwrap();
    path
}

/// Index `corpus` into `store` on the lanes `accel` names, with the remote
/// lane's settings when given.
fn index(
    corpus: &Path,
    store: &Path,
    home: &Path,
    accel: &str,
    remote: Option<(&str, &str, &Path)>,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
    command
        .args(["index", corpus.to_str().unwrap()])
        .arg("--store")
        .arg(store)
        .env("SEMLITH_HOME", home)
        .env("SEMLITH_ACCEL", accel)
        // A cached vector would never reach the worker.
        .env("SEMLITH_VECTOR_CACHE_MB", "0");
    if let Some((endpoint, token, policy)) = remote {
        command
            .env("SEMLITH_REMOTE_ENDPOINT", endpoint)
            .env("SEMLITH_REMOTE_TOKEN", token)
            .env("SEMLITH_REMOTE_POLICY", policy);
    }
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// The file of the best hit, and how many hits there were.
/// The chunk count a run reports: "indexed N files (M chunks)".
fn chunks(out: &std::process::Output) -> u64 {
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let at = text
        .find(" chunks)")
        .expect("a chunk count in the run's summary");
    text[..at]
        .rsplit('(')
        .next()
        .and_then(|n| n.trim().parse().ok())
        .expect("a number before \" chunks)\"")
}

fn search(store: &Path, home: &Path, query: &str) -> (String, usize) {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["search", query, "--json", "-k", "5"])
        .arg("--store")
        .arg(store)
        .env("SEMLITH_HOME", home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let hits = value
        .as_array()
        .cloned()
        .or_else(|| value["results"].as_array().cloned())
        .unwrap_or_default();
    let top = hits
        .first()
        .and_then(|hit| hit["path"].as_str())
        .map(|path| path.rsplit('/').next().unwrap_or(path).to_string())
        .unwrap_or_default();
    (top, hits.len())
}

fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path.display().to_string());
            }
        }
    }
    out.sort();
    out
}

/// The worker's closing line for a connection: (batches, rows).
fn counts(line: &str) -> (u64, u64) {
    let number = |word: &str| -> u64 {
        let before = line.split(word).next().unwrap_or("");
        before
            .split_whitespace()
            .last()
            .and_then(|n| n.trim_matches(',').parse().ok())
            .unwrap_or(0)
    };
    (number(" batches"), number(" rows"))
}

const QUERIES: [&str; 4] = [
    "who looks after the lighthouse",
    "feeding a sourdough starter",
    "pruning a bonsai",
    "seismograph readings on the volcano",
];

#[test]
fn a_run_through_the_worker_leaves_the_store_a_cpu_run_leaves_and_the_worker_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (notes, home, worker_home, worker_tmp) = (
        dir.path().join("notes"),
        dir.path().join("home"),
        dir.path().join("worker-home"),
        dir.path().join("worker-tmp"),
    );
    for d in [&notes, &home, &worker_home, &worker_tmp] {
        std::fs::create_dir_all(d).unwrap();
    }
    corpus(&notes, 40);
    let off = policy(dir.path(), r#"{"off": true}"#);
    let worker = Worker::start(&worker_home, &worker_tmp, &[]);
    let before = files_under(&worker_home)
        .into_iter()
        .chain(files_under(&worker_tmp))
        .collect::<Vec<_>>();

    let remote_store = dir.path().join("remote-store");
    let remote_run = index(
        &notes,
        &remote_store,
        &home,
        "remote",
        Some((&worker.endpoint, TOKEN, &off)),
    );
    let done = worker
        .wait_for("done after", Duration::from_secs(120))
        .expect("the worker's connection closed with a count");
    let (batches, rows) = counts(&done);
    assert!(
        batches > 0 && rows > 0,
        "nothing went through the worker: {done}"
    );

    let after = files_under(&worker_home)
        .into_iter()
        .chain(files_under(&worker_tmp))
        .collect::<Vec<_>>();
    assert_eq!(before, after, "the worker wrote files during the run");

    let cpu_store = dir.path().join("cpu-store");
    let cpu_run = index(&notes, &cpu_store, &home, "cpu", None);
    same_store(&remote_run, &remote_store, &cpu_run, &cpu_store, &home);
}

/// The same chunks, and every query's best file the same: the vectors differ
/// only by the int8 noise two CPU sessions show.
fn same_store(
    a_run: &std::process::Output,
    a: &Path,
    b_run: &std::process::Output,
    b: &Path,
    home: &Path,
) {
    assert_eq!(chunks(a_run), chunks(b_run), "different chunk counts");
    for query in QUERIES {
        let (top_a, n_a) = search(a, home, query);
        let (top_b, n_b) = search(b, home, query);
        assert_eq!(top_a, top_b, "{query}");
        assert_eq!(n_a, n_b, "{query}");
    }
}

#[test]
fn a_wrong_token_is_refused_and_the_cpu_carries_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let (notes, home) = (dir.path().join("notes"), dir.path().join("home"));
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    corpus(&notes, 5);
    let off = policy(dir.path(), r#"{"off": true}"#);
    let worker = Worker::start(&dir.path().join("wh"), dir.path(), &[]);
    let store = dir.path().join("store");
    index(
        &notes,
        &store,
        &home,
        "remote",
        Some((&worker.endpoint, "not-the-token-at-all", &off)),
    );
    assert!(
        worker
            .wait_for("the token was refused", Duration::from_secs(30))
            .is_some(),
        "the worker did not refuse the token"
    );
    assert!(
        search(&store, &home, QUERIES[0]).1 > 0,
        "the CPU did not carry the run"
    );
}

#[test]
fn evidence_the_policy_requires_and_the_worker_lacks_means_nothing_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let (notes, home) = (dir.path().join("notes"), dir.path().join("home"));
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    corpus(&notes, 5);
    let strict = policy(
        dir.path(),
        r#"{"cpu": {"issuer": "https://confidentialcomputing.googleapis.com",
                    "jwks": {"keys": []}, "nonce_claim": "eat_nonce"}}"#,
    );
    let worker = Worker::start(&dir.path().join("wh"), dir.path(), &[]);
    let store = dir.path().join("store");
    index(
        &notes,
        &store,
        &home,
        "remote",
        Some((&worker.endpoint, TOKEN, &strict)),
    );
    let done = worker
        .wait_for("done after", Duration::from_secs(60))
        .expect("the connection closed");
    assert_eq!(
        counts(&done),
        (0, 0),
        "a batch was sent before the check: {done}"
    );
    assert!(
        search(&store, &home, QUERIES[0]).1 > 0,
        "the CPU did not carry the run"
    );
}

#[test]
fn a_worker_that_dies_mid_run_leaves_the_same_store_the_cpu_would() {
    let dir = tempfile::tempdir().unwrap();
    let (notes, home) = (dir.path().join("notes"), dir.path().join("home"));
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    corpus(&notes, 240);
    let off = policy(dir.path(), r#"{"off": true}"#);
    // Its lane worker fails the second batch, and the connection goes with it.
    let worker = Worker::start(
        &dir.path().join("wh"),
        dir.path(),
        &[("SEMLITH_FAULT_ACCEL", "worker:batch:2")],
    );
    let remote_store = dir.path().join("remote-store");
    let remote_run = index(
        &notes,
        &remote_store,
        &home,
        "remote",
        Some((&worker.endpoint, TOKEN, &off)),
    );
    let said = String::from_utf8_lossy(&remote_run.stderr).to_string();
    assert!(
        said.contains("the remote lane failed"),
        "the run did not say the lane failed:\n{said}"
    );
    let cpu_store = dir.path().join("cpu-store");
    let cpu_run = index(&notes, &cpu_store, &home, "cpu", None);
    same_store(&remote_run, &remote_store, &cpu_run, &cpu_store, &home);
}
