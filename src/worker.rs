//! The parse worker: every reader of a file's own bytes, in a process of its
//! own.
//!
//! A file is untrusted input, and some of what reads it is C — tree-sitter's
//! grammars above all. A C `assert` calls `abort()`, which no `catch_unwind`
//! can catch: before 0.40.0 a 606-byte Markdown file of nested blockquotes
//! aborted the daemon, and under launchd or systemd the service then
//! crash-looped on it for as long as the file stayed in a watched root. The
//! abort is a class, not one grammar — tree-sitter-md and tree-sitter-yaml both
//! hit the same external-scanner assertion — so the fix is a boundary, not a
//! patch: the text readers, the grammars and an image decode check run in
//! `semlith __parse-worker`, this same binary started again, speaking
//! length-framed requests over its stdin and stdout.
//!
//! An abort, a stack overflow, an allocation past the memory limit or a file
//! that takes longer than its deadline kills one worker. The file is refused
//! with the reason, the worker is replaced on the next call, and nothing else
//! notices. The [`Journal`] remembers the refusal across restarts, so a file
//! that crashes a parser is parsed once per version, never in a loop.
//!
//! Workers are pooled: one is taken per call and handed back after, so the
//! pool grows to the number of threads that parse at once — the prepare
//! stage's — and no further. An idle worker costs a process and no models; it
//! exits when its stdin closes, which is when this process does, however it
//! does.
//!
//! The worker is switched on by the binary ([`enable`]), never by the library:
//! a test that links the crate runs its readers in-process as before, because
//! its own executable is a test harness, not semlith.

use crate::SkipReason;
use crate::graph::Extraction;
use serde::{Deserialize, Serialize};
use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The hidden subcommand a worker is started as.
pub const SUBCOMMAND: &str = "__parse-worker";

/// The memory one worker may allocate, in MiB. Two gibibytes is ten times
/// what the largest real document in the measured corpora needed; a file past
/// it is refused with "limit" in the reason, and this is the knob to raise it.
pub const MEMORY_ENV: &str = "SEMLITH_PARSE_MEMORY_MB";
const DEFAULT_MEMORY_MB: usize = 2048;

/// The most workers kept idle. More than the prepare stage ever runs at once
/// on any machine measured, so in practice every worker is kept.
const MAX_IDLE: usize = 32;

const OP_READ: u8 = 1;
const OP_READ_GRAPH: u8 = 2;
const OP_GRAPH: u8 = 3;
const OP_IMAGE: u8 = 4;
const OP_PATTERN: u8 = 5;

static EXE: OnceLock<PathBuf> = OnceLock::new();
static IDLE: Mutex<Vec<Worker>> = Mutex::new(Vec::new());

/// Run every parser in a worker started from `exe` from now on. Called once
/// by the binary at start.
pub fn enable(exe: PathBuf) {
    let _ = EXE.set(exe);
}

/// Whether parsers run in a worker in this process.
pub fn enabled() -> bool {
    EXE.get().is_some()
}

/// Why a call did not come back with an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The worker died or ran out of time on this file. The file is refused
    /// with this reason, and the journal keeps it refused.
    Crashed(String),
    /// No worker could be started or spoken to. Not this file's fault, so
    /// nothing is refused; the file fails this pass and is tried again.
    Unavailable(String),
}

impl Stop {
    pub fn reason(&self) -> &str {
        match self {
            Self::Crashed(r) | Self::Unavailable(r) => r,
        }
    }
}

/// A reader's answer about one file's bytes.
#[derive(Debug, Serialize, Deserialize)]
pub struct Parsed {
    /// The file's text and its units, why there is none, or the reader's
    /// panic message.
    pub text: Result<Result<(String, Option<i64>), SkipReason>, String>,
    /// The file's symbols and edges, when asked for and there was text.
    pub graph: Option<Result<Option<Extraction>, String>>,
}

