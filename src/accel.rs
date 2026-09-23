//! The CPU and the GPU sharing every index run.
//!
//! An index run's window of sorted chunks is split into batches that *lanes*
//! pull when they are free. The CPU lane is the run's own in-process session,
//! as it always was. A GPU lane is a worker process — this same binary started
//! as `semlith __embed-worker <lane>` — speaking length-framed batches over its
//! stdin and stdout, shared by every run in the daemon.
//!
//! The worker process is the isolation boundary, and the reason there is one.
//! A GPU driver that crashes or aborts kills the worker, never the daemon; the
//! batch it held goes back to the queue for another lane, the lane is marked
//! failed with its reason, and the run completes with the same files and
//! chunks. Its GPU memory goes with it when it exits after an idle minute. And
//! a CUDA worker can load a different ONNX Runtime from the daemon's.
//!
//! Lanes: `gpu` is WebGPU (Metal, D3D12 or Vulkan) through Microsoft's plugin
//! execution provider and the fp16 variant of the model; `cuda` is NVIDIA's
//! runtime through a pack fetched only on an explicit turn-on; `worker` is a
//! CPU-backed worker, never on by default, which is how the whole worker path
//! — spawn, handshake, queue, failure, fallback — is checked on machines that
//! have no GPU at all, which is every CI runner but one.
//!
//! Before a lane's first real batch the worker embeds 32 fixed chunks and
//! compares them with CPU fp32 vectors committed to the repository. A lane
//! whose cosine falls below [`MIN_COSINE_FP16`] is refused rather than used:
//! a GPU that produces wrong vectors fails silently otherwise, because the run
//! succeeds and search simply gets worse.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

/// Chunks a GPU lane takes at once. Measured on the M1's Metal: 50.6 chunks/s
/// at 16, length-sorted, against 19.5 on the CPU path of the time.
pub const GPU_BATCH: usize = 16;

/// Chunks the CUDA lane takes at once. A discrete card is starved by less.
pub const CUDA_BATCH: usize = 64;

/// How long one batch may take on a lane before the lane is failed. A hung
/// driver is a failed lane, not a run that never ends.
const BATCH_DEADLINE: Duration = Duration::from_secs(30);

/// Undocumented override for [`BATCH_DEADLINE`], in milliseconds, so a test
/// can drive the hang path without waiting half a minute.
const DEADLINE_ENV: &str = "SEMLITH_ACCEL_DEADLINE_MS";

/// How long a worker is kept with nothing to do. Its GPU memory goes with it.
const WORKER_IDLE: Duration = Duration::from_secs(60);

/// The lowest cosine against the fp32 fixture an fp16 lane may show, on every
/// one of the 32 chunks.
pub const MIN_COSINE_FP16: f32 = 0.999;

/// The same floor for the int8 CPU-backed worker, whose quantisation costs
/// about 0.013 on its own (measured 0.987 on the 2026-09-23 corpus).
const MIN_COSINE_INT8: f32 = 0.97;

/// Which lanes are on: a comma list such as `cpu,gpu`. Takes precedence over
/// the saved setting, like the other limit variables.
pub const ACCEL_ENV: &str = "SEMLITH_ACCEL";

/// Fault injection for the worker, read only by the worker:
/// `<lane>:load`, `<lane>:batch:N` (fail the Nth batch) or `<lane>:hang:MS`.
pub const FAULT_ENV: &str = "SEMLITH_FAULT_ACCEL";

/// The known-answer fixture: 32 chunks and their CPU fp32 vectors.
const FIXTURE_TEXTS: &str = include_str!("../tests/fixtures/gpu/known-answer.json");
const FIXTURE_VECTORS: &[u8] = include_bytes!("../tests/fixtures/gpu/known-answer.f32");

const DIM: usize = 384;

// ------------------------------------------------------------------ settings

/// The three switches, as `settings.json` holds them.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Switches {
    pub cpu: Option<bool>,
    pub gpu: Option<bool>,
    pub cuda: Option<bool>,
}

/// Which lanes are on, and where that came from.
#[derive(Debug, Clone, Serialize)]
pub struct Enabled {
    pub cpu: bool,
    pub gpu: bool,
    pub cuda: bool,
    /// The CPU-backed worker, for verification. Only the environment turns
    /// it on.
    pub worker: bool,
    pub source: &'static str,
}

