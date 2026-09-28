//! The llama.cpp lane: ggml-org's `llama-server`, pinned by build, serving
//! granite as GGUF f16 on Metal (macOS) or Vulkan (Windows and Linux).
//!
//! The server is a child of the worker and nothing else reaches it: it
//! listens on a loopback port the worker picked, takes a bearer key the
//! worker drew from the OS random source and never wrote down, and has its
//! web UI off. The worker hands it token ids verbatim — the run's own
//! tokenizer already added granite's CLS and SEP — through `POST /embedding`,
//! so llama.cpp's tokenizer never sees a chunk.
//!
//! The server dies with the worker however the worker ends: dropped on a
//! clean exit, and otherwise by a watchdog (a forked process on unix, a
//! kill-on-close job object on Windows), because the daemon stops an idle
//! worker by killing it outright and an orphaned server would keep the GPU's
//! memory for ever.

use anyhow::{Context, Result, bail};
use std::io::BufRead;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `cpu` runs the server with no GPU (`--device none -ngl 0`): how CI proves
/// the known answer on a runner with no GPU.
pub const DEVICE_ENV: &str = "SEMLITH_LLAMA_DEVICE";

/// The llama.cpp build: tag b11146, commit 7fe450e (llama.cpp 0.5.0).
pub const BUILD: &str = "b11146";

/// The GGUF's pin. CI's pack job (`packs.yml`) builds and publishes the
/// file, and its digest and size are pinned here once it has.
pub const LLAMA_GGUF_SHA256: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
pub const LLAMA_GGUF_SIZE: u64 = 0;
const GGUF: &str = "granite-embedding-small-english-r2-f16.gguf";

const GGUF_ASSET: crate::packs::Asset = crate::packs::Asset {
    url: "https://github.com/semlith/semlith/releases/download/pack-llama-v1/granite-embedding-small-english-r2-f16.gguf",
    sha256: LLAMA_GGUF_SHA256,
    size: LLAMA_GGUF_SIZE,
    form: crate::packs::Form::File { name: GGUF },
};

const MACOS: &[crate::packs::Asset] = &[
    crate::packs::Asset {
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b11146/llama-b11146-bin-macos-arm64.tar.gz",
        sha256: "1ad3f9eff80edb9dbef4259ad564d1720612ef7eea48fa4afed0e54f5f3d5711",
        size: 11_189_714,
        form: crate::packs::Form::TarGz,
    },
    GGUF_ASSET,
];

const LINUX: &[crate::packs::Asset] = &[
    crate::packs::Asset {
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b11146/llama-b11146-bin-ubuntu-vulkan-x64.tar.gz",
        sha256: "d3ce40fce7403cc93bcf5718fc46c6efb61ed9709f8e5d9f10c86bf0e30e8fb3",
        size: 30_598_492,
        form: crate::packs::Form::TarGz,
    },
    GGUF_ASSET,
];

const WINDOWS: &[crate::packs::Asset] = &[
    crate::packs::Asset {
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b11146/llama-b11146-bin-win-vulkan-x64.zip",
        sha256: "55a378aa095b466979d85075234f66d7655c7a7483222af0c006c0e55b4d7bd6",
        size: 32_127_004,
        // The Windows zip has its files at the root, with no directory to
        // drop; an empty prefix keeps every root file and nothing nested.
        form: crate::packs::Form::Wheel { prefix: "" },
    },
    GGUF_ASSET,
];

/// `llama-server` for this platform and the GGUF; no assets where no build
/// is pinned.
pub fn pack() -> crate::packs::Pack {
    crate::packs::Pack {
        name: "llama",
        version: "b11146-1",
        assets: if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            MACOS
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            LINUX
        } else if cfg!(all(windows, target_arch = "x86_64")) {
            WINDOWS
        } else {
            &[]
        },
    }
}

fn server_file() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// Parallel slots, and the context each one gets: a chunk is at most 400
/// tokens (`chunk::MAX_CHARS / 2`), so 512 holds any of them. The physical
/// batch holds two full chunks, because an embedding model needs a whole
/// sequence inside one ubatch.
///
/// ponytail: four slots and a 1024-token ubatch are the research run's
/// settings, not a measured optimum; tune them against the hello's pace.
const SLOTS: usize = 4;
const SLOT_CONTEXT: usize = 512;
const UBATCH: usize = 1024;

