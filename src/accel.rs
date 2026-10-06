//! Every device sharing every index run.
//!
//! An index run hands its chunks to *lanes* in token-budget batches (see
//! [`crate::pipeline`]). The CPU lane is the run's own session on a thread of
//! its own. Every other lane is a worker process — this same binary started as
//! `semlith __embed-worker <lane>` — speaking length-framed batches of token
//! ids over its stdin and stdout, shared by every run in the process.
//!
//! The worker process is the isolation boundary, and the reason there is one.
//! A driver that crashes or aborts kills the worker, never the daemon; the
//! batch it held goes back to the run for another lane, the lane is marked
//! failed with its reason, and the run completes with the same files and
//! chunks. Its device memory goes with it when it exits after an idle minute.
//! And a worker can load a runtime of its own: the CUDA pack's ONNX Runtime,
//! Core ML, NVIDIA's and Intel's plugin providers, llama.cpp's server.
//!
//! Lanes:
//! - `ane` — the Neural Engine on Apple silicon, through Core ML and the
//!   `coreml` pack. On by default once the pack is installed; while it runs no
//!   CPU lane runs beside it, because two int8 threads cut it from 236 to 79
//!   chunks/s on the M1.
//! - `gpu` — the GPU: Core ML's GPU on a Mac with the `coreml` pack (73.2
//!   chunks/s on the M1 against WebGPU's 43.7), WebGPU through Microsoft's
//!   plugin elsewhere. Off beside the Neural Engine unless `gpu-beside-ane`
//!   is on: it adds 30 % in bursts and 4 % sustained on a fanless Air.
//! - `cuda`, `trt`, `openvino`, `llama` — NVIDIA's CUDA and TensorRT for RTX,
//!   Intel's OpenVINO, and llama.cpp on Metal or Vulkan. Experimental: off
//!   until somebody turns one on, built and checked without the hardware, and
//!   labelled so everywhere a lane is shown. No throughput is claimed for them.
//! - `worker` — a CPU-backed worker, never on by default, which is how the
//!   whole worker path is checked on machines that have no accelerator.
//!
//! A lane starts in the background the first time a run could use it and
//! takes batches only once its worker has said hello; until then the run goes
//! on without it. Before that hello the worker embeds 32 fixed chunks and
//! compares them with CPU fp32 vectors committed to the repository. A lane
//! whose cosine falls below [`MIN_COSINE_FP16`] is refused rather than used:
//! a device that produces wrong vectors fails silently otherwise, because the
//! run succeeds and search simply gets worse.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

/// How long one batch may take on a lane before the lane is failed. A hung
/// driver is a failed lane, not a run that never ends.
const BATCH_DEADLINE: Duration = Duration::from_secs(30);

/// Undocumented override for [`BATCH_DEADLINE`], in milliseconds, so a test
/// can drive the hang path without waiting half a minute.
const DEADLINE_ENV: &str = "SEMLITH_ACCEL_DEADLINE_MS";

/// How long a worker may take to say hello. The Neural Engine's first load on
/// a Mac compiles every model for this machine: 163 s for all six on the M1.
const HELLO_DEADLINE: Duration = Duration::from_secs(20 * 60);

/// The time left shown for a Neural Engine compile this machine has never
/// timed: the longest bucket took 35 to 43 s on the M1.
const FIRST_COMPILE: Duration = Duration::from_secs(40);

/// The longest a lane may take to start however alive it says it is: a
/// compile that never finishes fails the lane, and the CPU takes the run.
const START_CAP: Duration = Duration::from_secs(60 * 60);

/// How long a worker is kept with nothing to do. Its device memory goes with
/// it.
const WORKER_IDLE: Duration = Duration::from_secs(60);

/// Unix seconds until which an idle worker is kept anyway: a run between two
/// of its slices. Its last slice drains and writes the index for about a
/// minute with nothing for the lanes, and on the owner's walk the Neural
/// Engine's worker went in that minute, so every slice after waited two
/// minutes for it to load again with the card saying nothing.
static WARM_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Keep idle workers for `for_` more: a run has more slices to come.
pub fn keep_warm(for_: Duration) {
    let until = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        + for_.as_secs();
    WARM_UNTIL.fetch_max(until, std::sync::atomic::Ordering::Relaxed);
}

fn kept_warm() -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    WARM_UNTIL.load(std::sync::atomic::Ordering::Relaxed) > now
}

/// Batches a worker holds at once: one it is computing and the next already
/// in its pipe, so it never waits on the round trip for its next batch.
const WORKER_DEPTH: usize = 2;

/// The lowest cosine against the fp32 fixture an fp16 lane may show, on every
/// one of the 32 chunks.
pub const MIN_COSINE_FP16: f32 = 0.999;

/// The same floor for the int8 CPU-backed worker, whose quantisation costs
/// about 0.013 on its own (measured 0.987 on the 2026-09-23 corpus).
const MIN_COSINE_INT8: f32 = 0.97;

/// Which lanes are on: a comma list such as `cpu,gpu,ane`. Takes precedence
/// over the saved setting, like the other limit variables.
pub const ACCEL_ENV: &str = "SEMLITH_ACCEL";

/// Fault injection for the worker, read only by the worker:
/// `<lane>:load`, `<lane>:batch:N` (fail the Nth batch), `<lane>:hang:MS`, or
/// `<lane>:poison:zero|nan|inf` (spoil the first vector of every batch).
pub const FAULT_ENV: &str = "SEMLITH_FAULT_ACCEL";

/// The known-answer fixture: 32 chunks and their CPU fp32 vectors.
const FIXTURE_TEXTS: &str = include_str!("../tests/fixtures/gpu/known-answer.json");
const FIXTURE_VECTORS: &[u8] = include_bytes!("../tests/fixtures/gpu/known-answer.f32");

const DIM: usize = 384;

// ------------------------------------------------------------------ settings

/// The switches, as `settings.json` holds them. Absent means the default.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Switches {
    pub cpu: Option<bool>,
    pub gpu: Option<bool>,
    pub cuda: Option<bool>,
    pub ane: Option<bool>,
    pub trt: Option<bool>,
    pub openvino: Option<bool>,
    pub llama: Option<bool>,
    /// The GPU lane beside the Neural Engine, which is off by default.
    pub gpu_beside_ane: Option<bool>,
}

/// Which lanes are on, and where that came from.
#[derive(Debug, Clone, Serialize)]
pub struct Enabled {
    pub cpu: bool,
    pub gpu: bool,
    pub cuda: bool,
    pub ane: bool,
    pub trt: bool,
    pub openvino: bool,
    pub llama: bool,
    pub gpu_beside_ane: bool,
    /// The CPU-backed worker, for verification. Only the environment turns
    /// it on.
    pub worker: bool,
    pub source: &'static str,
}

impl Enabled {
    /// The lanes switched on, as `stats` and `semlith_stats` name them:
    /// experimental ones said so.
    pub fn named(&self) -> Vec<String> {
        let mut lanes = Vec::new();
        if self.cpu {
            lanes.push("cpu".to_string());
        }
        for spec in SPECS {
            if spec.id != "worker" && self.lane(spec.id) {
                lanes.push(if spec.experimental {
                    format!("{} (experimental)", spec.id)
                } else {
                    spec.id.to_string()
                });
            }
        }
        lanes
    }

    pub fn lane(&self, id: &str) -> bool {
        match id {
            "cpu" => self.cpu,
            "gpu" => self.gpu,
            "cuda" => self.cuda,
            "ane" => self.ane,
            "trt" => self.trt,
            "openvino" => self.openvino,
            "llama" => self.llama,
            "worker" => self.worker,
            _ => false,
        }
    }
}

/// The switches in force: the environment, then the saved setting, then the
/// defaults. CPU, GPU and the Neural Engine are on; CUDA, TensorRT for RTX,
/// OpenVINO and llama.cpp are experimental and off, and each is fetched only
/// when somebody turns it on.
pub fn enabled() -> Enabled {
    if let Ok(list) = std::env::var(ACCEL_ENV) {
        let has = |name: &str| list.split(',').any(|item| item.trim() == name);
        return Enabled {
            cpu: has("cpu"),
            gpu: has("gpu"),
            cuda: has("cuda"),
            ane: has("ane"),
            trt: has("trt"),
            openvino: has("openvino"),
            llama: has("llama"),
            gpu_beside_ane: has("gpu-beside-ane"),
            worker: has("worker"),
            source: "set by the environment",
        };
    }
    let saved = crate::home::Settings::load().accelerators;
    Enabled {
        cpu: saved.cpu.unwrap_or(true),
        gpu: saved.gpu.unwrap_or(true),
        cuda: saved.cuda.unwrap_or(false),
        ane: saved.ane.unwrap_or(true),
        trt: saved.trt.unwrap_or(false),
        openvino: saved.openvino.unwrap_or(false),
        llama: saved.llama.unwrap_or(false),
        gpu_beside_ane: saved.gpu_beside_ane.unwrap_or(false),
        worker: false,
        source: if saved == Switches::default() {
            "default"
        } else {
            "saved"
        },
    }
}

// --------------------------------------------------------------------- lanes

/// What each lane is, in the order they are shown.
pub struct Spec {
    pub id: &'static str,
    pub label: &'static str,
    /// The vector variant it makes, as a store's `variants` row counts it.
    pub variant: &'static str,
    /// Built and checked without its hardware; labelled wherever it is shown.
    pub experimental: bool,
}