/// The switches in force: the environment, then the saved setting, then the
/// defaults (CPU and GPU on; CUDA off, because its pack is 1 to 2.6 GB and is
/// fetched only when somebody asks for it).
pub fn enabled() -> Enabled {
    if let Ok(list) = std::env::var(ACCEL_ENV) {
        let has = |name: &str| list.split(',').any(|item| item.trim() == name);
        return Enabled {
            cpu: has("cpu"),
            gpu: has("gpu"),
            cuda: has("cuda"),
            worker: has("worker"),
            source: "set by the environment",
        };
    }
    let saved = crate::home::Settings::load().accelerators;
    Enabled {
        cpu: saved.cpu.unwrap_or(true),
        gpu: saved.gpu.unwrap_or(true),
        cuda: saved.cuda.unwrap_or(false),
        worker: false,
        source: if saved == Switches::default() {
            "default"
        } else {
            "saved"
        },
    }
}

// --------------------------------------------------------------------- lanes

/// Where a lane stands, as the Machine limits card shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Status {
    /// Enabled and not yet asked for anything, or its worker exited idle.
    Idle,
    Starting,
    Active,
    Downloading {
        percent: u8,
    },
    Unavailable {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

struct Job {
    texts: Vec<String>,
    reply: mpsc::Sender<std::result::Result<Vec<Vec<f32>>, String>>,
}

/// One accelerator lane, shared by every run in the process.
pub struct Lane {
    pub id: &'static str,
    status: Mutex<Status>,
    device: Mutex<Option<String>>,
    variant: &'static str,
    /// Chunks this lane has embedded, and when, for its live share.
    samples: Mutex<std::collections::VecDeque<(Instant, u64)>>,
    chunks: AtomicU64,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
}

impl Lane {
    fn new(id: &'static str, variant: &'static str) -> Self {
        Self {
            id,
            status: Mutex::new(Status::Idle),
            device: Mutex::new(None),
            variant,
            samples: Mutex::new(std::collections::VecDeque::new()),
            chunks: AtomicU64::new(0),
            jobs: Mutex::new(None),
        }
    }

    pub fn batch(&self) -> usize {
        if self.id == "cuda" {
            CUDA_BATCH
        } else {
            GPU_BATCH
        }
    }

    pub fn variant(&self) -> &'static str {
        self.variant
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn set(&self, status: Status) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = status;
    }

    /// Whether a run may hand this lane a batch now.
    pub fn usable(&self) -> bool {
        !matches!(
            self.status(),
            Status::Unavailable { .. } | Status::Failed { .. }
        )
    }

    /// Clear a failure, so the next batch tries the lane again. A switch
    /// turned off and on is how a user asks for that.
    pub fn reset(&self) {
        if matches!(
            self.status(),
            Status::Failed { .. } | Status::Unavailable { .. }
        ) {
            self.set(Status::Idle);
        }
    }

    /// Hand one batch to the lane. The answer arrives on the returned
    /// receiver: vectors in the order of `texts`, or why the lane could not.
    pub fn submit(
        self: &Arc<Self>,
        texts: Vec<String>,
    ) -> mpsc::Receiver<std::result::Result<Vec<Vec<f32>>, String>> {
        let (reply, answer) = mpsc::channel();
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.is_none() {
            let (tx, rx) = mpsc::channel();
            let lane = Arc::clone(self);
            // A dispatcher thread per lane owns its worker, so a batch from
            // any run waits in one line for one device.
            if std::thread::Builder::new()
                .name(format!("semlith-lane-{}", self.id))
                .spawn(move || dispatch(lane, rx))
                .is_ok()
            {
                *jobs = Some(tx);
            }
        }
        let job = Job { texts, reply };
        match jobs.as_ref() {
            Some(tx) => {
                if let Err(mpsc::SendError(job)) = tx.send(job) {
                    *jobs = None;
                    let _ = job.reply.send(Err("the lane's dispatcher has gone".into()));
                }
            }
            None => {
                let _ = job
                    .reply
                    .send(Err("the lane's dispatcher could not start".into()));
            }
        }
        answer
    }

    /// Count chunks this lane embedded, for its share of the rate.
    pub fn count(&self, chunks: usize) {
        self.chunks.fetch_add(chunks as u64, Ordering::Relaxed);
        let mut samples = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        samples.push_back((Instant::now(), self.chunks.load(Ordering::Relaxed)));
        while samples
            .front()
            .is_some_and(|(at, _)| at.elapsed() > Duration::from_secs(10))
            && samples.len() > 1
        {
            samples.pop_front();
        }
    }

    /// Chunks per second over the last ten seconds.
    pub fn rate(&self) -> f64 {
        let samples = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        let (Some((first, from)), Some((_, to))) = (samples.front(), samples.back()) else {
            return 0.0;
        };
        let span = first.elapsed().as_secs_f64();
        if span < 0.5 {
            return 0.0;
        }
        (to - from) as f64 / span
    }
}