/// How long a server may take to load the model before the lane gives up.
const READY_DEADLINE: Duration = Duration::from_secs(120);

/// The GPU devices `llama-server --list-devices` printed, as (id, name):
/// `MTL0` on a Mac, `Vulkan0` and on elsewhere. A software renderer is
/// never one of them, for the reason the WebGPU lane refuses it.
pub fn gpu_devices(listing: &str) -> Vec<(String, String)> {
    listing
        .lines()
        .filter_map(|line| {
            let (id, rest) = line.trim().split_once(": ")?;
            if !(id.starts_with("MTL") || id.starts_with("Vulkan")) {
                return None;
            }
            let name = rest.split(" (").next().unwrap_or(rest).trim().to_string();
            (!crate::gpu::is_software(0, &name)).then(|| (id.to_string(), name))
        })
        .collect()
}

/// A `POST /embedding` answer as one normalised vector per row, in request
/// order. The server answers with an array of `{index, embedding}` (an object
/// alone for one row), and a pooled embedding comes either bare or wrapped in
/// a one-element array depending on the build.
pub fn vectors_from(answer: &serde_json::Value, rows: usize) -> Result<Vec<Vec<f32>>> {
    let items = match answer {
        serde_json::Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    let mut indexed = Vec::with_capacity(items.len());
    for (at, item) in items.iter().enumerate() {
        let index = item["index"].as_u64().map_or(at, |i| i as usize);
        let mut embedding = &item["embedding"];
        if embedding[0].is_array() {
            embedding = &embedding[0];
        }
        let vector: Vec<f32> = embedding
            .as_array()
            .context("an embedding answer with no vector")?
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|v| v as f32)
                    .context("a vector with a value that is not a number")
            })
            .collect::<Result<_>>()?;
        indexed.push((index, vector));
    }
    indexed.sort_by_key(|(index, _)| *index);
    if indexed.len() != rows {
        bail!(
            "llama-server answered {} vectors for {rows} chunks",
            indexed.len()
        );
    }
    Ok(indexed
        .into_iter()
        .map(|(_, mut vector)| {
            crate::normalize(&mut vector);
            vector
        })
        .collect())
}

// --------------------------------------------------------------- the session

/// What the llama.cpp worker embeds with: a running server and how to reach
/// it.
pub struct Session {
    server: Server,
    agent: ureq::Agent,
    url: String,
    key: String,
    device: String,
}