/// Read a file's bytes into text, and its graph too when `with_graph` is set.
pub fn read(path: &Path, bytes: &[u8], with_graph: bool) -> Result<Parsed, Stop> {
    let op = if with_graph { OP_READ_GRAPH } else { OP_READ };
    if !enabled() {
        return Ok(serve_read(path, bytes, with_graph));
    }
    let (head, text) = match call(op, path, bytes) {
        // Out of time with the graph asked for: the graph is what takes long
        // on a large file — tree-sitter's own parse is capped at two seconds,
        // but collecting a big tree's definitions is not. Before 0.40.0 that
        // ran with no limit at all. Losing the graph is what the parser's own
        // two-second cap already costs a file, so the text is read again, alone,
        // and indexed without one. Only text that cannot be read in time
        // refuses the file.
        Err(Stop::Crashed(reason)) if with_graph && reason.starts_with("parser timed out") => {
            let mut read = read(path, bytes, false)?;
            read.graph = Some(Ok(None));
            return Ok(read);
        }
        answer => answer?,
    };
    let mut read: Parsed = decode(&head)?;
    if let Ok(Ok((t, _))) = &mut read.text {
        *t = String::from_utf8(text).map_err(|_| {
            Stop::Unavailable("the parse worker sent text that is not UTF-8".into())
        })?;
    }
    Ok(read)
}

/// A file's symbols and edges, from text already decided.
pub fn graph(path: &Path, text: &str) -> Result<Result<Option<Extraction>, String>, Stop> {
    if !enabled() {
        return Ok(serve_graph(path, text));
    }
    let (head, _) = call(OP_GRAPH, path, text.as_bytes())?;
    decode(&head)
}

/// A pattern's captures in one file's text, parsed and queried in a worker:
/// the daemon parses stored text for `semlith_pattern`, and a grammar's crash
/// there was the daemon's.
pub fn pattern(
    language: &str,
    source: &str,
    text: &str,
) -> Result<Result<Vec<crate::pattern::Capture>, String>, Stop> {
    let payload = [
        language.as_bytes(),
        b"\0",
        source.as_bytes(),
        b"\0",
        text.as_bytes(),
    ]
    .concat();
    if !enabled() {
        return Ok(serve_pattern(&payload));
    }
    let (head, _) = call(OP_PATTERN, Path::new(language), &payload)?;
    decode(&head)
}

fn serve_pattern(payload: &[u8]) -> Result<Vec<crate::pattern::Capture>, String> {
    let mut parts = payload.splitn(3, |b| *b == 0);
    let (Some(language), Some(source), Some(text)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err("a pattern request without its parts".into());
    };
    let (language, source) = (
        String::from_utf8_lossy(language),
        String::from_utf8_lossy(source),
    );
    let text = String::from_utf8_lossy(text);
    crate::contained(|| crate::pattern::captures(&language, &source, &text))
        .and_then(|r| r)
        .map_err(|e| format!("{e:#}"))
}

/// Whether an image's bytes decode, checked where a decoder's crash costs a
/// worker. CLIP decodes the same bytes with the same crate afterwards.
pub fn image(path: &Path, bytes: &[u8]) -> Result<(), Stop> {
    if !enabled() {
        return Ok(());
    }
    call(OP_IMAGE, path, bytes).map(|_| ())
}

fn decode<T: for<'a> Deserialize<'a>>(head: &[u8]) -> Result<T, Stop> {
    serde_json::from_slice(head)
        .map_err(|e| Stop::Unavailable(format!("the parse worker's answer did not parse: {e}")))
}

fn serve_read(path: &Path, bytes: &[u8], with_graph: bool) -> Parsed {
    let text = crate::contained(|| {
        crate::fault_panic(path);
        crate::chunk::extract_counted(path, bytes)
    })
    .map_err(|e| format!("{e:#}"));
    let graph = match (&text, with_graph) {
        (Ok(Ok((text, _))), true) => Some(serve_graph(path, text)),
        _ => None,
    };
    Parsed { text, graph }
}

fn serve_graph(path: &Path, text: &str) -> Result<Option<Extraction>, String> {
    // Debug builds only: hang while making the graph, never while reading.
    #[cfg(debug_assertions)]
    if std::env::var("SEMLITH_FAULT_GRAPH_HANG").is_ok_and(|name| {
        path.file_name()
            .is_some_and(|n| n.to_string_lossy() == name)
    }) {
        std::thread::sleep(Duration::from_secs(3600));
    }
    crate::contained(|| crate::graph::extract(path, text))
        .and_then(|r| r)
        .map_err(|e| format!("{e:#}"))
}

// ------------------------------------------------------------------- parent