/// The in-process CPU lane's own count, for the card's share.
static CPU: OnceLock<Arc<Lane>> = OnceLock::new();

fn cpu_lane() -> &'static Arc<Lane> {
    CPU.get_or_init(|| Arc::new(Lane::new("cpu", "int8-cpu")))
}

/// Count chunks the in-process CPU lane embedded.
pub fn count_cpu(chunks: usize) {
    cpu_lane().count(chunks);
}

/// The worker lanes. Created on first use; only a daemon ever uses them.
static LANES: OnceLock<Vec<Arc<Lane>>> = OnceLock::new();

/// Whether this process hands batches to lanes at all. Only the daemon does:
/// a `semlith index` in a terminal, and the retrieval harness, stay on the
/// CPU alone, which is what keeps their figures reproducible (issue #88).
static MANAGED: AtomicBool = AtomicBool::new(false);

pub fn manage() {
    MANAGED.store(true, Ordering::Relaxed);
}

fn lanes() -> &'static Vec<Arc<Lane>> {
    LANES.get_or_init(|| {
        vec![
            Arc::new(Lane::new("gpu", "fp16-webgpu")),
            Arc::new(Lane::new("cuda", "fp16-cuda")),
            Arc::new(Lane::new("worker", "int8-cpu")),
        ]
    })
}

/// The worker lanes a run may hand batches to now, and whether its own CPU
/// session may take batches as well.
///
/// The CPU switch is honoured only while a worker lane is usable: with no GPU,
/// or a failed one, the CPU carries on as the fallback whatever the switch
/// says, and the card says so.
pub fn for_run() -> (Vec<Arc<Lane>>, bool) {
    if !MANAGED.load(Ordering::Relaxed) {
        return (Vec::new(), true);
    }
    let on = enabled();
    let lanes: Vec<Arc<Lane>> = lanes()
        .iter()
        .filter(|lane| match lane.id {
            "gpu" => on.gpu,
            "cuda" => on.cuda,
            "worker" => on.worker,
            _ => false,
        })
        .filter(|lane| lane.usable())
        .cloned()
        .collect();
    let cpu = on.cpu || lanes.is_empty();
    (lanes, cpu)
}

/// Every lane as the Machine limits card and `semlith accel status` show it.
pub fn snapshot() -> serde_json::Value {
    let on = enabled();
    let cpu = cpu_lane();
    let mut rows = vec![serde_json::json!({
        "lane": "cpu",
        "enabled": on.cpu,
        "status": Status::Active,
        "device": crate::system::cpu_name(),
        "variant": cpu.variant,
        "rate": round(cpu.rate()),
    })];
    for lane in lanes() {
        let enabled = match lane.id {
            "gpu" => on.gpu,
            "cuda" => on.cuda,
            _ => on.worker,
        };
        if lane.id == "worker" && !enabled {
            continue;
        }
        rows.push(serde_json::json!({
            "lane": lane.id,
            "enabled": enabled,
            "status": lane.status(),
            "device": lane.device.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            "variant": lane.variant,
            "rate": round(lane.rate()),
        }));
    }
    let total: f64 = rows.iter().filter_map(|row| row["rate"].as_f64()).sum();
    for row in &mut rows {
        let rate = row["rate"].as_f64().unwrap_or(0.0);
        row["share"] = serde_json::json!(if total > 0.0 {
            (rate / total * 100.0).round()
        } else {
            0.0
        });
    }
    let gpu_usable = lanes()
        .iter()
        .any(|lane| lane.id != "worker" && lane.usable() && lane.status() != Status::Idle);
    serde_json::json!({
        "lanes": rows,
        "source": on.source,
        // Said, not implied: the CPU carries the run whatever its switch says
        // while no worker lane can.
        "cpu_fallback": !on.cpu && !gpu_usable,
    })
}