impl Session {
    /// Start `llama-server` from the pack on a loopback port, wait for it to
    /// load the model, and keep it for the life of the worker.
    pub fn open(pack: &Path) -> Result<Self> {
        if let Some(why) = crate::accel::unavailable_here("llama") {
            bail!("unavailable — {why}");
        }
        let server = pack.join(server_file());
        let model = pack.join(GGUF);
        for needed in [&server, &model] {
            if !needed.exists() {
                bail!(
                    "unavailable — {} is not in the llama.cpp pack; turning the lane off and on \
                     again fetches it afresh",
                    crate::plain(&needed.display().to_string())
                );
            }
        }
        let cpu = std::env::var(DEVICE_ENV).is_ok_and(|v| v.trim().eq_ignore_ascii_case("cpu"));
        let (placement, device) = if cpu {
            (
                vec!["--device".into(), "none".into(), "-ngl".into(), "0".into()],
                format!("{} CPU (llama.cpp)", crate::system::cpu_name()),
            )
        } else {
            let (id, name) = first_gpu(&server)?;
            let backend = if id.starts_with("MTL") {
                "Metal"
            } else {
                "Vulkan"
            };
            (
                vec!["--device".into(), id, "-ngl".into(), "99".into()],
                format!("{name} (llama.cpp {backend})"),
            )
        };

        let key = random_key()?;
        // Picked here and released for the server to take: a port another
        // process takes in between fails this start, and the lane's next
        // start picks again.
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| listener.local_addr())
            .context("finding a free loopback port")?
            .port();
        let mut args: Vec<String> = vec![
            "--embeddings".into(),
            "--pooling".into(),
            "cls".into(),
            "--host".into(),
            "127.0.0.1".into(),
            "--port".into(),
            port.to_string(),
            "--api-key".into(),
            key.clone(),
            "--no-webui".into(),
            "-m".into(),
            crate::plain(&model.display().to_string()),
            "-c".into(),
            (SLOTS * SLOT_CONTEXT).to_string(),
            "-b".into(),
            UBATCH.to_string(),
            "-ub".into(),
            UBATCH.to_string(),
            "-np".into(),
            SLOTS.to_string(),
        ];
        args.extend(placement);
        let mut child = std::process::Command::new(&server)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("unavailable — llama-server would not start: {e}"))?;
        let tail = Tail::follow(child.stderr.take());
        let server = Server::guard(child);

        let agent: ureq::Agent = ureq::Agent::config_builder()
            // Loopback, never through a proxy the environment names: the
            // bearer key must not leave this machine.
            .proxy(None)
            .timeout_global(Some(Duration::from_secs(300)))
            .build()
            .into();
        let url = format!("http://127.0.0.1:{port}");
        let mut session = Self {
            server,
            agent,
            url,
            key,
            device,
        };
        session.wait_ready(&tail)?;
        Ok(session)
    }

    /// `/health` answers 503 while the model loads and 200 once it has. A
    /// server that exits first is the lane being unavailable here — no
    /// Vulkan loader, a driver that refuses, a model it cannot read — and
    /// the reason is the end of what it printed.
    fn wait_ready(&mut self, tail: &Tail) -> Result<()> {
        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = self.server.child.try_wait() {
                bail!(
                    "unavailable — llama-server exited ({status}) before it was ready: {}",
                    tail.text()
                );
            }
            let health = self
                .agent
                .get(format!("{}/health", self.url))
                .config()
                .timeout_global(Some(Duration::from_secs(2)))
                .build()
                .call();
            if health.is_ok() {
                return Ok(());
            }
            if started.elapsed() > READY_DEADLINE {
                bail!(
                    "unavailable — llama-server was not ready within {} s: {}",
                    READY_DEADLINE.as_secs(),
                    tail.text()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn device(&self) -> String {
        self.device.clone()
    }

    pub fn variant(&self) -> &'static str {
        "gguf-f16"
    }

    pub fn embed(&mut self, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        let body = serde_json::json!({ "content": batch }).to_string();
        let mut response = self
            .agent
            .post(format!("{}/embedding", self.url))
            .header("Authorization", format!("Bearer {}", self.key))
            .content_type("application/json")
            .send(body.as_bytes())
            .map_err(|e| anyhow::anyhow!("llama-server: {e}"))?;
        // A batch of short chunks can be thousands of rows; ureq's 10 MB
        // default would refuse the answer.
        let bytes = response
            .body_mut()
            .with_config()
            .limit(1 << 30)
            .read_to_vec()
            .map_err(|e| anyhow::anyhow!("reading llama-server's answer: {e}"))?;
        let answer: serde_json::Value =
            serde_json::from_slice(&bytes).context("llama-server's answer is not JSON")?;
        vectors_from(&answer, batch.len())
    }
}

/// The first hardware GPU this llama.cpp build can offload to, or why there
/// is none. Asked of the server itself, so a machine whose Vulkan loader or
/// driver is missing says so here rather than silently embedding on the CPU.
fn first_gpu(server: &Path) -> Result<(String, String)> {
    let out = std::process::Command::new(server)
        .arg("--list-devices")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| anyhow::anyhow!("unavailable — llama-server would not start: {e}"))?;
    let listing = String::from_utf8_lossy(&out.stdout);
    if let Some(first) = gpu_devices(&listing).into_iter().next() {
        return Ok(first);
    }
    if !out.status.success() {
        // The first line is the loader's or the driver's own reason (a
        // missing library, no Vulkan loader); what follows is detail.
        let stderr = String::from_utf8_lossy(&out.stderr);
        let said = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        bail!(
            "unavailable — llama-server would not start ({}): {}",
            out.status,
            clip(said)
        );
    }
    bail!(
        "unavailable — llama.cpp found no {} GPU on this machine",
        if cfg!(target_os = "macos") {
            "Metal"
        } else {
            "Vulkan"
        }
    )
}