pub const SPECS: &[Spec] = &[
    Spec {
        id: "ane",
        label: "Neural Engine",
        variant: "fp16-ane",
        experimental: false,
    },
    Spec {
        id: "gpu",
        label: "GPU",
        variant: "fp16-webgpu",
        experimental: false,
    },
    Spec {
        id: "cuda",
        label: "CUDA",
        variant: "fp16-cuda",
        experimental: true,
    },
    Spec {
        id: "trt",
        label: "TensorRT for RTX",
        variant: "fp16-trt",
        experimental: true,
    },
    Spec {
        id: "openvino",
        label: "OpenVINO",
        variant: "openvino",
        experimental: true,
    },
    Spec {
        id: "llama",
        label: "llama.cpp",
        variant: "gguf-f16",
        experimental: true,
    },
    Spec {
        id: "worker",
        label: "CPU worker",
        variant: "int8-cpu",
        experimental: false,
    },
];

pub fn spec(id: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.id == id)
}

/// Where a lane stands, as the Machine limits card shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Status {
    /// Enabled and not yet asked for anything, or its worker exited idle.
    Idle,
    Starting,
    /// Loading its models for the first time on this machine, which on the
    /// Neural Engine is a compilation of minutes. Runs wait for it.
    Compiling {
        percent: u8,
        /// How long is left, from how far it has got in the time it has
        /// taken; absent until it has got anywhere.
        #[serde(skip_serializing_if = "Option::is_none")]
        eta_ms: Option<u64>,
    },
    Active,
    Downloading {
        percent: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        eta_ms: Option<u64>,
    },
    Unavailable {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

enum Job {
    /// Start the worker, if there is none, and nothing else.
    Warm,
    Batch {
        batch: Vec<crate::session::Ids>,
        reply: mpsc::Sender<std::result::Result<Vec<Vec<f32>>, String>>,
    },
}

/// One accelerator lane, shared by every run in the process.
pub struct Lane {
    pub id: &'static str,
    pub experimental: bool,
    status: Mutex<Status>,
    device: Mutex<Option<String>>,
    /// Set from the worker's hello: the GPU lane is Core ML on a Mac with the
    /// pack and WebGPU elsewhere, and says which.
    variant: Mutex<&'static str>,
    /// Chunks this lane has embedded, and when, for its live share.
    samples: Mutex<std::collections::VecDeque<(Instant, u64)>>,
    chunks: AtomicU64,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    /// When the compile or download under way began, when its percentage
    /// last moved, and to what: its time left counts down from these.
    began: Mutex<Option<(Instant, Instant, u8)>>,
    /// How long this lane's last compile took on this machine, for a time
    /// left before the compile has moved: the Neural Engine's first bucket
    /// is a single step of half a minute.
    expected: Mutex<Option<Duration>>,
    /// When the lane last went from working to failed: a failure is tried
    /// again after [`LANE_RETRY`] rather than kept for the life of the daemon.
    failed_at: Mutex<Option<Instant>>,
}

/// How long a failed lane waits before it is tried again. A Neural Engine
/// worker that failed once at a daemon's start left that daemon embedding on
/// the CPU, ten times slower, for the rest of its life, and switching the lane
/// off and on from the terminal never reached it.
/// ponytail: one fixed interval; back off if a lane that keeps failing costs
/// more than the retries are worth.
const LANE_RETRY: Duration = Duration::from_secs(600);

impl Lane {
    fn new(spec: &Spec) -> Self {
        Self {
            id: spec.id,
            experimental: spec.experimental,
            status: Mutex::new(Status::Idle),
            device: Mutex::new(None),
            variant: Mutex::new(spec.variant),
            samples: Mutex::new(std::collections::VecDeque::new()),
            chunks: AtomicU64::new(0),
            jobs: Mutex::new(None),
            began: Mutex::new(None),
            expected: Mutex::new(None),
            failed_at: Mutex::new(None),
        }
    }

    /// Padded tokens per batch this lane takes: its floor, its first guess
    /// before it has a measured pace, and its ceiling. The run sizes each
    /// batch from the lane's own pace inside these.
    pub fn token_budget(&self) -> (usize, usize, usize) {
        match self.id {
            // A discrete card is starved by less.
            "cuda" | "trt" => (2_048, 16_384, 65_536),
            // Four rows a call, so several calls a batch keep it busy.
            "ane" => (1_024, 4_096, 12_288),
            _ => (512, 4_096, 16_384),
        }
    }

    /// Rows the lane's model computes a call, whatever it is given: the Core
    /// ML packs are converted at a fixed batch (the manifest's `batch`), and a
    /// call with fewer rows computes the rest as padding.
    pub fn rows_per_call(&self) -> usize {
        match self.variant() {
            "fp16-ane" => 4,
            "fp16-coreml-gpu" => 8,
            _ => 1,
        }
    }

    pub fn variant(&self) -> &'static str {
        *self.variant.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> Status {
        let status = self
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        // The time left, read at the moment it is asked for: how long the
        // phase has taken so far, scaled by how far it has got.
        // A countdown rather than a fresh guess each read: the pace up to the
        // last step, applied to what is left, less the time since that step.
        // A compile moves in steps (a sixth at a time on the Neural Engine),
        // and a guess from the total time so far climbed between steps.
        let began = *self.began.lock().unwrap_or_else(|e| e.into_inner());
        let expected = *self.expected.lock().unwrap_or_else(|e| e.into_inner());
        let compiling = matches!(status, Status::Compiling { .. });
        let left = || {
            let (start, step, at) = began?;
            if at == 0 && compiling {
                let whole = expected?.as_millis() as u64;
                return Some(
                    whole
                        .saturating_sub(start.elapsed().as_millis() as u64)
                        .max(1_000),
                );
            }
            if at == 0 || at >= 100 {
                return None;
            }
            let per = step.duration_since(start).as_millis() as u64 / u64::from(at);
            let whole = per * u64::from(100 - at);
            Some(
                whole
                    .saturating_sub(step.elapsed().as_millis() as u64)
                    .max(1_000),
            )
        };
        match status {
            Status::Compiling { percent, .. } => Status::Compiling {
                percent,
                eta_ms: left(),
            },
            Status::Downloading { percent, .. } => Status::Downloading {
                percent,
                eta_ms: left(),
            },
            other => other,
        }
    }

    fn set(&self, status: Status) {
        let mut current = self.status.lock().unwrap_or_else(|e| e.into_inner());
        let phase = |s: &Status| match s {
            Status::Compiling { .. } => 1,
            Status::Downloading { .. } => 2,
            // Timed too, so a run waiting on a start says for how long.
            Status::Starting => 3,
            _ => 0,
        };
        let (was, now) = (phase(&current), phase(&status));
        let percent = match &status {
            Status::Compiling { percent, .. } | Status::Downloading { percent, .. } => *percent,
            _ => 0,
        };
        let mut began = self.began.lock().unwrap_or_else(|e| e.into_inner());
        if now != was {
            *began = (now != 0).then(|| (Instant::now(), Instant::now(), percent));
        } else if let Some((start, _, at)) = *began
            && percent != at
        {
            *began = Some((start, Instant::now(), percent));
        }
        drop(began);
        let mut failed_at = self.failed_at.lock().unwrap_or_else(|e| e.into_inner());
        match &status {
            Status::Failed { reason } if !matches!(*current, Status::Failed { .. }) => {
                *failed_at = Some(Instant::now());
                // The daemon's log is its stderr: without this line a lane that
                // failed was visible only to someone who ran `semlith accel`.
                eprintln!(
                    "semlith: the {} lane failed: {reason}; it is tried again in {} minutes",
                    self.id,
                    LANE_RETRY.as_secs() / 60
                );
            }
            Status::Failed { .. } => {}
            _ => *failed_at = None,
        }
        drop(failed_at);
        *current = status;
    }

    /// Whether the lane is on its way to taking batches: asked for, starting,
    /// downloading its pack or compiling its models. A run waits for such a
    /// lane rather than handing its work to the CPU.
    pub fn coming(&self) -> bool {
        matches!(
            self.status(),
            Status::Idle | Status::Starting | Status::Compiling { .. } | Status::Downloading { .. }
        )
    }

    /// Whether the lane can be used at all: not failed, not unavailable. A
    /// failure older than [`LANE_RETRY`] is cleared here, so the next run
    /// tries the lane afresh.
    pub fn usable(&self) -> bool {
        let due = self
            .failed_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|at| at.elapsed() >= LANE_RETRY);
        if due {
            self.reset();
        }
        !matches!(
            self.status(),
            Status::Unavailable { .. } | Status::Failed { .. }
        )
    }

    /// Whether a run may hand this lane a batch now: its worker said hello.
    pub fn ready(&self) -> bool {
        self.status() == Status::Active
    }

    /// Start the lane's worker in the background if it has none. A run that
    /// has no ready lane and no CPU waits for it (see [`for_run`]).
    pub fn wake(self: &Arc<Self>) {
        {
            let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
            if *status != Status::Idle {
                return;
            }
            *status = Status::Starting;
        }
        self.send(Job::Warm);
    }

    /// Clear a failure, so the lane is tried afresh. A switch turned off and
    /// on is how a user asks for that.
    pub fn reset(&self) {
        if matches!(
            self.status(),
            Status::Failed { .. } | Status::Unavailable { .. }
        ) {
            self.set(Status::Idle);
        }
    }

    /// Hand one batch of token ids to the lane. The answer arrives on the
    /// returned receiver: vectors in the order of `batch`, or why the lane
    /// could not.
    pub fn submit(
        self: &Arc<Self>,
        batch: Vec<crate::session::Ids>,
    ) -> mpsc::Receiver<std::result::Result<Vec<Vec<f32>>, String>> {
        let (reply, answer) = mpsc::channel();
        self.send(Job::Batch { batch, reply });
        answer
    }

    fn send(self: &Arc<Self>, job: Job) {
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
        let refused = match jobs.as_ref() {
            Some(tx) => match tx.send(job) {
                Ok(()) => None,
                Err(mpsc::SendError(job)) => {
                    *jobs = None;
                    Some((job, "the lane's dispatcher has gone"))
                }
            },
            None => Some((job, "the lane's dispatcher could not start")),
        };
        if let Some((Job::Batch { reply, .. }, why)) = refused {
            let _ = reply.send(Err(why.into()));
        }
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

    /// Chunks per second over the last [`RATE_WINDOW`], and 0 once the lane
    /// has not finished a batch for [`RATE_IDLE`]: a paused or stopped run,
    /// or a lane switched off, reads 0 at once. The samples are pruned only
    /// when the lane counts, so a lane that had stopped used to keep the rate
    /// it had, fading over ten seconds, and its share of the machine with it.
    pub fn rate(&self) -> f64 {
        let samples = self.samples.lock().unwrap_or_else(|e| e.into_inner());
        let Some(&(last, to)) = samples.back() else {
            return 0.0;
        };
        if last.elapsed() > RATE_IDLE {
            return 0.0;
        }
        // From the newest sample at least a window old, so the window holds
        // whole batches rather than starting at the first one inside it.
        let Some(&(since, from)) = samples
            .iter()
            .rev()
            .find(|(at, _)| at.elapsed() >= RATE_WINDOW)
            .or(samples.front())
        else {
            return 0.0;
        };
        let span = since.elapsed().as_secs_f64();
        if span < 0.5 {
            return 0.0;
        }
        (to - from) as f64 / span
    }
}

/// How far back a lane's rate looks.
const RATE_WINDOW: Duration = Duration::from_secs(3);

/// How long without a finished batch before a lane's rate reads 0: several
/// of any lane's batches, which are sized to take a fifth of a second.
const RATE_IDLE: Duration = Duration::from_millis(1500);

/// The in-process CPU lane's own count, for the card's share.
static CPU: OnceLock<Arc<Lane>> = OnceLock::new();

fn cpu_lane() -> &'static Arc<Lane> {
    CPU.get_or_init(|| {
        Arc::new(Lane::new(&Spec {
            id: "cpu",
            label: "CPU",
            variant: "int8-cpu",
            experimental: false,
        }))
    })
}