fn round(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Turn a lane on or off, as the page's switch and `semlith accel` do.
///
/// Saved to `settings.json` and read by every run at its next window, which is
/// the "next batch" the contract promises. Turning a failed lane on again
/// clears its failure so it is tried afresh. The CPU may be turned off only
/// while a GPU lane can carry the work; with none, the refusal says why.
pub fn set(lane: &str, on: bool) -> Result<String> {
    if std::env::var(ACCEL_ENV).is_ok() {
        bail!("{ACCEL_ENV} is set in this process's environment, so the switches cannot change it");
    }
    let mut settings = crate::home::Settings::load();
    match (lane, on) {
        ("cpu", false) => {
            let gpu_on = enabled().gpu || enabled().cuda;
            let can = gpu_on && lanes().iter().any(|l| l.id != "worker" && l.usable())
                && detect_gpu().is_ok();
            if !can {
                bail!(
                    "the CPU cannot be turned off: no GPU lane is on and usable here ({}), so the CPU \
                     is what indexes",
                    detect_gpu().err().unwrap_or_else(|| "the GPU lane is off".to_string())
                );
            }
            settings.accelerators.cpu = Some(false);
        }
        ("cpu", true) => settings.accelerators.cpu = Some(true),
        ("gpu", _) => settings.accelerators.gpu = Some(on),
        ("cuda", _) => {
            if let Some(why) = crate::gpu::cuda_unavailable_here() {
                bail!("{why}");
            }
            settings.accelerators.cuda = Some(on);
        }
        (other, _) => bail!("there is no lane called {other}; the lanes are cpu, gpu and cuda"),
    }
    settings.save()?;
    if on && let Some(found) = lanes().iter().find(|l| l.id == lane) {
        found.reset();
    }
    Ok(format!(
        "{lane} {} — runs pick it up at their next batch",
        if on { "on" } else { "off" }
    ))
}

/// Delete a lane's downloaded components, and say how many bytes that freed.
/// Turning a lane off never does this; only asking does.
pub fn remove(lane: &str) -> Result<u64> {
    let cache = crate::model_cache_dir()?;
    let dir = match lane {
        "gpu" => component_dir(&cache, &format!("webgpu-{}", crate::gpu::WEBGPU_VERSION)),
        "cuda" => crate::gpu::cuda_dir(&cache),
        other => bail!("{other} has nothing downloaded to remove; the lanes with components are gpu and cuda"),
    };
    let bytes = dir_bytes(&dir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| {
            format!("removing {}", crate::plain(&dir.display().to_string()))
        })?;
    }
    Ok(bytes)
}