struct Worker {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    answers: Receiver<std::io::Result<(Vec<u8>, Vec<u8>)>>,
    /// The last of what the worker printed to stderr, which is where Rust
    /// says why it aborted: out of memory, or out of stack.
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_reader: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn memory_limit() -> usize {
    std::env::var(MEMORY_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|mb| *mb > 0)
        .unwrap_or(DEFAULT_MEMORY_MB)
        * 1024
        * 1024
}

/// How long one file may take: thirty seconds, and a second more per MiB, so
/// a large PDF is not cut short where a small file that hangs is.
fn deadline(bytes: usize) -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("SEMLITH_PARSE_DEADLINE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return Duration::from_millis(ms);
    }
    Duration::from_secs(30 + (bytes / (1024 * 1024)) as u64)
}

fn spawn() -> Result<Worker, Stop> {
    let exe = EXE.get().expect("enabled");
    let mut child = Command::new(exe)
        .arg(SUBCOMMAND)
        .env(MEMORY_ENV, (memory_limit() / (1024 * 1024)).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Stop::Unavailable(format!("the parse worker did not start: {e}")))?;
    // The C half of a parser allocates outside Rust's allocator; Windows caps
    // the whole process here, as Linux does with RLIMIT_AS in the worker.
    #[cfg(windows)]
    crate::llama::job(&child, memory_limit() + 512 * 1024 * 1024);
    let stdin = child.stdin.take().expect("piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped"));
    let mut err = child.stderr.take().expect("piped");
    let (tx, answers) = channel();
    std::thread::Builder::new()
        .name("semlith-parse-answers".into())
        .spawn(move || {
            loop {
                let answer = read_frame(&mut stdout)
                    .and_then(|head| read_frame(&mut stdout).map(|text| (head, text)));
                let done = answer.is_err();
                if tx.send(answer).is_err() || done {
                    return;
                }
            }
        })
        .map_err(|e| Stop::Unavailable(format!("starting the parse worker's reader: {e}")))?;
    // The worker says hello once it is up. Its start — a cold exec of a large
    // binary, the first after an upgrade, a busy disk — is not the file's
    // time: a deadline that counted it refused healthy files as timed out,
    // and the journal then kept them refused.
    match answers.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok(_)) => {}
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Stop::Unavailable(
                "the parse worker started but never said it was ready".into(),
            ));
        }
    }
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let tail = Arc::clone(&stderr);
    let stderr_reader = std::thread::Builder::new()
        .name("semlith-parse-stderr".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    return;
                }
                let mut tail = tail.lock().unwrap_or_else(|p| p.into_inner());
                tail.extend_from_slice(&buf[..n]);
                let over = tail.len().saturating_sub(4096);
                tail.drain(..over);
            }
        })
        .ok();
    Ok(Worker {
        child,
        stdin: BufWriter::new(stdin),
        answers,
        stderr,
        stderr_reader,
    })
}

fn call(op: u8, path: &Path, payload: &[u8]) -> Result<(Vec<u8>, Vec<u8>), Stop> {
    let taken = IDLE.lock().unwrap_or_else(|p| p.into_inner()).pop();
    let mut worker = match taken {
        Some(w) => w,
        None => spawn()?,
    };
    let path_bytes = path.to_string_lossy();
    let sent = (|| {
        worker.stdin.write_all(&[op])?;
        write_frame(&mut worker.stdin, path_bytes.as_bytes())?;
        write_frame(&mut worker.stdin, payload)?;
        worker.stdin.flush()
    })();
    if let Err(e) = sent {
        // A worker that died between calls: nothing of this file reached it.
        // One fresh worker gets the file before anyone calls it a crash.
        drop(worker);
        worker = spawn()?;
        let again = (|| {
            worker.stdin.write_all(&[op])?;
            write_frame(&mut worker.stdin, path_bytes.as_bytes())?;
            write_frame(&mut worker.stdin, payload)?;
            worker.stdin.flush()
        })();
        if again.is_err() {
            return Err(Stop::Unavailable(format!(
                "the parse worker would not take the file: {e}"
            )));
        }
    }
    let limit = deadline(payload.len());
    match worker.answers.recv_timeout(limit) {
        Ok(Ok(answer)) => {
            let mut idle = IDLE.lock().unwrap_or_else(|p| p.into_inner());
            if idle.len() < MAX_IDLE {
                idle.push(worker);
            }
            Ok(answer)
        }
        Err(RecvTimeoutError::Timeout) => Err(Stop::Crashed(format!(
            "parser timed out after {}",
            seconds(limit)
        ))),
        Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => Err(Stop::Crashed(died(worker))),
    }
}