/// Count chunks the in-process CPU lane embedded.
pub fn count_cpu(chunks: usize) {
    cpu_lane().count(chunks);
}

/// The worker lanes. Created on first use.
static LANES: OnceLock<Vec<Arc<Lane>>> = OnceLock::new();

/// Whether this process hands batches to lanes at all. The daemon does, and
/// from 0.32.0 so do `semlith index` and `semlith watch` in a terminal. The
/// library alone does not: the retrieval harness and the tests stay on the
/// CPU, which is what keeps their figures reproducible (issue #88).
static MANAGED: AtomicBool = AtomicBool::new(false);

pub fn manage() {
    MANAGED.store(true, Ordering::Relaxed);
}

/// Whether this process uses the lanes: the daemon and the command line.
pub fn managed() -> bool {
    MANAGED.load(Ordering::Relaxed)
}

fn lanes() -> &'static Vec<Arc<Lane>> {
    LANES.get_or_init(|| SPECS.iter().map(|spec| Arc::new(Lane::new(spec))).collect())
}

pub fn lane(id: &str) -> Option<&'static Arc<Lane>> {
    lanes().iter().find(|lane| lane.id == id)
}

/// Why a lane cannot run on this machine at all, before anything is fetched.
pub fn unavailable_here(id: &str) -> Option<String> {
    let apple = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    let x86_windows_or_linux = cfg!(all(
        target_arch = "x86_64",
        any(windows, target_os = "linux")
    ));
    match id {
        "ane" if !apple => Some("the Neural Engine lane needs a Mac with Apple silicon".into()),
        "cuda" => crate::cuda::unavailable_here(),
        "trt" | "openvino" if !x86_windows_or_linux => Some(format!(
            "the {} lane is built for Windows and Linux on x86_64",
            spec(id).map_or(id, |s| s.label)
        )),
        "llama" if !(apple || x86_windows_or_linux) => {
            Some("no llama.cpp build is pinned for this platform".into())
        }
        _ => None,
    }
}

/// The worker lanes a run may use now, and whether its own CPU lane may take
/// batches as well. A lane in the list may still be starting: the run wakes
/// it and uses it once it is ready.
///
/// The lane policy lives here. On Apple silicon the Neural Engine goes first
/// and, once it is running, nothing else runs beside it: no CPU embedding
/// lane (it takes the cores that feed the Neural Engine), and no GPU lane
/// unless `gpu-beside-ane` is on. Elsewhere every accelerator that is on runs,
/// with the CPU beside them — measured on the M1 before the Neural Engine
/// existed, CPU beside WebGPU was 1.52x WebGPU alone. With no accelerator
/// ready the CPU carries the run whatever its switch says only when none is on
/// its way: a lane starting, downloading or compiling is waited for.
pub fn for_run() -> (Vec<Arc<Lane>>, bool) {
    if !MANAGED.load(Ordering::Relaxed) {
        return (Vec::new(), true);
    }
    let on = enabled();
    let mut chosen: Vec<Arc<Lane>> = lanes()
        .iter()
        .filter(|lane| on.lane(lane.id) && unavailable_here(lane.id).is_none())
        .filter(|lane| lane.usable())
        .cloned()
        .collect();
    let mut cpu = on.cpu;
    let ane_running = chosen.iter().any(|lane| lane.id == "ane" && lane.ready());
    if ane_running {
        cpu = false;
        if !on.gpu_beside_ane {
            chosen.retain(|lane| lane.id != "gpu");
        }
    }
    // With no lane ready the CPU carries the run only when no lane is on its
    // way either: a lane still downloading, starting or compiling is waited
    // for, not raced — its first minutes on the CPU were slower than the wait
    // and took the cores the lane's own start needed. A lane that fails is
    // not on its way, and the CPU takes over then.
    let any_ready = chosen.iter().any(|lane| lane.ready());
    let any_coming = chosen.iter().any(|lane| lane.coming());
    (chosen, cpu || (!any_ready && !any_coming))
}

/// A time left as a person says it: seconds under a minute, minutes after.
pub fn spell_left(ms: u64) -> String {
    let secs = ms.div_ceil(1000);
    if secs < 60 {
        format!("about {secs} s left")
    } else {
        format!("about {} min left", secs.div_ceil(60))
    }
}

/// What a run is waiting for, when it waits: no lane it may use is ready, one
/// is on its way, and the CPU is not switched on beside them. The run card and
/// the terminal say this instead of a rate.
pub fn waiting_for() -> Option<String> {
    if !MANAGED.load(Ordering::Relaxed) {
        return None;
    }
    let (lanes, cpu) = for_run();
    if cpu || lanes.iter().any(|lane| lane.ready()) {
        return None;
    }
    let lane = lanes.iter().find(|lane| lane.coming())?;
    let label = spec(lane.id).map_or(lane.id, |s| s.label);
    let left = |eta: Option<u64>| {
        eta.map(|ms| format!(", {}", spell_left(ms)))
            .unwrap_or_default()
    };
    Some(match lane.status() {
        Status::Compiling { percent, eta_ms } => format!(
            "waiting for the {label} lane to compile its models: {percent} %{}",
            left(eta_ms)
        ),
        Status::Downloading { percent, eta_ms } => format!(
            "waiting for the {label} lane to download: {percent} %{}",
            left(eta_ms)
        ),
        // Seconds so far, so a long first start after an update (Core ML
        // compiling the model again, two minutes on replay 11) still moves.
        _ => match *lane.began.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((start, _, _)) if start.elapsed() >= Duration::from_secs(5) => format!(
                "waiting for the {label} lane to start: {} s so far",
                start.elapsed().as_secs()
            ),
            _ => format!("waiting for the {label} lane to start"),
        },
    })
}

/// The variants a cached vector may be for this process's runs, best first:
/// what its lanes make, then the CPU's. A cached vector of any of them is as
/// good as embedding it again here, which is the point of the cache.
pub fn cache_variants() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    if MANAGED.load(Ordering::Relaxed) {
        let on = enabled();
        for spec in SPECS {
            if spec.id == "worker" || !on.lane(spec.id) || unavailable_here(spec.id).is_some() {
                continue;
            }
            // The GPU lane is Core ML on a Mac and WebGPU elsewhere.
            let variants: &[&'static str] = match spec.id {
                "gpu" => &["fp16-coreml-gpu", "fp16-webgpu"],
                _ => std::slice::from_ref(&spec.variant),
            };
            for variant in variants {
                if !out.contains(variant) {
                    out.push(variant);
                }
            }
        }
    }
    out.push("int8-cpu");
    out
}

/// Whether this process's index runs use the machine-wide vector cache:
/// where they use the lanes — the daemon and the command line — or where the
/// environment names a cap. The library alone does not, so the retrieval
/// harness never reads a vector another run left behind (#88).
pub fn cache_in_use() -> bool {
    (MANAGED.load(Ordering::Relaxed) || std::env::var(crate::cache::CAP_ENV).is_ok())
        && crate::cache::cap_bytes() > 0
}