/// What each lane's downloaded components take on disk.
pub fn component_bytes() -> serde_json::Value {
    let Ok(cache) = crate::model_cache_dir() else {
        return serde_json::json!({});
    };
    serde_json::json!({
        "gpu": dir_bytes(&component_dir(&cache, &format!("webgpu-{}", crate::gpu::WEBGPU_VERSION))),
        "cuda": dir_bytes(&crate::gpu::cuda_dir(&cache)),
        "cuda_download": crate::gpu::cuda_pack_bytes(),
    })
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                dir_bytes(&path)
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

// ------------------------------------------------------------ the dispatcher

fn deadline() -> Duration {
    std::env::var(DEADLINE_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(BATCH_DEADLINE)
}

/// A running worker and the channel its answers arrive on.
struct Worker {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    answers: mpsc::Receiver<std::result::Result<Vec<u8>, String>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn dispatch(lane: Arc<Lane>, jobs: mpsc::Receiver<Job>) {
    let mut worker: Option<Worker> = None;
    loop {
        let job = match jobs.recv_timeout(WORKER_IDLE) {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Idle: the worker goes, and its device memory with it.
                if worker.take().is_some() && lane.status() == Status::Active {
                    lane.set(Status::Idle);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if !lane.usable() {
            let _ = job.reply.send(Err(reason_of(&lane.status())));
            continue;
        }
        if worker.is_none() {
            lane.set(Status::Starting);
            match start(&lane) {
                Ok((started, _)) => {
                    worker = Some(started);
                    lane.set(Status::Active);
                }
                Err(e) => {
                    let reason = format!("{e:#}");
                    // Nothing on this machine for the lane to use is not a
                    // failure; everything else is.
                    if reason.starts_with("unavailable") {
                        lane.set(Status::Unavailable {
                            reason: reason.trim_start_matches("unavailable — ").to_string(),
                        });
                    } else {
                        lane.set(Status::Failed {
                            reason: reason.clone(),
                        });
                    }
                    let _ = job.reply.send(Err(reason));
                    continue;
                }
            }
        }
        let running = worker.as_mut().expect("started above");
        match run_batch(running, &job.texts) {
            Ok(vectors) => {
                lane.count(vectors.len());
                let _ = job.reply.send(Ok(vectors));
            }
            Err(e) => {
                // The worker is gone or cannot be trusted: kill it, fail the
                // lane with the reason, and hand the batch back.
                worker = None;
                let reason = format!("{e:#}");
                lane.set(Status::Failed {
                    reason: reason.clone(),
                });
                let _ = job.reply.send(Err(reason));
            }
        }
    }
}

fn reason_of(status: &Status) -> String {
    match status {
        Status::Unavailable { reason } | Status::Failed { reason } => reason.clone(),
        other => format!("{other:?}"),
    }
}

fn run_batch(worker: &mut Worker, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let request = serde_json::to_vec(&serde_json::json!({ "texts": texts }))?;
    write_frame(&mut worker.stdin, &request).context("the worker stopped reading")?;
    let frame = match worker.answers.recv_timeout(deadline()) {
        Ok(Ok(frame)) => frame,
        Ok(Err(e)) => bail!("the worker exited: {e}"),
        Err(_) => bail!(
            "the worker did not answer a batch within {} s",
            deadline().as_secs_f32()
        ),
    };
    decode_vectors(&frame, texts.len())
}

/// Run the known-answer check on every lane this machine could use: the CPU
/// in this process, then each worker lane in a worker of its own. A lane with
/// nothing to run on is reported with the reason rather than as a failure.
///
/// What `semlith doctor --gpu` prints and the Doctor page shows. Components a
/// lane needs are fetched first, as a run would, and `say` narrates that.
pub fn check_all(say: impl Fn(&str)) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    say("checking the CPU lane");
    let cpu = (|| -> Result<Check> {
        let cache = crate::model_cache_dir()?;
        let mut model =
            crate::embed::Model::Granite.load(cache, crate::chunk::MAX_CHARS / 2, true)?;
        known_answer("cpu", &crate::system::cpu_name(), "int8-cpu", |texts| {
            let mut got = model
                .embed(texts, Some(1))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            for vector in &mut got {
                crate::normalize(vector);
            }
            Ok(got)
        })
    })();
    out.push(match cpu {
        Ok(check) => serde_json::to_value(check).unwrap_or_default(),
        Err(e) => serde_json::json!({ "lane": "cpu", "passed": false, "reason": format!("{e:#}") }),
    });
    let on = enabled();
    for (id, variant) in [
        ("gpu", "fp16-webgpu"),
        ("cuda", "fp16-cuda"),
        ("worker", "int8-cpu"),
    ] {
        if id == "worker" && !on.worker {
            continue;
        }
        say(&format!("checking the {id} lane"));
        let lane = Arc::new(Lane::new(
            if id == "gpu" {
                "gpu"
            } else if id == "cuda" {
                "cuda"
            } else {
                "worker"
            },
            variant,
        ));
        out.push(match start(&lane) {
            Ok((_, hello)) => serde_json::json!({
                "lane": id,
                "device": hello["device"],
                "variant": hello["variant"],
                "cosine": hello["cosine"],
                "chunks_per_s": hello["chunks_per_s"],
                "passed": true,
            }),
            Err(e) => {
                let text = format!("{e:#}");
                match text.strip_prefix("unavailable — ") {
                    Some(reason) => serde_json::json!({ "lane": id, "reason": format!("unavailable — {reason}") }),
                    None => serde_json::json!({ "lane": id, "passed": false, "reason": text }),
                }
            }
        });
    }
    out
}

/// Start a lane's worker: fetch what it needs, spawn it, and read its hello,
/// which carries the known-answer check.
fn start(lane: &Arc<Lane>) -> Result<(Worker, serde_json::Value)> {
    let args = match lane.id {
        "gpu" => {
            let device = detect_gpu().map_err(|why| anyhow::anyhow!("unavailable — {why}"))?;
            *lane.device.lock().unwrap_or_else(|e| e.into_inner()) = Some(device);
            let dir = fetch_webgpu(lane).context("fetching the WebGPU components")?;
            vec![
                "__embed-worker".to_string(),
                "gpu".to_string(),
                dir.display().to_string(),
            ]
        }
        "cuda" => bail!("unavailable — {}", crate::gpu::cuda_unavailable()),
        _ => vec!["__embed-worker".to_string(), "worker".to_string()],
    };
    let exe = std::env::current_exe().context("locating this binary")?;
    let mut child = std::process::Command::new(exe)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("starting the embed worker")?;
    let stdin = child.stdin.take().context("the worker has no stdin")?;
    let mut stdout = child.stdout.take().context("the worker has no stdout")?;
    let (tx, answers) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let frame = read_frame(&mut stdout).map_err(|e| e.to_string());
            let end = frame.is_err();
            if tx.send(frame).is_err() || end {
                return;
            }
        }
    });
    let worker = Worker {
        child,
        stdin,
        answers,
    };
    // The hello, and with it the known-answer check. Bounded like a batch:
    // it is the first thing the device is asked to do.
    let hello = match worker.answers.recv_timeout(deadline() * 4) {
        Ok(Ok(frame)) => frame,
        Ok(Err(e)) => bail!("the worker exited before it was ready: {e}"),
        Err(_) => bail!(
            "the worker was not ready within {} s",
            (deadline() * 4).as_secs()
        ),
    };
    let hello: serde_json::Value = serde_json::from_slice(&hello).context("the worker's hello")?;
    if hello["ok"].as_bool() != Some(true) {
        let reason = hello["reason"].as_str().unwrap_or("no reason given");
        if hello["unavailable"].as_bool() == Some(true) {
            bail!("unavailable — {reason}");
        }
        bail!("{reason}");
    }
    if let Some(device) = hello["device"].as_str() {
        *lane.device.lock().unwrap_or_else(|e| e.into_inner()) = Some(device.to_string());
    }
    Ok((worker, hello))
}

// ---------------------------------------------------------------- the frames

/// A frame is a little-endian `u32` length and that many bytes.
pub fn write_frame(out: &mut impl Write, body: &[u8]) -> std::io::Result<()> {
    out.write_all(&(body.len() as u32).to_le_bytes())?;
    out.write_all(body)?;
    out.flush()
}

pub fn read_frame(input: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    input.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    // A frame is a batch of texts or of vectors; 64 MB is far past either.
    if len > 64 << 20 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("a frame of {len} bytes"),
        ));
    }
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    Ok(body)
}