/// One line of a server's output, cut to what an error message can carry.
fn clip(line: &str) -> String {
    let line = line.trim();
    match line.char_indices().nth(200) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

/// 128 bits from the OS random source, as hex. Never logged or stored.
fn random_key() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("the OS random source: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The last lines the server printed, kept for the reason it gives when it
/// fails to start. Reading its stderr also keeps a chatty server from
/// blocking on a full pipe.
struct Tail(Arc<Mutex<std::collections::VecDeque<String>>>);

impl Tail {
    fn follow(stderr: Option<std::process::ChildStderr>) -> Self {
        let lines = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Some(stderr) = stderr {
            let kept = Arc::clone(&lines);
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(stderr)
                    .lines()
                    .map_while(|l| l.ok())
                {
                    let mut kept = kept.lock().unwrap_or_else(|e| e.into_inner());
                    if kept.len() == 8 {
                        kept.pop_front();
                    }
                    kept.push_back(line);
                }
            });
        }
        Self(lines)
    }

    fn text(&self) -> String {
        // A moment for the reader to catch the last lines of a server that
        // just exited.
        std::thread::sleep(Duration::from_millis(200));
        let lines = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let text = lines
            .iter()
            .map(|l| clip(l))
            .collect::<Vec<_>>()
            .join(" | ");
        if text.is_empty() {
            "it printed nothing".to_string()
        } else {
            text
        }
    }
}

/// The running server, killed when dropped, and tied to this process so it
/// is killed when the worker is too.
struct Server {
    child: std::process::Child,
    #[cfg(unix)]
    watchdog: Option<libc::pid_t>,
}

impl Server {
    fn guard(child: std::process::Child) -> Self {
        #[cfg(unix)]
        let watchdog = watchdog(child.id() as libc::pid_t);
        #[cfg(windows)]
        kill_on_close(&child);
        Self {
            child,
            #[cfg(unix)]
            watchdog,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        #[cfg(unix)]
        if let Some(pid) = self.watchdog {
            // SAFETY: the watchdog is this process's own child, not yet
            // reaped, so the pid cannot have been reused; it is signalled and
            // then reaped once.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, std::ptr::null_mut(), 0);
            }
        }
    }
}

/// A forked process that kills the server once this one is gone: its parent
/// changes when the worker dies, however it dies. `None` if the fork failed,
/// in which case the drop guard alone stands.
#[cfg(unix)]
fn watchdog(server: libc::pid_t) -> Option<libc::pid_t> {
    let worker = std::process::id() as libc::pid_t;
    // SAFETY: the child of a fork in a threaded process may only make
    // async-signal-safe calls, and it makes nothing else: close, sleep,
    // getppid, kill and _exit. It never returns into Rust code, allocates or
    // takes a lock. Closing 0-2 drops its copy of the worker's pipes to the
    // daemon, so the daemon sees the worker's end when the worker ends.
    match unsafe { libc::fork() } {
        -1 => None,
        0 => unsafe {
            for fd in 0..3 {
                libc::close(fd);
            }
            loop {
                libc::sleep(1);
                if libc::getppid() != worker {
                    libc::kill(server, libc::SIGKILL);
                    libc::_exit(0);
                }
            }
        },
        pid => Some(pid),
    }
}

/// Put the server in a job object that kills what it holds when its last
/// handle closes. The handle is never closed by this code, so it closes when
/// the worker exits, however it exits.
#[cfg(windows)]
fn kill_on_close(child: &std::process::Child) {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    #[derive(Default)]
    struct Basic {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    /// JOBOBJECT_EXTENDED_LIMIT_INFORMATION, with IO_COUNTERS as six u64s.
    #[repr(C)]
    #[derive(Default)]
    struct Extended {
        basic: Basic,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }
    #[cfg(target_arch = "x86_64")]
    const _: () = assert!(size_of::<Extended>() == 144);
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: i32,
            info: *const c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    }

    let info = Extended {
        basic: Basic {
            limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: an anonymous job with default security; the limit structure is
    // passed with its own size, which is the documented contract, and only
    // read. The process handle is the child's own, alive while `child` is.
    // Every failure leaves the drop guard as the only guard, which is what a
    // build without job objects would have.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return;
        }
        if SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
            (&info as *const Extended).cast(),
            size_of::<Extended>() as u32,
        ) != 0
        {
            AssignProcessToJobObject(job, child.as_raw_handle().cast());
        }
    }
}
