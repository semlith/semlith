//! The CPU cap: the most of this machine's CPU semlith's own process may use
//! while it indexes, from 0 to 100 % of every logical core.
//!
//! The owner's choice (2026-10-08) is semlith's own CPU, not the whole
//! machine's and not a load average: on the Semlith Cloud box the service, its
//! database and its identity provider share four vCPUs with indexing, and the
//! cap is what keeps an index from taking all four. The CPU lane can never be
//! switched off, so this is the control instead.
//!
//! Enforced by pacing, not by fewer threads: before each CPU batch and each
//! prepared file, [`pace`] reads how much CPU the process has used over the
//! last two seconds and sleeps until that share is back under the cap. The
//! GPU, Neural Engine and remote lanes run in other processes or other
//! machines and are slowed only by the prepare stage they share. 0 % pauses
//! CPU work until the cap is raised.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::daemon::Source;

/// The variable that sets the cap, which wins over the saved setting.
pub const ENV: &str = "SEMLITH_CPU_CAP";

/// The window the cap is held over.
const WINDOW: Duration = Duration::from_secs(2);

/// The longest one wait inside [`pace`] lasts before the cap is read again,
/// so a raised cap or a stopped run is noticed within a quarter second.
const STEP: Duration = Duration::from_millis(250);

/// Aim a little under the cap, so the one-second samples a reader takes stay
/// under it and not only the two-second average.
const AIM: f64 = 0.85;

/// The cap in force and where it came from: the environment, then the saved
/// setting, then 100 (off). Values above 100 read as 100.
pub fn in_force() -> (u8, Source) {
    if let Some(n) = std::env::var(ENV)
        .ok()
        .and_then(|raw| raw.trim().trim_end_matches('%').parse::<u64>().ok())
    {
        return (n.min(100) as u8, Source::Environment);
    }
    match crate::home::Settings::load().cpu_cap_percent {
        Some(n) => (n.min(100), Source::Saved),
        None => (100, Source::Derived),
    }
}

/// The cap, read again at most every half second: [`pace`] asks before every
/// prepared file, and the saved setting is a file.
pub fn percent() -> u8 {
    static CACHED: Mutex<Option<(Instant, u8)>> = Mutex::new(None);
    let mut cached = CACHED.lock().unwrap_or_else(|e| e.into_inner());
    match *cached {
        Some((at, cap)) if at.elapsed() < Duration::from_millis(500) => cap,
        _ => {
            let cap = in_force().0;
            *cached = Some((Instant::now(), cap));
            cap
        }
    }
}

/// This process's CPU time so far, user and system, over all its threads.
#[cfg(unix)]
pub fn process_cpu_seconds() -> f64 {
    // SAFETY: getrusage writes one rusage into the zeroed value it is given.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0.0;
    }
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    tv(usage.ru_utime) + tv(usage.ru_stime)
}

#[cfg(windows)]
pub fn process_cpu_seconds() -> f64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let zero = || FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero(), zero(), zero(), zero());
    // SAFETY: the pseudo-handle needs no closing, and each pointer is a live
    // FILETIME on this stack.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return 0.0;
    }
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) as f64;
    // In 100-nanosecond ticks.
    (ticks(kernel) + ticks(user)) / 1e7
}

#[cfg(not(any(unix, windows)))]
pub fn process_cpu_seconds() -> f64 {
    0.0
}

/// The cap's share of the cores, at least one; `None` with no cap.
fn capped_cores() -> Option<usize> {
    let cap = percent();
    (cap < 100).then(|| ((cores() * f64::from(cap) / 100.0).floor() as usize).max(1))
}

/// Files prepared at once under the cap: half the cap's cores, rounded up.
fn prepare_slots() -> usize {
    capped_cores().map_or(usize::MAX, |n| n.div_ceil(2))
}

/// The most embedding threads a CPU batch may use under the cap: what the
/// prepare stage leaves of the cap's cores, at least one. Unbounded with no
/// cap. Measured on the M1 with eight threads: each stage given the whole
/// share ran a 50 % cap to 56 %.
pub fn thread_ceiling() -> usize {
    capped_cores().map_or(usize::MAX, |n| n.saturating_sub(prepare_slots()).max(1))
}

/// Prepared files under way now, held to [`prepare_slots`] while capped.
static BUSY: Mutex<usize> = Mutex::new(0);
static FREED: std::sync::Condvar = std::sync::Condvar::new();

/// One file's preparation under the cap: waits for room, and gives it back
/// when dropped. Every prepare thread starting a file at the same moment ran
/// all the cores past the cap between two paced checks.
pub struct Slot(());

impl Drop for Slot {
    fn drop(&mut self) {
        let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        *busy = busy.saturating_sub(1);
        FREED.notify_one();
    }
}