fn seconds(d: Duration) -> String {
    if d.as_secs() > 0 {
        format!("{} s", d.as_secs())
    } else {
        format!("{} ms", d.as_millis())
    }
}

/// Why a worker that stopped answering stopped, in the words a refusal shows.
fn died(mut worker: Worker) -> String {
    let status = worker.child.wait().ok();
    if let Some(reader) = worker.stderr_reader.take() {
        let _ = reader.join();
    }
    let said = String::from_utf8_lossy(&worker.stderr.lock().unwrap_or_else(|p| p.into_inner()))
        .into_owned();
    if said.contains("memory allocation of") {
        return format!(
            "parser ran past its memory limit ({} MiB; {MEMORY_ENV} raises it)",
            memory_limit() / (1024 * 1024)
        );
    }
    if said.contains("has overflowed its stack") {
        return "parser overflowed its stack".to_string();
    }
    match status {
        Some(status) => format!("parser crashed: {}", describe(status)),
        None => "parser crashed".to_string(),
    }
}

#[cfg(unix)]
fn describe(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(signal) => match signal {
            libc::SIGABRT => "SIGABRT".to_string(),
            libc::SIGSEGV => "SIGSEGV".to_string(),
            libc::SIGBUS => "SIGBUS".to_string(),
            libc::SIGILL => "SIGILL".to_string(),
            libc::SIGFPE => "SIGFPE".to_string(),
            libc::SIGKILL => "SIGKILL".to_string(),
            other => format!("signal {other}"),
        },
        None => format!("exit status {}", status.code().unwrap_or(-1)),
    }
}

#[cfg(not(unix))]
fn describe(status: std::process::ExitStatus) -> String {
    format!("exit code {:#x}", status.code().unwrap_or(-1) as u32)
}

fn write_frame(out: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    out.write_all(&(bytes.len() as u64).to_le_bytes())?;
    out.write_all(bytes)
}

fn read_frame(input: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 8];
    input.read_exact(&mut len)?;
    let len = u64::from_le_bytes(len) as usize;
    let mut bytes = vec![0u8; len];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

// ------------------------------------------------------------------- worker

/// `semlith __parse-worker`: answer requests on stdin until it closes.
pub fn worker_main() -> i32 {
    let limit = std::env::var(MEMORY_ENV)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MEMORY_MB)
        * 1024
        * 1024;
    limit_memory(limit);
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut out = BufWriter::new(std::io::stdout().lock());
    // Hello: an empty answer, before the first request.
    if write_frame(&mut out, b"")
        .and_then(|_| write_frame(&mut out, b""))
        .and_then(|_| out.flush())
        .is_err()
    {
        return 0;
    }
    loop {
        let mut op = [0u8];
        if input.read_exact(&mut op).is_err() {
            return 0;
        }
        let Ok(path) = read_frame(&mut input) else {
            return 1;
        };
        let Ok(payload) = read_frame(&mut input) else {
            return 1;
        };
        let path = PathBuf::from(String::from_utf8_lossy(&path).into_owned());
        fault(&path);
        let (head, text) = serve(op[0], &path, payload);
        if write_frame(&mut out, &head)
            .and_then(|_| write_frame(&mut out, &text))
            .and_then(|_| out.flush())
            .is_err()
        {
            return 0;
        }
    }
}