/// An answer frame: `0` then `n * DIM` little-endian `f32`, or `1` then why.
fn encode_vectors(vectors: &[Vec<f32>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + vectors.len() * DIM * 4);
    out.push(0);
    for vector in vectors {
        for value in vector {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

fn encode_error(why: &str) -> Vec<u8> {
    let mut out = vec![1];
    out.extend_from_slice(why.as_bytes());
    out
}

fn decode_vectors(frame: &[u8], expected: usize) -> Result<Vec<Vec<f32>>> {
    match frame.first() {
        Some(0) => {}
        Some(1) => bail!("{}", String::from_utf8_lossy(&frame[1..])),
        _ => bail!("the worker sent a frame it should not have"),
    }
    let body = &frame[1..];
    if body.len() != expected * DIM * 4 {
        bail!(
            "the worker sent {} bytes for {expected} vectors",
            body.len()
        );
    }
    Ok(body
        .chunks_exact(DIM * 4)
        .map(|vector| {
            vector
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect()
        })
        .collect())
}

// ------------------------------------------------------------ known answers

/// The fixture's texts and vectors.
pub fn fixture() -> (Vec<String>, Vec<Vec<f32>>) {
    let texts: Vec<String> = serde_json::from_str::<serde_json::Value>(FIXTURE_TEXTS)
        .ok()
        .and_then(|v| {
            v["texts"].as_array().map(|list| {
                list.iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default();
    let vectors = FIXTURE_VECTORS
        .chunks_exact(DIM * 4)
        .map(|vector| {
            vector
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect()
        })
        .collect();
    (texts, vectors)
}

/// What one lane made of the fixture.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub lane: String,
    pub device: String,
    pub variant: String,
    /// The lowest cosine against the fp32 fixture over all 32 chunks.
    pub cosine: f32,
    pub chunks_per_s: f64,
    pub passed: bool,
}

/// Embed the fixture with `embed` and score it against the committed vectors.
pub fn known_answer(
    lane: &str,
    device: &str,
    variant: &str,
    mut embed: impl FnMut(&[String]) -> Result<Vec<Vec<f32>>>,
) -> Result<Check> {
    let (texts, expected) = fixture();
    // Each chunk alone, as the fixture was made: padding would change the
    // answer and it is the device being checked, not the batching.
    let started = Instant::now();
    let mut worst = 1.0f32;
    for (text, want) in texts.iter().zip(&expected) {
        let got = embed(std::slice::from_ref(text))?;
        let got = got.first().context("no vector came back")?;
        worst = worst.min(crate::index::cosine(got, want));
    }
    let elapsed = started.elapsed().as_secs_f64().max(1e-6);
    let floor = if variant.starts_with("fp16") {
        MIN_COSINE_FP16
    } else {
        MIN_COSINE_INT8
    };
    Ok(Check {
        lane: lane.to_string(),
        device: device.to_string(),
        variant: variant.to_string(),
        cosine: worst,
        chunks_per_s: (texts.len() as f64 / elapsed * 10.0).round() / 10.0,
        passed: worst >= floor,
    })
}

// ----------------------------------------------------------------- the worker

/// `semlith __embed-worker <lane> [dir]`: load the lane's session, check it
/// against the fixture, say hello, then answer batches until stdin closes.
pub fn worker_main(lane: &str, dir: Option<&Path>) -> i32 {
    let mut out = std::io::stdout().lock();
    let mut input = std::io::stdin().lock();
    let hello = |out: &mut std::io::StdoutLock, value: serde_json::Value| {
        let _ = write_frame(out, &serde_json::to_vec(&value).unwrap_or_default());
    };
    let fault = Fault::read(lane);
    if matches!(fault, Some(Fault::Load)) {
        hello(
            &mut out,
            serde_json::json!({ "ok": false, "reason": format!("{FAULT_ENV} failed the load") }),
        );
        return 1;
    }
    let mut session = match crate::gpu::Session::open(lane, dir) {
        Ok(session) => session,
        Err(e) => {
            let text = format!("{e:#}");
            let unavailable = text.starts_with("unavailable");
            hello(
                &mut out,
                serde_json::json!({
                    "ok": false,
                    "unavailable": unavailable,
                    "reason": text.trim_start_matches("unavailable — "),
                }),
            );
            return 1;
        }
    };
    let check = known_answer(lane, &session.device(), session.variant(), |texts| {
        session.embed(texts)
    });
    match check {
        Ok(check) if check.passed => hello(
            &mut out,
            serde_json::json!({
                "ok": true,
                "device": check.device,
                "variant": check.variant,
                "cosine": check.cosine,
                "chunks_per_s": check.chunks_per_s,
            }),
        ),
        Ok(check) => {
            hello(
                &mut out,
                serde_json::json!({
                    "ok": false,
                    "reason": format!(
                        "{} failed the known-answer check: cosine {:.4} against the fp32 fixture",
                        check.device, check.cosine
                    ),
                }),
            );
            return 1;
        }
        Err(e) => {
            hello(
                &mut out,
                serde_json::json!({ "ok": false, "reason": format!("the known-answer check failed: {e:#}") }),
            );
            return 1;
        }
    }

    let mut batches = 0u64;
    loop {
        let Ok(frame) = read_frame(&mut input) else {
            return 0;
        };
        batches += 1;
        match fault {
            Some(Fault::Batch(n)) if batches == n => {
                let _ = write_frame(
                    &mut out,
                    &encode_error(&format!("{FAULT_ENV} failed batch {n}")),
                );
                return 1;
            }
            Some(Fault::Hang(ms)) => std::thread::sleep(Duration::from_millis(ms)),
            _ => {}
        }
        let texts: Vec<String> = serde_json::from_slice::<serde_json::Value>(&frame)
            .ok()
            .and_then(|v| {
                v["texts"].as_array().map(|list| {
                    list.iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect()
                })
            })
            .unwrap_or_default();
        let answer = match session.embed(&texts) {
            Ok(vectors) => encode_vectors(&vectors),
            Err(e) => encode_error(&format!("{e:#}")),
        };
        if write_frame(&mut out, &answer).is_err() {
            return 0;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Fault {
    Load,
    Batch(u64),
    Hang(u64),
}

impl Fault {
    fn read(lane: &str) -> Option<Self> {
        let raw = std::env::var(FAULT_ENV).ok()?;
        let mut parts = raw.split(':');
        if parts.next()? != lane {
            return None;
        }
        match (parts.next()?, parts.next()) {
            ("load", _) => Some(Fault::Load),
            ("batch", Some(n)) => n.parse().ok().map(Fault::Batch),
            ("hang", Some(ms)) => ms.parse().ok().map(Fault::Hang),
            _ => None,
        }
    }
}

// ----------------------------------------------------------------- the GPU

/// Whether this machine has a hardware GPU, and what it is called — asked
/// before anything is downloaded, so a machine without one fetches nothing.
fn detect_gpu() -> std::result::Result<String, String> {
    crate::gpu::detect()
}

/// The WebGPU plugin for this platform, from Microsoft's own wheel on PyPI,
/// pinned by URL and digest. The library is extracted into the model cache
/// beside the fp16 weights.
fn fetch_webgpu(lane: &Lane) -> Result<PathBuf> {
    let cache = crate::model_cache_dir()?;
    crate::gpu::fetch_webgpu(&cache, &mut |percent| {
        lane.set(Status::Downloading { percent });
    })
}

/// Where the worker lanes' components live, for `semlith accel remove`.
pub fn component_dir(cache: &Path, lane: &str) -> PathBuf {
    cache.join("accel").join(lane)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_refuse_a_wrong_length() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"hello").unwrap();
        assert_eq!(read_frame(&mut buffer.as_slice()).unwrap(), b"hello");

        let vectors = vec![vec![0.5f32; DIM], vec![-1.0f32; DIM]];
        let frame = encode_vectors(&vectors);
        assert_eq!(decode_vectors(&frame, 2).unwrap(), vectors);
        assert!(
            decode_vectors(&frame, 3).is_err(),
            "a short answer is refused"
        );
        let error = encode_error("the driver went away");
        assert_eq!(
            decode_vectors(&error, 1).unwrap_err().to_string(),
            "the driver went away"
        );
    }

    #[test]
    fn the_fixture_is_32_normalised_vectors() {
        let (texts, vectors) = fixture();
        assert_eq!(texts.len(), 32);
        assert_eq!(vectors.len(), 32);
        for vector in &vectors {
            assert_eq!(vector.len(), DIM);
            let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "norm {norm}");
        }
    }

    #[test]
    fn faults_are_read_for_their_own_lane_only() {
        // SAFETY: read on this thread by `Fault::read` below and nowhere else
        // in this test binary.
        unsafe { std::env::set_var(FAULT_ENV, "worker:batch:3") };
        assert_eq!(Fault::read("worker"), Some(Fault::Batch(3)));
        assert_eq!(Fault::read("gpu"), None);
        unsafe { std::env::set_var(FAULT_ENV, "gpu:hang:60000") };
        assert_eq!(Fault::read("gpu"), Some(Fault::Hang(60000)));
        unsafe { std::env::remove_var(FAULT_ENV) };
    }
}