/// Every lane as the Machine limits card and `semlith accel status` show it.
pub fn snapshot() -> serde_json::Value {
    let on = enabled();
    let cpu = cpu_lane();
    let cache = crate::model_cache_dir().ok();
    let mut rows = vec![serde_json::json!({
        "lane": "cpu",
        "label": "CPU",
        "enabled": on.cpu,
        "experimental": false,
        "status": Status::Active,
        "device": crate::system::cpu_name(),
        "variant": cpu.variant(),
        "rate": round(cpu.rate()),
    })];
    for lane in lanes() {
        let enabled = on.lane(lane.id);
        if lane.id == "worker" && !enabled {
            continue;
        }
        let status = match unavailable_here(lane.id) {
            Some(reason) => Status::Unavailable { reason },
            None => lane.status(),
        };
        let installed = cache
            .as_deref()
            .and_then(|cache| pack_for(lane.id).map(|pack| crate::packs::installed(cache, &pack)));
        rows.push(serde_json::json!({
            "lane": lane.id,
            "label": spec(lane.id).map_or(lane.id, |s| s.label),
            "enabled": enabled,
            "experimental": lane.experimental,
            "status": status,
            "device": lane.device.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            "variant": lane.variant(),
            "rate": round(lane.rate()),
            "installed": installed.map(|dir| dir.is_some()),
            "download_bytes": pack_for(lane.id).map(|pack| pack.bytes()),
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
    let accel_ready = lanes()
        .iter()
        .any(|lane| lane.id != "worker" && on.lane(lane.id) && lane.ready());
    let accel_coming = lanes().iter().any(|lane| {
        lane.id != "worker"
            && on.lane(lane.id)
            && unavailable_here(lane.id).is_none()
            && lane.coming()
    });
    serde_json::json!({
        "lanes": rows,
        "source": on.source,
        "gpu_beside_ane": on.gpu_beside_ane,
        // Said, not implied: the CPU carries the run whatever its switch says
        // while no worker lane can and none is on its way.
        "cpu_fallback": !on.cpu && !accel_ready && !accel_coming,
        // What each lane has managed here, measured or from its check: what
        // a page estimates a run from before it starts.
        "rates": rates(),
    })
}

fn round(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

// ----------------------------------------------------------------- lane rates

/// The CPU lane's known-answer check, its speed remembered.
fn check_cpu() -> Result<Check> {
    let cache = crate::model_cache_dir()?;
    let mut model = crate::embed::Model::Granite.load(cache, crate::chunk::MAX_CHARS / 2, true)?;
    let check = known_answer("cpu", &crate::system::cpu_name(), "int8-cpu", |texts| {
        let mut got = model
            .embed(texts, Some(1))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for vector in &mut got {
            crate::normalize(vector);
        }
        Ok(got)
    })?;
    if check.passed {
        // In batches, as a run embeds: see the worker's figure.
        let (texts, _) = fixture();
        let started = Instant::now();
        model
            .embed(&texts, Some(8))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        note_known_answer(
            "cpu",
            texts.len() as f64 / started.elapsed().as_secs_f64().max(1e-6),
        );
    }
    Ok(check)
}

/// Give the CPU lane a figure if it has none, so the first estimate on a
/// fresh machine says something rather than nothing: thirty-two chunks, a
/// second or two, once.
pub fn seed_cpu_rate() {
    // Never a download for it: only a model already on this machine.
    let cached = crate::model_cache_dir().is_ok_and(|cache| crate::embed::is_cached(&cache));
    if cached && !rates().contains_key("cpu") {
        let _ = check_cpu();
    }
}

/// What one lane has managed on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, serde::Deserialize)]
pub struct Rate {
    /// Chunks per second, or images per second for `clip`.
    pub per_s: f64,
    /// `measured` from runs, or `known-answer` from the lane's check, which
    /// embeds one chunk at a time and so undersells a batched lane.
    pub source: RateSource,
    /// Runs the measured figure is an average of.
    #[serde(default)]
    pub runs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RateSource {
    Measured,
    KnownAnswer,
}

/// Every lane's remembered rate, by lane id (`clip` for images).
pub fn rates() -> std::collections::BTreeMap<String, Rate> {
    crate::home::lane_rates_path()
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_rates(rates: &std::collections::BTreeMap<String, Rate>) {
    if let (Ok(path), Ok(bytes)) = (
        crate::home::lane_rates_path(),
        serde_json::to_vec_pretty(rates),
    ) {
        let _ = crate::home::write_private(&path, &bytes);
    }
}

/// A run measured `per_s` on `lane`: folded into the remembered figure, half
/// old and half new, so one odd run moves it but does not own it.
pub fn note_rate(lane: &str, per_s: f64) {
    if !per_s.is_finite() || per_s <= 0.0 {
        return;
    }
    let mut all = rates();
    let next = match all.get(lane) {
        Some(old) if old.source == RateSource::Measured => Rate {
            per_s: (old.per_s + per_s) / 2.0,
            source: RateSource::Measured,
            runs: old.runs.saturating_add(1),
        },
        _ => Rate {
            per_s,
            source: RateSource::Measured,
            runs: 1,
        },
    };
    all.insert(lane.to_string(), next);
    save_rates(&all);
}

/// A lane's speed on the known-answer fixture in batches, kept only until a
/// run measures the lane.
pub fn note_known_answer(lane: &str, per_s: f64) {
    if !per_s.is_finite() || per_s <= 0.0 {
        return;
    }
    let mut all = rates();
    if all
        .get(lane)
        .is_some_and(|r| r.source == RateSource::Measured)
    {
        return;
    }
    all.insert(
        lane.to_string(),
        Rate {
            per_s,
            source: RateSource::KnownAnswer,
            runs: 0,
        },
    );
    save_rates(&all);
}

/// Chunks per second the lanes switched on here are expected to manage
/// together, or `None` when none of them has a figure yet. The prior a run's
/// time left starts from.
pub fn expected_rate() -> Option<f64> {
    let known = rates();
    let sum: f64 = run_lanes()
        .iter()
        .filter_map(|lane| known.get(*lane).map(|r| r.per_s))
        .sum();
    (sum > 0.0).then_some(sum)
}

/// Whether a run's text embeds off the CPU, so the image model, which runs on
/// the CPU on a thread of its own, works beside it rather than taking turns.
pub fn images_beside_text() -> bool {
    run_lanes().iter().any(|lane| *lane != "cpu")
}

/// The lanes a run would embed text on, as `for_run` chooses them, among
/// those with a known rate or usable here.
fn run_lanes() -> Vec<&'static str> {
    let on = enabled();
    let known = rates();
    let usable = |lane: &&str| on.lane(lane) && unavailable_here(lane).is_none();
    // The lanes a run would use, as `for_run` chooses them: with the Neural
    // Engine on, the CPU steps aside and the GPU joins only when asked to.
    let ane = usable(&"ane") && known.contains_key("ane");
    let lanes: Vec<&str> = ["cpu", "ane", "gpu", "cuda", "trt", "openvino", "llama"]
        .into_iter()
        .filter(|lane| usable(lane))
        .filter(|lane| !ane || (*lane != "cpu" && (*lane != "gpu" || on.gpu_beside_ane)))
        .collect();
    lanes
}

/// The names `semlith accel on|off` takes.
pub const SWITCH_NAMES: &str = "cpu, gpu, ane, cuda, trt, openvino, llama, gpu-beside-ane";

/// Turn a lane on or off, as the page's switch and `semlith accel` do.
///
/// Saved to `settings.json` and read by every run before every batch, which
/// is the "next batch" the switch promises. Turning a failed lane on again
/// clears its failure so it is tried afresh. The CPU may be turned off only
/// while an accelerator lane can carry the work; with none, the refusal says
/// why. Turning on a lane whose pack is not here fetches it first, with
/// `progress` told how far the download has got.
pub fn set(lane_id: &str, on: bool) -> Result<String> {
    set_with_progress(lane_id, on, &mut |_| {})
}

pub fn set_with_progress(lane_id: &str, on: bool, progress: &mut dyn FnMut(u8)) -> Result<String> {
    if std::env::var(ACCEL_ENV).is_ok() {
        bail!("{ACCEL_ENV} is set in this process's environment, so the switches cannot change it");
    }
    let mut settings = crate::home::Settings::load();
    match (lane_id, on) {
        ("cpu", false) => {
            let now = enabled();
            let can = lanes().iter().any(|l| {
                l.id != "worker" && now.lane(l.id) && l.usable() && unavailable_here(l.id).is_none()
            }) && (now.ane || detect_gpu().is_ok());
            if !can {
                bail!(
                    "the CPU cannot be turned off: no accelerator lane is on and usable here ({}), \
                     so the CPU is what indexes",
                    detect_gpu()
                        .err()
                        .unwrap_or_else(|| "the GPU lane is off".to_string())
                );
            }
            settings.accelerators.cpu = Some(false);
        }
        ("cpu", true) => settings.accelerators.cpu = Some(true),
        ("gpu", _) => settings.accelerators.gpu = Some(on),
        ("gpu-beside-ane", _) => settings.accelerators.gpu_beside_ane = Some(on),
        (id @ ("ane" | "cuda" | "trt" | "openvino" | "llama"), _) => {
            if on && let Some(why) = unavailable_here(id) {
                bail!("{why}");
            }
            if on && let Some(pack) = pack_for(id) {
                let cache = crate::model_cache_dir()?;
                crate::packs::fetch(&cache, &pack, progress)
                    .with_context(|| format!("fetching the {} pack", pack.name))?;
            }
            let slot = match id {
                "ane" => &mut settings.accelerators.ane,
                "cuda" => &mut settings.accelerators.cuda,
                "trt" => &mut settings.accelerators.trt,
                "openvino" => &mut settings.accelerators.openvino,
                _ => &mut settings.accelerators.llama,
            };
            *slot = Some(on);
        }
        (other, _) => bail!("there is no lane called {other}; the switches are {SWITCH_NAMES}"),
    }
    settings.save()?;
    if on && let Some(found) = lane(lane_id) {
        found.reset();
    }
    let note = spec(lane_id)
        .filter(|s| s.experimental && on)
        .map(|s| {
            format!(
                " ({} is experimental: built and checked without its hardware, and not measured on it)",
                s.label
            )
        })
        .unwrap_or_default();
    Ok(format!(
        "{lane_id} {} — runs pick it up at their next batch{note}",
        if on { "on" } else { "off" }
    ))
}

/// [`set`] for the page: a lane whose pack is not here is switched on once
/// the pack has arrived, fetched on a thread of its own while its row shows
/// the download. Anything that can be refused at once is refused at once.
pub fn set_in_background(lane_id: &str) -> Result<String> {
    let needs_fetch = pack_for(lane_id).is_some_and(|pack| {
        crate::model_cache_dir()
            .ok()
            .is_some_and(|cache| crate::packs::installed(&cache, &pack).is_none())
    });
    if !needs_fetch {
        return set(lane_id, true);
    }
    if std::env::var(ACCEL_ENV).is_ok() {
        bail!("{ACCEL_ENV} is set in this process's environment, so the switches cannot change it");
    }
    if let Some(why) = unavailable_here(lane_id) {
        bail!("{why}");
    }
    let Some(found) = lane(lane_id) else {
        bail!("there is no lane called {lane_id}; the switches are {SWITCH_NAMES}");
    };
    found.set(Status::Downloading {
        percent: 0,
        eta_ms: None,
    });
    let id = lane_id.to_string();
    let lane = Arc::clone(found);
    std::thread::spawn(move || {
        let fetched = set_with_progress(&id, true, &mut |percent| {
            lane.set(Status::Downloading {
                percent,
                eta_ms: None,
            })
        });
        match fetched {
            Ok(_) => lane.set(Status::Idle),
            Err(e) => lane.set(Status::Failed {
                reason: format!("{e:#}"),
            }),
        }
    });
    Ok(format!(
        "{lane_id}: fetching its components; it turns on when they have arrived"
    ))
}

/// The pack a lane's worker needs beyond the model, if any.
pub fn pack_for(id: &str) -> Option<crate::packs::Pack> {
    match id {
        "ane" => Some(crate::packs::coreml()),
        "trt" => Some(crate::trt::pack()),
        "openvino" => Some(crate::openvino::pack()),
        "llama" => Some(crate::llama::pack()),
        _ => None,
    }
}

/// Delete a lane's downloaded components, and say how many bytes that freed.
/// Turning a lane off never does this; only asking does.
pub fn remove(lane_id: &str) -> Result<u64> {
    let cache = crate::model_cache_dir()?;
    if let Some(pack) = pack_for(lane_id) {
        let worker = if lane_id == "ane" {
            remove_coreml_worker(&cache)
        } else {
            0
        };
        return Ok(crate::packs::remove(&cache, &pack)? + worker);
    }
    let dir = match lane_id {
        "gpu" => component_dir(&cache, &format!("webgpu-{}", crate::gpu::WEBGPU_VERSION)),
        "cuda" => crate::gpu::cuda_dir(&cache),
        other => bail!(
            "{other} has nothing downloaded to remove; the lanes with components are gpu, ane, \
             cuda, trt, openvino and llama"
        ),
    };
    let bytes = dir_bytes(&dir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("removing {}", crate::plain(&dir.display().to_string())))?;
    }
    Ok(bytes)
}

/// The Core ML worker's copy of semlith, which goes with the Neural Engine's
/// models when they are removed.
fn remove_coreml_worker(cache: &Path) -> u64 {
    let dir = component_dir(cache, &format!("coreml-worker-v{COREML_WORKER}"));
    let bytes = dir_bytes(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    bytes
}

/// What each lane's downloaded components take on disk.
pub fn component_bytes() -> serde_json::Value {
    let Ok(cache) = crate::model_cache_dir() else {
        return serde_json::json!({});
    };
    let mut out = serde_json::json!({
        "gpu": dir_bytes(&component_dir(&cache, &format!("webgpu-{}", crate::gpu::WEBGPU_VERSION))),
        "cuda": dir_bytes(&crate::gpu::cuda_dir(&cache)),
        "cuda_download": crate::gpu::cuda_pack_bytes(),
    });
    for id in ["ane", "trt", "openvino", "llama"] {
        if let Some(pack) = pack_for(id) {
            out[id] = serde_json::json!(dir_bytes(&pack.dir(&cache)));
            out[format!("{id}_download")] = serde_json::json!(pack.bytes());
        }
    }
    out
}

pub(crate) fn dir_bytes(dir: &Path) -> u64 {
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
    /// `None` only while the worker is being let go.
    stdin: Option<std::process::ChildStdin>,
    answers: mpsc::Receiver<std::result::Result<Vec<u8>, String>>,
}

impl Drop for Worker {
    /// Closing its stdin is how a worker is told to go, and it goes, taking
    /// whatever it started with it (llama.cpp's server). Killed only if it has
    /// not gone within two seconds.
    fn drop(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One batch sent to the worker and not yet answered.
struct Sent {
    rows: usize,
    reply: mpsc::Sender<std::result::Result<Vec<Vec<f32>>, String>>,
}

fn dispatch(lane: Arc<Lane>, jobs: mpsc::Receiver<Job>) {
    let mut worker: Option<Worker> = None;
    let mut sent: std::collections::VecDeque<Sent> = std::collections::VecDeque::new();
    // What to do when the worker cannot be trusted any more: kill it, fail
    // the lane with the reason, and hand every batch it held back.
    let fail = |lane: &Lane,
                worker: &mut Option<Worker>,
                sent: &mut std::collections::VecDeque<Sent>,
                reason: String| {
        *worker = None;
        lane.set(Status::Failed {
            reason: reason.clone(),
        });
        for batch in sent.drain(..) {
            let _ = batch.reply.send(Err(reason.clone()));
        }
    };
    loop {
        // Take a job: wait for one only when nothing is in flight.
        let job = if sent.is_empty() {
            match jobs.recv_timeout(WORKER_IDLE) {
                Ok(job) => Some(job),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Not while a Core ML worker is still compiling the rest
                    // of its buckets: killing it abandoned the compile half
                    // written, the system's compiler went on with it anyway,
                    // and the next start compiled it again.
                    if worker.is_some() && matches!(lane.id, "ane" | "gpu") && coreml_compiling() {
                        continue;
                    }
                    // Nor between a run's slices.
                    if worker.is_some() && kept_warm() {
                        continue;
                    }
                    // Idle: the worker goes, and its device memory with it.
                    if worker.take().is_some() && lane.status() == Status::Active {
                        lane.set(Status::Idle);
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else if sent.len() < WORKER_DEPTH {
            jobs.try_recv().ok()
        } else {
            None
        };
        if let Some(job) = job {
            let refuse = |job: Job, why: String| {
                if let Job::Batch { reply, .. } = job {
                    let _ = reply.send(Err(why));
                }
            };
            if !lane.usable() {
                refuse(job, reason_of(&lane.status()));
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
                        if let Some(why) = reason.strip_prefix("unavailable — ") {
                            lane.set(Status::Unavailable {
                                reason: why.to_string(),
                            });
                        } else {
                            lane.set(Status::Failed {
                                reason: reason.clone(),
                            });
                        }
                        refuse(job, reason);
                        continue;
                    }
                }
            }
            let Job::Batch { batch, reply } = job else {
                continue;
            };
            let running = worker.as_mut().expect("started above");
            let written = match running.stdin.as_mut() {
                Some(stdin) => write_frame(stdin, &encode_ids(&batch)),
                None => Err(std::io::Error::other("the worker is being let go")),
            };
            if let Err(e) = written {
                let _ = reply.send(Err(format!("the worker stopped reading: {e}")));
                fail(
                    &lane,
                    &mut worker,
                    &mut sent,
                    format!("the worker stopped reading: {e}"),
                );
                continue;
            }
            sent.push_back(Sent {
                rows: batch.len(),
                reply,
            });
            // Another batch may be waiting: send it before reading, so the
            // worker's next batch is already in its pipe.
            if sent.len() < WORKER_DEPTH {
                continue;
            }
        }
        // Read the oldest answer.
        let Some(oldest) = sent.front() else {
            continue;
        };
        let running = worker.as_mut().expect("a batch in flight has a worker");
        let answer = match running.answers.recv_timeout(deadline()) {
            Ok(Ok(frame)) => decode_vectors(&frame, oldest.rows),
            Ok(Err(e)) => Err(anyhow::anyhow!("the worker exited: {e}")),
            Err(_) => Err(anyhow::anyhow!(
                "the worker did not answer a batch within {} s",
                deadline().as_secs_f32()
            )),
        };
        match answer {
            Ok(vectors) => {
                let done = sent.pop_front().expect("checked above");
                lane.count(vectors.len());
                let _ = done.reply.send(Ok(vectors));
            }
            Err(e) => fail(&lane, &mut worker, &mut sent, format!("{e:#}")),
        }
    }
}

fn reason_of(status: &Status) -> String {
    match status {
        Status::Unavailable { reason } | Status::Failed { reason } => reason.clone(),
        other => format!("{other:?}"),
    }
}

/// Run the known-answer check on every lane this machine could use: the CPU
/// in this process, then each worker lane that is on in a worker of its own.
/// A lane with nothing to run on is reported with the reason, and a lane that
/// is off says how to turn it on.
///
/// What `semlith doctor --gpu` prints and the Doctor page shows. Components a
/// lane needs are fetched first, as a run would, and `say` narrates that.
pub fn check_all(say: impl Fn(&str)) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    say("checking the CPU lane");
    let cpu = check_cpu();
    out.push(match cpu {
        Ok(check) => serde_json::to_value(check).unwrap_or_default(),
        Err(e) => serde_json::json!({ "lane": "cpu", "passed": false, "reason": format!("{e:#}") }),
    });
    let on = enabled();
    for spec in SPECS {
        let id = spec.id;
        let base = serde_json::json!({
            "lane": id,
            "label": spec.label,
            "experimental": spec.experimental,
        });
        let row = |extra: serde_json::Value| {
            let mut row = base.clone();
            if let (Some(row), Some(extra)) = (row.as_object_mut(), extra.as_object()) {
                row.extend(extra.clone());
            }
            row
        };
        if let Some(why) = unavailable_here(id) {
            if id != "worker" {
                out.push(row(serde_json::json!({
                    "reason": format!("unavailable — {why}"),
                    "fallback": "the CPU carries the run",
                })));
            }
            continue;
        }
        // A lane that is off is not checked: checking it would fetch its
        // components, and an experimental lane's are fetched only on an
        // explicit turn-on.
        if !on.lane(id) {
            if id != "worker" {
                out.push(row(serde_json::json!({
                    "reason": format!("off — semlith accel on {id} turns it on"),
                })));
            }
            continue;
        }
        say(&format!("checking the {} lane", spec.label));
        let lane = Arc::new(Lane::new(spec));
        out.push(match start(&lane) {
            Ok((_, hello)) => row(serde_json::json!({
                "device": hello["device"],
                "variant": hello["variant"],
                "cosine": hello["cosine"],
                "chunks_per_s": hello["chunks_per_s"],
                "passed": true,
            })),
            Err(e) => {
                let text = format!("{e:#}");
                match text.strip_prefix("unavailable — ") {
                    Some(reason) => row(serde_json::json!({
                        "reason": format!("unavailable — {reason}"),
                        "fallback": "the CPU carries the run",
                    })),
                    None => row(serde_json::json!({
                        "passed": false,
                        "reason": text,
                        "fallback": "the lane is failed and the CPU carries the run",
                    })),
                }
            }
        });
    }
    out
}

/// The WebGPU adapter a setting names, if any: `SEMLITH_GPU_ADAPTER` or the
/// saved `gpu_adapter`. Passed to a WebGPU worker, which prefers it.
fn adapter_choice() -> Option<String> {
    std::env::var(crate::gpu::ADAPTER_ENV)
        .ok()
        .or_else(|| crate::home::Settings::load().gpu_adapter)
        .filter(|name| !name.trim().is_empty())
}

/// What a lane's worker is started with: its lane name and a directory.
fn worker_args(lane: &Arc<Lane>) -> Result<Vec<String>> {
    if let Some(why) = unavailable_here(lane.id) {
        bail!("unavailable — {why}");
    }
    let cache = crate::model_cache_dir()?;
    let progress = |percent: u8| {
        lane.set(Status::Downloading {
            percent,
            eta_ms: None,
        })
    };
    Ok(match lane.id {
        "ane" => {
            // Fetched by `setup` or `accel on ane`, never by a run: a run on a
            // Mac without the pack indexes on the GPU and the CPU instead.
            let Some(pack) = crate::packs::installed(&cache, &crate::packs::coreml()) else {
                bail!(
                    "unavailable — the Neural Engine pack is not installed; `semlith accel on ane` \
                     or `semlith setup` fetches it"
                );
            };
            vec!["ane".into(), pack.display().to_string()]
        }
        "gpu" => {
            let device = detect_gpu().map_err(|why| anyhow::anyhow!("unavailable — {why}"))?;
            *lane.device.lock().unwrap_or_else(|e| e.into_inner()) = Some(device);
            // A Mac with the Core ML pack runs its GPU through Core ML.
            if cfg!(target_os = "macos")
                && let Some(pack) = crate::packs::installed(&cache, &crate::packs::coreml())
            {
                vec!["gpu-coreml".into(), pack.display().to_string()]
            } else {
                let dir = crate::gpu::fetch_webgpu(&cache, &mut |p| progress(p))
                    .context("fetching the WebGPU components")?;
                let mut args = vec!["gpu".into(), dir.display().to_string()];
                if let Some(name) = adapter_choice() {
                    args.push(name);
                }
                args
            }
        }
        "cuda" => {
            if let Some(why) = crate::cuda::unavailable_here() {
                bail!("unavailable — {why}");
            }
            // Fetched only because somebody turned CUDA on, which is the only
            // way this lane is ever started. The fp16 weights are the WebGPU
            // lane's; the pack is NVIDIA's runtime and ORT's GPU build.
            crate::gpu::fetch_fp16(&cache, &mut |p| progress(p))
                .context("fetching the fp16 model")?;
            let pack = crate::cuda::fetch_pack(&cache, &mut |p| progress(p))
                .context("fetching the CUDA pack")?;
            if let Ok(device) = crate::cuda::detect() {
                *lane.device.lock().unwrap_or_else(|e| e.into_inner()) = Some(device.name);
            }
            vec!["cuda".into(), pack.display().to_string()]
        }
        id @ ("trt" | "openvino" | "llama") => {
            let pack = pack_for(id).expect("these lanes have packs");
            if id != "llama" {
                crate::gpu::fetch_fp16(&cache, &mut |p| progress(p))
                    .context("fetching the fp16 model")?;
            }
            let dir = crate::packs::fetch(&cache, &pack, &mut |p| progress(p))
                .with_context(|| format!("fetching the {} pack", pack.name))?;
            vec![id.into(), dir.display().to_string()]
        }
        _ => vec!["worker".into()],
    })
}

/// The Core ML worker's protocol: bump it whenever the `__embed-worker ane`
/// or `gpu-coreml` code, its arguments or its frames change, and a fresh copy
/// of the binary becomes the worker (see [`coreml_worker`]).
pub const COREML_WORKER: u32 = 3;

/// Run the Core ML lanes' worker from the binary that is running now rather
/// than the stable copy: for developing the worker itself.
pub const COREML_WORKER_ENV: &str = "SEMLITH_COREML_WORKER";

/// The executable the Core ML lanes run as: a copy of semlith kept beside the
/// models, made once per [`COREML_WORKER`] and left alone by upgrades.
///
/// macOS compiles a model for the Neural Engine the first time a program loads
/// it — minutes — and caches the result for that program: a new semlith binary
/// compiled all six models again, on every upgrade and every build. A worker
/// whose executable never changes compiles them once per machine, and every
/// start after that loads them in about a second and a half. A hard link where
/// it can be, so the copy costs nothing until the binary it came from is
/// replaced; a copy where it cannot.
pub fn coreml_worker(cache: &Path) -> Result<PathBuf> {
    let current = std::env::current_exe().context("locating this binary")?;
    if std::env::var(COREML_WORKER_ENV).is_ok_and(|v| v == "current") {
        return Ok(current);
    }
    let dir = component_dir(cache, &format!("coreml-worker-v{COREML_WORKER}"));
    // Named `semlith`, as the installed binary is: macOS keeps a program's
    // compiled models under its name.
    let exe = dir.join(if cfg!(windows) {
        "semlith.exe"
    } else {
        "semlith"
    });
    if exe.is_file() {
        return Ok(exe);
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let temp = dir.join(format!(".semlith.{}", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    if std::fs::hard_link(&current, &temp).is_err() {
        std::fs::copy(&current, &temp)
            .with_context(|| format!("copying this binary to {}", temp.display()))?;
    }
    std::fs::rename(&temp, &exe).with_context(|| format!("placing {}", exe.display()))?;
    // Earlier protocols' workers are nobody's any more.
    if let Ok(entries) = std::fs::read_dir(cache.join("accel")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("coreml-worker-v") && entry.path() != dir {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(exe)
}

/// Where macOS keeps what it compiled for the Core ML worker, which is named
/// `semlith`: under the program's name in the user's caches.
const E5_CACHE: &str = "Library/Caches/semlith/com.apple.e5rt.e5bundlecache";

/// Delete the compiles macOS was writing for a worker that was killed: each
/// stays behind as a `<hash>.tmp.<pid>_<n>.bundle` of about 94 MB that nothing
/// reads or removes. They matter beyond the space: macOS empties its compile
/// cache when the disk runs low, and on a Mac with 13 GB free it had emptied
/// all six buckets within ten minutes, so the next start compiled again.
fn sweep_abandoned_compiles(root: &Path) {
    let within = |dir: &Path| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    };
    for build in within(root) {
        for key in within(&build) {
            for bundle in within(&key) {
                let name = bundle.file_name().unwrap_or_default().to_string_lossy();
                let pid = name
                    .split_once(".tmp.")
                    .and_then(|(_, rest)| rest.split('_').next())
                    .and_then(|pid| pid.parse::<u32>().ok());
                if pid.is_some_and(|pid| !crate::daemon::alive(pid)) {
                    let _ = std::fs::remove_dir_all(&bundle);
                }
            }
        }
    }
}

/// Held by a Core ML worker while it loads its models: macOS compiles them one
/// program at a time, and a second semlith asking for the same compile while
/// the first is under way only queued another minutes-long compile behind it.
/// The second waits here instead, then loads what the first compiled.
pub fn coreml_compile_lock() -> Option<std::fs::File> {
    let dir = crate::model_cache_dir().ok()?.join("accel");
    std::fs::create_dir_all(&dir).ok()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("coreml-compile.lock"))
        .ok()?;
    file.lock().ok()?;
    Some(file)
}

/// Whether a Core ML worker holds the compile lock, which it does until the
/// last of its buckets has loaded.
fn coreml_compiling() -> bool {
    cfg!(target_os = "macos")
        && crate::model_cache_dir().is_ok_and(|cache| {
            std::fs::OpenOptions::new()
                .write(true)
                .open(cache.join("accel").join("coreml-compile.lock"))
                .is_ok_and(|file| matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
        })
}

/// Start a lane's worker: fetch what it needs, spawn it, and read its hello,
/// which carries the known-answer check. A worker loading models for the
/// first time says how far it has got before it says hello.
fn start(lane: &Arc<Lane>) -> Result<(Worker, serde_json::Value)> {
    let mut args = vec!["__embed-worker".to_string()];
    args.extend(worker_args(lane)?);
    lane.set(Status::Starting);
    let exe = if cfg!(target_os = "macos") && matches!(args[1].as_str(), "ane" | "gpu-coreml") {
        if let Ok(home) = crate::home::user_home() {
            sweep_abandoned_compiles(&home.join(E5_CACHE));
        }
        coreml_worker(&crate::model_cache_dir()?)?
    } else {
        std::env::current_exe().context("locating this binary")?
    };
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
        stdin: Some(stdin),
        answers,
    };
    // The hello, and with it the known-answer check. Bounded like a batch
    // once the models are loaded; a first load says how far it has got.
    // The deadline is for silence, not for the whole start: every progress
    // frame renews it, so a compile queued behind another program's is not
    // failed for taking its turn.
    let mut started = Instant::now();
    let began = Instant::now();
    let timed = crate::model_cache_dir()
        .ok()
        .map(|cache| component_dir(&cache, &format!("{}.compile-ms", lane.id)));
    *lane.expected.lock().unwrap_or_else(|e| e.into_inner()) = timed
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|ms| ms.trim().parse().ok())
        .map(Duration::from_millis)
        .or((lane.id == "ane").then_some(FIRST_COMPILE));
    let mut compiled = None;
    let hello = loop {
        if began.elapsed() > START_CAP {
            bail!(
                "the worker was not ready within {} min",
                START_CAP.as_secs() / 60
            );
        }
        let left = HELLO_DEADLINE.saturating_sub(started.elapsed());
        let frame = match worker.answers.recv_timeout(left) {
            Ok(Ok(frame)) => frame,
            Ok(Err(e)) => bail!("the worker exited before it was ready: {e}"),
            Err(_) => bail!(
                "the worker was not ready within {} s",
                HELLO_DEADLINE.as_secs()
            ),
        };
        let value: serde_json::Value =
            serde_json::from_slice(&frame).context("the worker's hello")?;
        if value["waiting"].as_bool() == Some(true) {
            started = Instant::now();
            continue;
        }
        if let (Some(done), Some(of)) = (value["loaded"].as_u64(), value["of"].as_u64()) {
            started = Instant::now();
            compiled.get_or_insert(started);
            lane.set(Status::Compiling {
                percent: (done * 100 / of.max(1)).min(100) as u8,
                eta_ms: None,
            });
            continue;
        }
        break value;
    };
    // A load that compiled is the next one's time left; one served from
    // macOS's cache says nothing about the next compile.
    if let (Some(since), Some(path)) = (compiled, timed)
        && since.elapsed() >= Duration::from_secs(10)
    {
        let _ = std::fs::write(path, since.elapsed().as_millis().to_string());
    }
    if hello["ok"].as_bool() != Some(true) {
        let reason = hello["reason"].as_str().unwrap_or("no reason given");
        if hello["unavailable"].as_bool() == Some(true) {
            bail!("unavailable — {reason}");
        }
        bail!("{reason}");
    }
    if let Some(per_s) = hello["batch_per_s"].as_f64() {
        note_known_answer(lane.id, per_s);
    }
    if let Some(device) = hello["device"].as_str() {
        *lane.device.lock().unwrap_or_else(|e| e.into_inner()) = Some(device.to_string());
    }
    if let Some(variant) = hello["variant"].as_str().and_then(static_variant) {
        *lane.variant.lock().unwrap_or_else(|e| e.into_inner()) = variant;
    }
    Ok((worker, hello))
}

/// The slowest batched rate a worker lane may show on the fixture and still be
/// used. Every hardware lane measured is far above it, even on a loaded
/// machine (an M1's GPU 17.8 and Neural Engine 21 while another run embedded;
/// an A30's CUDA 863); llvmpipe standing in for an A30 managed 0.7.
const MIN_LANE_PER_S: f64 = 2.0;

/// A worker's variant as the `'static` name a store's counts use.
fn static_variant(name: &str) -> Option<&'static str> {
    [
        "int8-cpu",
        "fp16-webgpu",
        "fp16-coreml-gpu",
        "fp16-ane",
        "fp16-cuda",
        "fp16-trt",
        "openvino",
        "gguf-f16",
    ]
    .into_iter()
    .find(|v| *v == name)
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

/// A request frame: the row count, then each row's length and ids, every
/// number a little-endian `u32`. Ids rather than text, so a worker never
/// tokenises what the run already tokenised.
pub fn encode_ids(batch: &[crate::session::Ids]) -> Vec<u8> {
    let words = 1 + batch.iter().map(|ids| 1 + ids.len()).sum::<usize>();
    let mut out = Vec::with_capacity(words * 4);
    out.extend_from_slice(&(batch.len() as u32).to_le_bytes());
    for ids in batch {
        out.extend_from_slice(&(ids.len() as u32).to_le_bytes());
        for id in ids {
            out.extend_from_slice(&id.to_le_bytes());
        }
    }
    out
}

/// [`encode_ids`] read back, refusing a frame whose lengths do not add up.
pub fn decode_ids(frame: &[u8]) -> Result<Vec<crate::session::Ids>> {
    let words: Vec<u32> = frame
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect();
    if !frame.len().is_multiple_of(4) || words.is_empty() {
        bail!("a request frame of {} bytes", frame.len());
    }
    let rows = words[0] as usize;
    let mut at = 1;
    let mut batch = Vec::with_capacity(rows.min(4096));
    for _ in 0..rows {
        let len = *words.get(at).context("a request frame cut short")? as usize;
        let ids = words
            .get(at + 1..at + 1 + len)
            .context("a request frame cut short")?;
        batch.push(ids.to_vec());
        at += 1 + len;
    }
    if at != words.len() {
        bail!("a request frame with {} words left over", words.len() - at);
    }
    Ok(batch)
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
    Ok(vectors_of(body))
}

/// Little-endian `f32`s, `DIM` to a vector. The caller has checked the length.
fn vectors_of(bytes: &[u8]) -> Vec<Vec<f32>> {
    let values: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    values.chunks(DIM).map(<[f32]>::to_vec).collect()
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
    let vectors = vectors_of(FIXTURE_VECTORS);
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
    /// Every chunk under the floor, by its place in the fixture: whether a
    /// failure is one long chunk or spread over all of them (#197).
    pub low: Vec<(usize, f32)>,
    pub chunks_per_s: f64,
    pub passed: bool,
}

impl Check {
    /// `3 of 32 chunks below the floor: #4 0.9989, #17 0.9991, #30 0.9989`.
    pub fn low_line(&self) -> String {
        let each: Vec<String> = self
            .low
            .iter()
            .map(|(at, cosine)| format!("#{at} {cosine:.4}"))
            .collect();
        format!(
            "{} of {} chunks below the floor: {}",
            self.low.len(),
            fixture().0.len(),
            each.join(", ")
        )
    }
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
    // Every variant but the CPU's int8 is full or half precision, and held to
    // the fp16 floor: llama.cpp's GGUF and OpenVINO's are too.
    let floor = if variant == "int8-cpu" {
        MIN_COSINE_INT8
    } else {
        MIN_COSINE_FP16
    };
    let started = Instant::now();
    let mut worst = 1.0f32;
    let mut low = Vec::new();
    for (at, (text, want)) in texts.iter().zip(&expected).enumerate() {
        let got = embed(std::slice::from_ref(text))?;
        let got = got.first().context("no vector came back")?;
        let cosine = crate::index::cosine(got, want);
        worst = worst.min(cosine);
        if cosine < floor {
            low.push((at, cosine));
        }
    }
    let elapsed = started.elapsed().as_secs_f64().max(1e-6);
    Ok(Check {
        lane: lane.to_string(),
        device: device.to_string(),
        variant: variant.to_string(),
        cosine: worst,
        low,
        chunks_per_s: (texts.len() as f64 / elapsed * 10.0).round() / 10.0,
        passed: worst >= floor,
    })
}

// ----------------------------------------------------------------- the worker

/// `semlith __embed-worker <lane> [dir] [adapter]`: load the lane's session,
/// check it against the fixture, say hello, then answer batches until stdin
/// closes.
pub fn worker_main(lane: &str, dir: Option<&Path>, adapter: Option<&str>) -> i32 {
    let mut out = std::io::stdout().lock();
    let mut input = std::io::stdin().lock();
    let say = |out: &mut std::io::StdoutLock, value: serde_json::Value| {
        let _ = write_frame(out, &serde_json::to_vec(&value).unwrap_or_default());
    };
    // A worker runs at the priority an index run has, whatever the process
    // that started it was doing at the time.
    crate::priority::worker();
    let fault = Fault::read(lane);
    if matches!(fault, Some(Fault::Load)) {
        say(
            &mut out,
            serde_json::json!({ "ok": false, "reason": format!("{FAULT_ENV} failed the load") }),
        );
        return 1;
    }
    // Said every half minute until the hello, so the host knows a worker
    // whose first compile is queued behind another program's is alive.
    let loading = std::sync::Arc::new(AtomicBool::new(true));
    {
        let beat = std::sync::Arc::clone(&loading);
        std::thread::spawn(move || {
            while beat.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(30));
                if beat.load(Ordering::Relaxed) {
                    let frame = serde_json::to_vec(&serde_json::json!({ "waiting": true }))
                        .unwrap_or_default();
                    let _ = write_frame(&mut std::io::stdout().lock(), &frame);
                }
            }
        });
    }
    let opened = {
        let mut progress = |loaded: usize, of: usize| {
            say(
                &mut std::io::stdout().lock(),
                serde_json::json!({ "loaded": loaded, "of": of }),
            );
        };
        crate::gpu::Session::open(lane, dir, adapter, &mut progress)
    };
    loading.store(false, Ordering::Relaxed);
    let mut session = match opened {
        Ok(session) => session,
        Err(e) => {
            let text = format!("{e:#}");
            let unavailable = text.starts_with("unavailable");
            say(
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
    // The worker tokenises only here, for the fixture; a run's batches arrive
    // as ids.
    let tokenizer = crate::model_cache_dir().and_then(|cache| crate::session::tokenizer(&cache));
    let mut batched = None;
    let check = tokenizer.and_then(|tokenizer| {
        let check = known_answer(lane, &session.device(), session.variant(), |texts| {
            let batch = texts
                .iter()
                .map(|text| crate::session::encode(&tokenizer, text))
                .collect::<Result<Vec<_>>>()?;
            let rows: Vec<&[u32]> = batch.iter().map(Vec::as_slice).collect();
            session.embed(&rows)
        })?;
        // The check embeds one chunk at a time, which is latency, not what a
        // run's batches manage: the fixture again in batches of eight, timed,
        // is this lane's first figure for an estimate.
        if check.passed {
            let (texts, _) = fixture();
            let ids = texts
                .iter()
                .map(|text| crate::session::encode(&tokenizer, text))
                .collect::<Result<Vec<_>>>()?;
            let started = Instant::now();
            for group in ids.chunks(8) {
                let rows: Vec<&[u32]> = group.iter().map(Vec::as_slice).collect();
                session.embed(&rows)?;
            }
            batched = Some(ids.len() as f64 / started.elapsed().as_secs_f64().max(1e-6));
        }
        Ok(check)
    });
    // A lane slower than any GPU is not on one. On a Linux box whose
    // container lacks the graphics capability, or with no vendor Vulkan
    // driver, Dawn runs on llvmpipe while the device it reports is still the
    // card's PCI id, and the "NVIDIA" lane embedded on the CPU at under one
    // chunk a second (#197, measured on an A30). Refused, saying so.
    if let (Ok(check), Some(per_s)) = (&check, batched)
        && check.passed
        && per_s < MIN_LANE_PER_S
    {
        say(
            &mut out,
            serde_json::json!({
                "ok": false,
                "unavailable": true,
                "reason": format!(
                    "{} embedded at {per_s:.1} chunks/s, slower than any GPU, so it is most likely a software renderer standing in for the card (on Linux: the GPU maker's Vulkan driver is missing, or the container has no graphics capability)",
                    check.device
                ),
            }),
        );
        return 1;
    }
    match check {
        Ok(check) if check.passed => say(
            &mut out,
            serde_json::json!({
                "ok": true,
                "device": check.device,
                "variant": check.variant,
                "cosine": check.cosine,
                "chunks_per_s": check.chunks_per_s,
                "batch_per_s": batched,
            }),
        ),
        Ok(check) => {
            say(
                &mut out,
                serde_json::json!({
                    "ok": false,
                    "reason": format!(
                        "{} failed the known-answer check: cosine {:.4} against the fp32 fixture ({})",
                        check.device, check.cosine, check.low_line()
                    ),
                }),
            );
            return 1;
        }
        Err(e) => {
            say(
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
        let answer = match decode_ids(&frame).and_then(|batch| {
            let rows: Vec<&[u32]> = batch.iter().map(Vec::as_slice).collect();
            session.embed(&rows)
        }) {
            Ok(mut vectors) => {
                if let (Some(Fault::Poison(value)), Some(first)) = (fault, vectors.first_mut()) {
                    first.iter_mut().for_each(|v| *v = value);
                }
                encode_vectors(&vectors)
            }
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
    /// Every batch's first vector filled with this: zero, NaN or infinity.
    Poison(f32),
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
            ("poison", Some("zero")) => Some(Fault::Poison(0.0)),
            ("poison", Some("nan")) => Some(Fault::Poison(f32::NAN)),
            ("poison", Some("inf")) => Some(Fault::Poison(f32::INFINITY)),
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

/// Where the worker lanes' components live, for `semlith accel remove`.
pub fn component_dir(cache: &Path, lane: &str) -> PathBuf {
    cache.join("accel").join(lane)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed check names every chunk under the floor, not only the worst
    /// (#197: the NVIDIA report gave the minimum and nothing else).
    #[test]
    fn a_failed_check_names_every_chunk_under_the_floor() {
        let (texts, expected) = fixture();
        let mut at = 0;
        let check = known_answer("gpu", "test", "fp16-webgpu", |_| {
            let mut v = expected[at].clone();
            // Two chunks bent off their fixture vector, the rest exact.
            if at == 3 || at == 17 {
                v[0] += 0.3;
                v[1] -= 0.3;
            }
            at += 1;
            Ok(vec![v])
        })
        .unwrap();
        assert!(!check.passed);
        assert_eq!(
            check.low.iter().map(|l| l.0).collect::<Vec<_>>(),
            vec![3, 17]
        );
        let line = check.low_line();
        assert!(
            line.starts_with(&format!("2 of {} chunks below the floor: #3 ", texts.len())),
            "{line}"
        );
    }

    /// The Core ML worker is a copy made once and reused, whatever binary asks
    /// for it next, and an earlier protocol's copy goes.
    #[test]
    fn the_core_ml_worker_is_made_once_and_kept() {
        let cache = tempfile::tempdir().unwrap();
        let old = component_dir(cache.path(), "coreml-worker-v0");
        std::fs::create_dir_all(&old).unwrap();
        let made = coreml_worker(cache.path()).unwrap();
        assert!(made.is_file());
        assert!(made.starts_with(cache.path()));
        assert_eq!(
            made.file_name()
                .unwrap()
                .to_string_lossy()
                .trim_end_matches(".exe"),
            "semlith"
        );
        assert!(
            !old.exists(),
            "the earlier protocol's worker is still there"
        );
        let modified = std::fs::metadata(&made).unwrap().modified().unwrap();
        let again = coreml_worker(cache.path()).unwrap();
        assert_eq!(again, made);
        assert_eq!(
            std::fs::metadata(&again).unwrap().modified().unwrap(),
            modified
        );
    }

    /// A compiling lane is on its way, a failed one is not, and the time left
    /// counts down between steps rather than climbing.
    #[test]
    fn a_compiling_lane_is_waited_for_and_its_time_left_counts_down() {
        let lane = Lane::new(spec("ane").unwrap());
        // The clock the lane reads, measured here rather than assumed from the
        // nominal sleep: a slow macOS runner overshot 1 100 ms by 150 and failed
        // a bound written around it (#166). Started before the lane's own, so
        // the lane can never have been compiling for longer than `taken` says:
        // taken after it, a preempted Linux runner put 84 ms between the two
        // and the lane's estimate overshot the bound.
        let began = std::time::Instant::now();
        lane.set(Status::Compiling {
            percent: 0,
            eta_ms: None,
        });
        // And how far behind it the lane's clock started. A runner preempted
        // between the two put the lane's clock 82 ms behind and its estimate
        // under a lower bound that assumed none (#177, the second time): the
        // lane's elapsed time is `taken` less this, give or take a read.
        let lag = began.elapsed().as_millis() as u64;
        assert!(lane.coming() && !lane.ready());
        // Before it has moved, the time the last compile took counts down.
        *lane.expected.lock().unwrap() = Some(Duration::from_secs(40));
        let eta = |lane: &Lane| match lane.status() {
            Status::Compiling { eta_ms, .. } => eta_ms.unwrap(),
            other => panic!("{other:?}"),
        };
        assert!((39_000..=40_000).contains(&eta(&lane)));
        std::thread::sleep(Duration::from_millis(1100));
        assert!(eta(&lane) <= 39_000);
        // Half done after `taken`, so as much again is left, less the moment
        // between the step and the read.
        let taken = began.elapsed().as_millis() as u64;
        lane.set(Status::Compiling {
            percent: 50,
            eta_ms: None,
        });
        let first = eta(&lane);
        let slack = began.elapsed().as_millis() as u64 - taken + 50;
        assert!(
            (taken.saturating_sub(slack + lag).max(1_000)..=taken + slack).contains(&first),
            "{first} after {taken} ms, the lane's clock {lag} ms behind"
        );
        std::thread::sleep(Duration::from_millis(300));
        // Floored at a second, and never above what it said before.
        assert!(eta(&lane) <= first);
        lane.set(Status::Failed { reason: "x".into() });
        assert!(!lane.coming());
        lane.set(Status::Active);
        assert!(lane.ready() && !lane.coming());
    }

    /// A failed lane stays failed for a while and is then tried again, rather
    /// than for the life of the process.
    #[test]
    fn a_failed_lane_is_tried_again_after_its_cool_down() {
        let lane = Lane::new(spec("ane").unwrap());
        lane.set(Status::Failed { reason: "x".into() });
        assert!(!lane.usable());
        // Failing again does not restart the wait.
        let first = *lane.failed_at.lock().unwrap();
        lane.set(Status::Failed { reason: "y".into() });
        assert_eq!(*lane.failed_at.lock().unwrap(), first);
        *lane.failed_at.lock().unwrap() = Instant::now().checked_sub(LANE_RETRY);
        assert!(lane.usable());
        assert_eq!(lane.status(), Status::Idle);
        assert!(lane.failed_at.lock().unwrap().is_none());
    }

    /// A compile left by a killed worker is deleted; one still being written
    /// and a finished one are not.
    #[cfg(unix)]
    #[test]
    fn abandoned_compiles_are_swept_and_nothing_else() {
        let root = tempfile::tempdir().unwrap();
        let key = root.path().join("26A428").join("3BADE4");
        let bundle = |name: &str| {
            let path = key.join(name);
            std::fs::create_dir_all(path.join("H13G.bundle")).unwrap();
            path
        };
        let dead = bundle("E3A0.tmp.999999999_1.bundle");
        let live = bundle(&format!("E3A0.tmp.{}_1.bundle", std::process::id()));
        let done = bundle("E3A0.bundle");
        sweep_abandoned_compiles(root.path());
        assert!(!dead.exists());
        assert!(live.exists() && done.exists());
    }

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
    fn id_frames_round_trip_and_refuse_a_bad_length() {
        let batch = vec![vec![1u32, 2, 3], vec![], vec![70_000]];
        assert_eq!(decode_ids(&encode_ids(&batch)).unwrap(), batch);
        let mut cut = encode_ids(&batch);
        cut.truncate(cut.len() - 4);
        assert!(decode_ids(&cut).is_err());
        let mut long = encode_ids(&batch);
        long.extend_from_slice(&9u32.to_le_bytes());
        assert!(decode_ids(&long).is_err());
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
        unsafe { std::env::set_var(FAULT_ENV, "ane:poison:zero") };
        assert_eq!(Fault::read("ane"), Some(Fault::Poison(0.0)));
        unsafe { std::env::remove_var(FAULT_ENV) };
    }
}