fn serve(op: u8, path: &Path, payload: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    match op {
        OP_READ | OP_READ_GRAPH => {
            let mut read = serve_read(path, &payload, op == OP_READ_GRAPH);
            drop(payload);
            // The text goes out raw, beside the answer rather than escaped
            // inside it: most of what crosses the pipe is text.
            let text = match &mut read.text {
                Ok(Ok((text, _))) => std::mem::take(text).into_bytes(),
                _ => Vec::new(),
            };
            (serde_json::to_vec(&read).unwrap_or_default(), text)
        }
        OP_GRAPH => {
            let text = String::from_utf8_lossy(&payload);
            let graph = serve_graph(path, &text);
            (serde_json::to_vec(&graph).unwrap_or_default(), Vec::new())
        }
        OP_PATTERN => {
            let found = serve_pattern(&payload);
            (serde_json::to_vec(&found).unwrap_or_default(), Vec::new())
        }
        OP_IMAGE => {
            let _ = crate::contained(|| ::image::load_from_memory(&payload).map(|_| ()));
            (b"null".to_vec(), Vec::new())
        }
        _ => (b"null".to_vec(), Vec::new()),
    }
}

/// Test hooks, compiled into debug builds only: name a file and the worker
/// aborts on it, hangs on it or allocates past its limit on it, so the crash and deadline paths are proved on
/// every runner without a hostile file.
fn fault(path: &Path) {
    #[cfg(debug_assertions)]
    {
        let named = |var: &str| {
            std::env::var(var).is_ok_and(|name| {
                path.file_name()
                    .is_some_and(|n| n.to_string_lossy() == name)
            })
        };
        if named("SEMLITH_FAULT_ABORT") {
            std::process::abort();
        }
        if named("SEMLITH_FAULT_HANG") {
            std::thread::sleep(Duration::from_secs(3600));
        }
        if named("SEMLITH_FAULT_ALLOC") {
            let hog: Vec<u8> = Vec::with_capacity(usize::MAX / 4);
            std::hint::black_box(hog);
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = path;
}

// ------------------------------------------------------------------- memory

/// The allocator the binary runs on: the system's, counted against a limit
/// only inside a parse worker. Outside one the limit is zero and every call
/// is one relaxed load more than the system allocator.
pub struct Counting;

static LIMIT: AtomicUsize = AtomicUsize::new(0);
/// Signed: what was allocated before the limit was set is freed after it.
static USED: AtomicIsize = AtomicIsize::new(0);

fn take(size: usize) -> bool {
    let limit = LIMIT.load(Ordering::Relaxed);
    if limit == 0 {
        return true;
    }
    let after = USED.fetch_add(size as isize, Ordering::Relaxed) + size as isize;
    if after > limit as isize {
        USED.fetch_sub(size as isize, Ordering::Relaxed);
        return false;
    }
    true
}

fn give(size: usize) {
    if LIMIT.load(Ordering::Relaxed) != 0 {
        USED.fetch_sub(size as isize, Ordering::Relaxed);
    }
}

// SAFETY: every call goes straight to `System` with the caller's own layout
// and pointer; the counting only decides whether to call it at all, and a
// refused allocation returns null, which is the documented way to fail.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !take(layout.size()) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            give(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !take(layout.size()) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc_zeroed(layout) };
        if p.is_null() {
            give(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        give(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let grow = new_size.saturating_sub(layout.size());
        if !take(grow) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if p.is_null() {
            give(grow);
        } else {
            give(layout.size().saturating_sub(new_size));
        }
        p
    }
}

fn limit_memory(limit: usize) {
    LIMIT.store(limit, Ordering::Relaxed);
    // ponytail: the counting allocator bounds Rust's allocations everywhere;
    // tree-sitter's own mallocs are bounded on Linux here and on Windows by the
    // job object, and on macOS only by the deadline — Darwin refuses a lower
    // RLIMIT_AS. Route tree-sitter through `set_allocator` if that matters.
    #[cfg(target_os = "linux")]
    {
        let mapped = std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse::<usize>().ok())
            .unwrap_or(0)
            * 4096;
        let cap = (mapped + limit + 512 * 1024 * 1024) as libc::rlim_t;
        let rlimit = libc::rlimit {
            rlim_cur: cap,
            rlim_max: cap,
        };
        // SAFETY: a live local passed by pointer to a call that only reads it.
        unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlimit) };
    }
}

// ------------------------------------------------------------------ journal

/// The file under a store's `logs` folder that remembers which files crashed
/// a parser.
pub const JOURNAL: &str = "parse-journal";

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Line {
    Begin {
        path: String,
        hash: String,
    },
    End {
        path: String,
    },
    Crash {
        path: String,
        hash: String,
        reason: String,
    },
    /// Begun and never ended when a process stopped, once.
    Suspect {
        path: String,
        hash: String,
    },
    Clear {
        path: String,
    },
}