/// Pace, then take a slot for one file's preparation.
pub fn slot() -> Slot {
    pace();
    let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
    // Re-read on a timeout, so a raised cap lets the waiters through.
    while *busy >= prepare_slots() {
        busy = FREED
            .wait_timeout(busy, STEP)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    *busy += 1;
    Slot(())
}

/// Run `work` — vector encoding, which turbovec spreads over rayon's pool of
/// every core — on a pool the size of the cap's share of the cores, while
/// capped. Measured on the M1 at a 25 % cap: the global pool's bursts took two
/// one-second samples to 37 %.
pub fn within<R: Send>(work: impl FnOnce() -> R + Send) -> R {
    static POOL: Mutex<Option<(usize, std::sync::Arc<rayon::ThreadPool>)>> = Mutex::new(None);
    let Some(threads) = capped_cores() else {
        return work();
    };
    let pool = {
        let mut slot = POOL.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            Some((n, pool)) if *n == threads => Some(std::sync::Arc::clone(pool)),
            _ => rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("semlith-capped-{i}"))
                .build()
                .ok()
                .map(std::sync::Arc::new)
                .inspect(|pool| *slot = Some((threads, std::sync::Arc::clone(pool)))),
        }
    };
    match pool {
        Some(pool) => pool.install(work),
        None => work(),
    }
}

fn cores() -> f64 {
    std::thread::available_parallelism().map_or(1, |n| n.get()) as f64
}

/// Readings of the process's CPU time, newest last, kept for a little over
/// one window.
static SAMPLES: Mutex<VecDeque<(Instant, f64)>> = Mutex::new(VecDeque::new());

/// Take a reading and return the share of all cores the process used since
/// the newest reading at least `over` old (or the oldest kept), in percent,
/// with the seconds it spans. `None` when there is nothing to compare with.
fn share(over: Duration) -> Option<(f64, f64)> {
    let now = Instant::now();
    let cpu = process_cpu_seconds();
    let mut samples = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
    // A reading every 20 ms is plenty for a two-second window, and keeps the
    // list short when every prepared file asks.
    if samples
        .back()
        .is_none_or(|(at, _)| now.duration_since(*at) >= Duration::from_millis(20))
    {
        samples.push_back((now, cpu));
    }
    while samples.len() > 2
        && samples
            .get(1)
            .is_some_and(|(at, _)| now.duration_since(*at) >= WINDOW + Duration::from_secs(1))
    {
        samples.pop_front();
    }
    let &(since, from) = samples
        .iter()
        .rev()
        .find(|(at, _)| now.duration_since(*at) >= over)
        .or(samples.front())?;
    let span = now.duration_since(since).as_secs_f64();
    if span < 0.05 {
        return None;
    }
    Some(((cpu - from).max(0.0) / (span * cores()) * 100.0, span))
}

/// The share of all cores semlith's process used over about the last second,
/// in percent, as the Machine card shows it beside the cap.
pub fn measured_percent() -> f64 {
    static LAST: Mutex<f64> = Mutex::new(0.0);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((percent, _)) = share(Duration::from_secs(1)) {
        *last = (percent * 10.0).round() / 10.0;
    }
    *last
}

static PAUSED: AtomicBool = AtomicBool::new(false);
static PACING: AtomicBool = AtomicBool::new(false);

/// Whether CPU work is paused by a 0 % cap right now.
pub fn paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}

/// What a run card says while the cap holds CPU work back entirely.
pub fn paused_line() -> Option<String> {
    paused().then(|| "paused by the CPU cap (0 %); raise it to resume".to_string())
}

/// Wait, if the process is over its cap, until it is back under it. Called
/// before each CPU batch and each prepared file; returns at once with the cap
/// at 100 %.
pub fn pace() {
    loop {
        let cap = percent();
        if cap >= 100 {
            settle();
            return;
        }
        if cap == 0 {
            if !PAUSED.swap(true, Ordering::Relaxed) {
                eprintln!("semlith: CPU work paused by the CPU cap (0 %)");
            }
            std::thread::sleep(STEP);
            continue;
        }
        if PAUSED.swap(false, Ordering::Relaxed) {
            eprintln!("semlith: CPU work resumed under a {cap} % CPU cap");
        }
        let Some((used, span)) = share(WINDOW) else {
            return;
        };
        let target = f64::from(cap) * AIM;
        if used <= target {
            return;
        }
        if !PACING.swap(true, Ordering::Relaxed) {
            eprintln!("semlith: pacing CPU work to the {cap} % CPU cap ({used:.0} % used)");
        }
        // The wall time the CPU already spent would need to fit the cap, less
        // the time already covered.
        let need = span * used / target - span;
        std::thread::sleep(Duration::from_secs_f64(need.max(0.01)).min(STEP));
    }
}

fn settle() {
    PAUSED.store(false, Ordering::Relaxed);
    PACING.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_wins_and_is_clamped_to_a_hundred() {
        crate::home::with_env_var(ENV, "250".as_ref(), || {
            assert_eq!(in_force(), (100, Source::Environment));
        });
        crate::home::with_env_var(ENV, "35%".as_ref(), || {
            assert_eq!(in_force(), (35, Source::Environment));
        });
    }

    #[test]
    fn process_cpu_time_moves_when_the_process_works() {
        let before = process_cpu_seconds();
        let started = Instant::now();
        let mut x = 0u64;
        while started.elapsed() < Duration::from_millis(200) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
        assert!(process_cpu_seconds() > before);
    }
}