/// A file a parser crashed on, refused until its content changes or a person
/// asks for one more try.
#[derive(Debug, Clone, Serialize)]
pub struct Poisoned {
    pub path: String,
    pub hash: String,
    pub reason: String,
}

/// The poison journal: a line before each file goes to a parser and a line
/// after, so a crash that took the whole process with it is still known at
/// the next start.
///
/// A file still begun at a start is suspect; begun again and left again by a
/// second process, it is refused. Two, not one, because the daemon stopping
/// for any other reason — `semlith stop`, a reboot — leaves the files it was
/// reading begun too, and refusing those would refuse healthy files. A worker
/// crash is known for what it is at once and is refused the first time.
///
/// Plain writes, no fsync: the failure this guards against is the process
/// dying, after which what it wrote is still in the kernel's cache and reaches
/// the file. Power loss mid-parse costs one more try, not a loop.
pub struct Journal {
    file: Mutex<std::fs::File>,
    poisoned: HashMap<String, Poisoned>,
}

fn journal_path(store_dir: &Path) -> PathBuf {
    store_dir.join(crate::daemon::LOGS_DIR).join(JOURNAL)
}

/// What a journal file says now: refused files, and suspect ones.
fn replay(path: &Path) -> (HashMap<String, Poisoned>, HashMap<String, String>) {
    let mut pending: HashMap<String, String> = HashMap::new();
    let mut suspect: HashMap<String, String> = HashMap::new();
    let mut poisoned: HashMap<String, Poisoned> = HashMap::new();
    let text = std::fs::read_to_string(path).unwrap_or_default();
    for line in text.lines() {
        let Ok(line) = serde_json::from_str::<Line>(line) else {
            continue;
        };
        match line {
            Line::Begin { path, hash } => {
                pending.insert(path, hash);
            }
            Line::End { path } | Line::Clear { path } => {
                pending.remove(&path);
                suspect.remove(&path);
                poisoned.remove(&path);
            }
            Line::Crash { path, hash, reason } => {
                pending.remove(&path);
                suspect.remove(&path);
                poisoned.insert(path.clone(), Poisoned { path, hash, reason });
            }
            Line::Suspect { path, hash } => {
                suspect.insert(path, hash);
            }
        }
    }
    for (path, hash) in pending {
        if suspect.get(&path) == Some(&hash) {
            suspect.remove(&path);
            poisoned.insert(
                path.clone(),
                Poisoned {
                    path,
                    hash,
                    reason: "semlith stopped twice while parsing this file".to_string(),
                },
            );
        } else {
            suspect.insert(path, hash);
        }
    }
    (poisoned, suspect)
}

impl Journal {
    /// Open the store's journal for a run: replay it, write back only what
    /// still matters, and append from there.
    pub fn open(store_dir: &Path) -> anyhow::Result<Self> {
        let path = journal_path(store_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let (poisoned, suspect) = replay(&path);
        let mut compact = String::new();
        for p in poisoned.values() {
            compact.push_str(&line(&Line::Crash {
                path: p.path.clone(),
                hash: p.hash.clone(),
                reason: p.reason.clone(),
            }));
        }
        for (path, hash) in &suspect {
            compact.push_str(&line(&Line::Suspect {
                path: path.clone(),
                hash: hash.clone(),
            }));
        }
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, compact)?;
        std::fs::rename(&temp, &path)?;
        let file = std::fs::OpenOptions::new().append(true).open(&path)?;
        Ok(Self {
            file: Mutex::new(file),
            poisoned,
        })
    }

    /// Why this version of the file is refused, when a parser crashed on it.
    pub fn refused(&self, path: &str, hash: &str) -> Option<&str> {
        self.poisoned
            .get(path)
            .filter(|p| p.hash == hash)
            .map(|p| p.reason.as_str())
    }

    fn write(&self, l: &Line) {
        let mut file = self.file.lock().unwrap_or_else(|p| p.into_inner());
        let _ = file.write_all(line(l).as_bytes());
    }

    /// Run `parse` on one file between a begin and an end line, recording a
    /// crash in place of the end.
    pub fn around<T>(
        &self,
        path: &str,
        hash: &str,
        parse: impl FnOnce() -> Result<T, Stop>,
    ) -> Result<T, Stop> {
        self.write(&Line::Begin {
            path: path.to_string(),
            hash: hash.to_string(),
        });
        let answer = parse();
        match &answer {
            Err(Stop::Crashed(reason)) => self.write(&Line::Crash {
                path: path.to_string(),
                hash: hash.to_string(),
                reason: reason.clone(),
            }),
            _ => self.write(&Line::End {
                path: path.to_string(),
            }),
        }
        answer
    }
}

fn line(l: &Line) -> String {
    let mut s = serde_json::to_string(l).unwrap_or_default();
    s.push('\n');
    s
}

/// Let one refused file be parsed once more: a person asked for it.
pub fn clear(store_dir: &Path, path: &str) -> anyhow::Result<()> {
    let at = journal_path(store_dir);
    if !at.exists() {
        return Ok(());
    }
    let mut file = std::fs::OpenOptions::new().append(true).open(at)?;
    file.write_all(
        line(&Line::Clear {
            path: path.to_string(),
        })
        .as_bytes(),
    )?;
    Ok(())
}

/// Every file the store's journal refuses, for `doctor` and the portal.
pub fn refused_files(store_dir: &Path) -> Vec<Poisoned> {
    let mut list: Vec<Poisoned> = replay(&journal_path(store_dir)).0.into_values().collect();
    list.sort_by(|a, b| a.path.cmp(&b.path));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn a_crash_is_refused_at_once_and_a_new_version_is_not() {
        let dir = store();
        let j = Journal::open(dir.path()).unwrap();
        let r: Result<(), Stop> = j.around("/a.md", "h1", || {
            Err(Stop::Crashed("parser crashed: SIGABRT".into()))
        });
        assert!(r.is_err());
        let j = Journal::open(dir.path()).unwrap();
        assert_eq!(j.refused("/a.md", "h1"), Some("parser crashed: SIGABRT"));
        assert_eq!(j.refused("/a.md", "h2"), None);
        let _ = j.around("/a.md", "h2", || Ok::<_, Stop>(()));
        let j = Journal::open(dir.path()).unwrap();
        assert_eq!(j.refused("/a.md", "h1"), None);
    }

    #[test]
    fn a_process_that_dies_mid_parse_twice_refuses_the_file() {
        let dir = store();
        // Begun and never ended: the process died while parsing.
        let die = |dir: &Path| {
            let j = Journal::open(dir).unwrap();
            j.write(&Line::Begin {
                path: "/b.yaml".into(),
                hash: "h".into(),
            });
        };
        die(dir.path());
        let j = Journal::open(dir.path()).unwrap();
        assert_eq!(
            j.refused("/b.yaml", "h"),
            None,
            "once is a stop, not a crash"
        );
        j.write(&Line::Begin {
            path: "/b.yaml".into(),
            hash: "h".into(),
        });
        drop(j);
        let j = Journal::open(dir.path()).unwrap();
        assert!(j.refused("/b.yaml", "h").is_some());
        assert_eq!(refused_files(dir.path()).len(), 1);
    }

    #[test]
    fn a_stop_then_a_clean_parse_forgets_the_file() {
        let dir = store();
        let j = Journal::open(dir.path()).unwrap();
        j.write(&Line::Begin {
            path: "/c.md".into(),
            hash: "h".into(),
        });
        drop(j);
        let j = Journal::open(dir.path()).unwrap();
        let _ = j.around("/c.md", "h", || Ok::<_, Stop>(()));
        drop(j);
        let j = Journal::open(dir.path()).unwrap();
        j.write(&Line::Begin {
            path: "/c.md".into(),
            hash: "h".into(),
        });
        drop(j);
        assert_eq!(
            Journal::open(dir.path()).unwrap().refused("/c.md", "h"),
            None
        );
    }

    #[test]
    fn clearing_lets_the_file_be_tried_again() {
        let dir = store();
        let j = Journal::open(dir.path()).unwrap();
        let _ = j.around("/d.md", "h", || {
            Err::<(), _>(Stop::Crashed("parser crashed: SIGABRT".into()))
        });
        drop(j);
        clear(dir.path(), "/d.md").unwrap();
        assert!(refused_files(dir.path()).is_empty());
    }
}
