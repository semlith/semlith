//! Priority that follows the work.
//!
//! The daemon used to be launched by a launchd plist asking for `ProcessType
//! Background`, which on Apple silicon means the efficiency cores and nothing
//! else. Measured on the reference M1: 28 chunks/s from a terminal, 3.3 under
//! background policy — and a process launchd starts as `Background` cannot lift
//! itself out of it (`setpriority` returns 0 and the scheduler priority stays
//! at 4). So the plist now asks for `Standard`, and the daemon puts *itself* in
//! background state while nothing is embedding, which a process may undo at
//! will: measured at under 100 µs in both directions.
//!
//! One process-wide count of embed passes decides it. Every index pass and
//! every query embedding holds an [`Embedding`] guard; the count going from 0
//! to 1 lifts the process to normal priority on the spot, and going from 1 to 0
//! drops it back after [`GRACE`], so a burst of short embeds — a query, a saved
//! file — stays lifted rather than toggling the scheduler several times a
//! second.
//!
//! Only the daemon opts in, through [`manage`]. A `semlith index` in a
//! terminal is already at the priority its user gave it, and nothing here
//! touches it.
//!
//! On Windows the same edges drive EcoQoS power throttling: on while idle,
//! explicitly off while anything embeds, at below-normal priority either way,
//! so a bulk index never competes with the person at the keyboard. On Linux
//! the systemd user unit sets the baseline (`Nice=5`, `CPUWeight=50`,
//! `IOWeight=50`) and the edges move nothing process-wide, because an
//! unprivileged process cannot lower its nice value again once it has raised
//! it.
//!
//! From 0.32.0 the threads of an index run in the daemon also say what kind of
//! work they are, through [`indexing_thread`]: Utility QoS on macOS, which is
//! work a person is not waiting on but which must not be parked on the
//! efficiency cores, and `SCHED_BATCH` on Linux, which the scheduler may give
//! longer slices and never preempts a person for.

use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// How long the count must stay at zero before the daemon drops back.
///
/// Long enough to hold a run's gaps between batches and a burst of searches
/// together; short enough that the acceptance's "within one second of the last
/// embedding ending" holds with room to spare.
pub const GRACE: Duration = Duration::from_millis(300);

struct Inner {
    active: usize,
    background: bool,
    idle_since: Option<Instant>,
    switches: u64,
    last_switch_us: u64,
    /// The lift came from a request that has not embedded anything, so
    /// neither it nor the drop after it is logged. The portal polls about once
    /// a second, and two log lines a poll buried everything else in the log.
    quiet: bool,
}

struct Manager {
    inner: Mutex<Inner>,
    wake: Condvar,
    log: Box<dyn Fn(&str) + Send + Sync>,
}

static MANAGER: OnceLock<Manager> = OnceLock::new();

impl Manager {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Flip the process and say so, with how long the flip itself took.
    fn switch(&self, inner: &mut Inner, background: bool, why: &str, say: bool) {
        let started = Instant::now();
        let result = platform::set(background);
        let took = started.elapsed().as_micros() as u64;
        inner.background = background;
        inner.switches += 1;
        inner.last_switch_us = took;
        let to = if background { "background" } else { "normal" };
        match result {
            Ok(()) if !say => {}
            Ok(()) => (self.log)(&format!("priority: {to} ({why}) in {took} µs")),
            // Never fatal. An older Windows build without power throttling, or
            // a sandbox that refuses the call, leaves a daemon that runs at one
            // priority all the time, which is what every release before this
            // one did.
            Err(e) => (self.log)(&format!("priority: could not switch to {to}: {e}")),
        }
    }
}

/// Make the daemon's priority follow its work, starting in background state.
///
/// `false` where the platform has nothing to switch. Called once, by the
/// daemon; a second call is ignored.
pub fn manage(log: impl Fn(&str) + Send + Sync + 'static) -> bool {
    if !platform::SUPPORTED {
        log("priority: not switched on this platform (an idle daemon uses no CPU)");
        return false;
    }
    let fresh = MANAGER.set(Manager {
        inner: Mutex::new(Inner {
            active: 0,
            background: false,
            idle_since: Some(Instant::now()),
            switches: 0,
            last_switch_us: 0,
            quiet: false,
        }),
        wake: Condvar::new(),
        log: Box::new(log),
    });
    if fresh.is_err() {
        return true;
    }
    let manager = MANAGER.get().expect("just set");
    {
        let mut inner = manager.lock();
        manager.switch(&mut inner, true, "idle at start", true);
    }
    std::thread::Builder::new()
        .name("semlith-priority".to_string())
        .spawn(|| settle(MANAGER.get().expect("set before the thread")))
        .map(|_| true)
        .unwrap_or(false)
}

/// Drop to background once the count has sat at zero for [`GRACE`].
fn settle(manager: &'static Manager) {
    let mut inner = manager.lock();
    loop {
        match (inner.active, inner.background, inner.idle_since) {
            (0, false, Some(at)) => {
                let left = GRACE.saturating_sub(at.elapsed());
                if left.is_zero() {
                    let say = !inner.quiet;
                    inner.quiet = false;
                    manager.switch(&mut inner, true, "idle", say);
                } else {
                    inner = manager
                        .wake
                        .wait_timeout(inner, left)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
            }
            _ => {
                inner = manager.wake.wait(inner).unwrap_or_else(|e| e.into_inner());
            }
        }
    }
}

/// Held for as long as something is embedding. See the module comment.
#[must_use = "the priority drops the moment the guard does"]
pub struct Embedding(bool);

/// Say that an embed pass has started.
pub fn embedding() -> Embedding {
    hold(false)
}

/// Say that an HTTP request is being served: lifted like an embed pass, so a
/// control route answers at once whatever the machine is doing, but logged
/// only if the request goes on to embed something.
pub fn request() -> Embedding {
    hold(true)
}

fn hold(request: bool) -> Embedding {
    let Some(manager) = MANAGER.get() else {
        return Embedding(false);
    };
    let mut inner = manager.lock();
    inner.active += 1;
    inner.idle_since = None;
    if inner.background {
        inner.quiet = request;
        manager.switch(&mut inner, false, "embedding", !request);
    } else if !request && inner.quiet {
        // Lifted quietly by the request this embed is serving: say so now,
        // so a search still reads as a lift and a drop in the log.
        inner.quiet = false;
        (manager.log)(&format!(
            "priority: normal (embedding) in {} µs",
            inner.last_switch_us
        ));
    }
    Embedding(true)
}

impl Drop for Embedding {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let Some(manager) = MANAGER.get() else {
            return;
        };
        let mut inner = manager.lock();
        inner.active = inner.active.saturating_sub(1);
        if inner.active == 0 {
            inner.idle_since = Some(Instant::now());
            manager.wake.notify_all();
        }
    }
}

/// What `/api/about` reports: whether the daemon manages its priority, where
/// it stands now, what the last switch cost, and how many index threads are
/// at the run's class right now (checked by reading it back, not assumed).
pub fn snapshot() -> serde_json::Value {
    let threads = INDEXING_THREADS.load(std::sync::atomic::Ordering::Relaxed);
    match MANAGER.get() {
        None => serde_json::json!({
            "managed": false,
            "state": "normal",
            "embedding": 0,
            "switches": 0,
            "indexing_threads": threads,
            "indexing_class": platform::INDEXING_CLASS,
        }),
        Some(manager) => {
            let inner = manager.lock();
            serde_json::json!({
                "managed": true,
                "state": if inner.background { "background" } else { "normal" },
                "embedding": inner.active,
                "switches": inner.switches,
                "last_switch_us": inner.last_switch_us,
                "indexing_threads": threads,
                "indexing_class": platform::INDEXING_CLASS,
            })
        }
    }
}

/// Index threads in the daemon at the run's class right now.
static INDEXING_THREADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Held by each thread of an index run: the class a bulk index runs at, put
/// back when the guard goes. Nothing outside the daemon: a terminal run keeps
/// the priority its user gave it.
#[must_use = "the thread's class is restored the moment the guard drops"]
pub struct IndexingThread(Option<platform::Saved>);

pub fn indexing_thread() -> IndexingThread {
    if MANAGER.get().is_none() {
        return IndexingThread(None);
    }
    let saved = platform::enter_indexing();
    if saved.is_some() {
        INDEXING_THREADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    IndexingThread(saved)
}

impl Drop for IndexingThread {
    fn drop(&mut self) {
        if let Some(saved) = self.0.take() {
            platform::leave_indexing(saved);
            INDEXING_THREADS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// An embed worker's own start: never parked in background state, whatever
/// the process that started it was doing at the moment it did.
pub fn worker() {
    platform::clear_background();
}

#[cfg(target_os = "macos")]
mod platform {
    pub const SUPPORTED: bool = true;

    /// `PRIO_DARWIN_PROCESS` and `PRIO_DARWIN_BG` from `<sys/resource.h>`,
    /// which the libc crate does not export for macOS.
    const PRIO_DARWIN_PROCESS: libc::c_int = 4;
    const PRIO_DARWIN_BG: libc::c_int = 0x1000;

    pub fn set(background: bool) -> Result<(), String> {
        let value = if background { PRIO_DARWIN_BG } else { 0 };
        // SAFETY: plain integers; `who` 0 names this process.
        let rc = unsafe { libc::setpriority(PRIO_DARWIN_PROCESS, 0, value) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }

    pub fn clear_background() {
        let _ = set(false);
    }

    pub const INDEXING_CLASS: &str = "utility";

    /// The thread's class before the run took it.
    pub type Saved = libc::qos_class_t;

    /// Utility QoS for this thread, read back to be sure it took.
    pub fn enter_indexing() -> Option<Saved> {
        // SAFETY: this thread's own handle, and out-parameters that are live
        // locals of the types the calls write.
        unsafe {
            let mut before = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            let mut relative = 0;
            libc::pthread_get_qos_class_np(libc::pthread_self(), &mut before, &mut relative);
            if libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) != 0 {
                return None;
            }
            let mut now = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            libc::pthread_get_qos_class_np(libc::pthread_self(), &mut now, &mut relative);
            (now as u32 == libc::qos_class_t::QOS_CLASS_UTILITY as u32).then_some(before)
        }
    }

    pub fn leave_indexing(before: Saved) {
        // An unspecified class cannot be set back; default is what an
        // unspecified thread runs at.
        let back = match before {
            libc::qos_class_t::QOS_CLASS_UNSPECIFIED => libc::qos_class_t::QOS_CLASS_DEFAULT,
            other => other,
        };
        // SAFETY: a class value from the enum, for this thread only.
        unsafe {
            libc::pthread_set_qos_class_self_np(back, 0);
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Threading::{
        BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
    };

    pub const SUPPORTED: bool = true;

    /// Below-normal either way: a bulk index never competes with the person
    /// at the keyboard. What the edge moves is EcoQoS, on while idle and
    /// explicitly off while anything embeds, so Windows never parks a run on
    /// the efficiency cores or clocks it down.
    pub fn set(background: bool) -> Result<(), String> {
        let class = BELOW_NORMAL_PRIORITY_CLASS;
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            // Set means "throttle me"; clear, with the bit still in the
            // control mask, means "never throttle me", which is stronger than
            // leaving the decision to Windows.
            StateMask: if background {
                PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            } else {
                0
            },
        };
        // SAFETY: the pseudo-handle for this process needs no closing, and the
        // struct is passed with its own size, which is the documented contract.
        let (class_ok, throttle_ok) = unsafe {
            let me = GetCurrentProcess();
            (
                SetPriorityClass(me, class) != 0,
                SetProcessInformation(
                    me,
                    ProcessPowerThrottling,
                    &state as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                ) != 0,
            )
        };
        match (class_ok, throttle_ok) {
            (true, true) => Ok(()),
            // Power throttling exists from Windows 10 1709. Before that the
            // priority class alone is the switch, and that is not an error.
            (true, false) => Ok(()),
            _ => Err(std::io::Error::last_os_error().to_string()),
        }
    }

    pub fn clear_background() {
        let _ = set(false);
    }

    pub const INDEXING_CLASS: &str = "below-normal, EcoQoS off";

    pub type Saved = ();

    /// The process class carries a run on Windows; a thread has nothing of
    /// its own to change, and says so by counting itself.
    pub fn enter_indexing() -> Option<Saved> {
        Some(())
    }

    pub fn leave_indexing(_: Saved) {}
}

#[cfg(target_os = "linux")]
mod platform {
    /// Managed, in the sense that the edges are logged and the index threads
    /// take `SCHED_BATCH`; the process's nice value is the unit's to set.
    pub const SUPPORTED: bool = true;

    pub fn set(_background: bool) -> Result<(), String> {
        Ok(())
    }

    pub fn clear_background() {}

    pub const INDEXING_CLASS: &str = "SCHED_BATCH";

    /// The thread's policy and priority before the run took it.
    pub type Saved = (libc::c_int, libc::sched_param);

    pub fn enter_indexing() -> Option<Saved> {
        // SAFETY: 0 names this thread; the parameter structs are live locals
        // the calls read and write.
        unsafe {
            let policy = libc::sched_getscheduler(0);
            let mut before: libc::sched_param = std::mem::zeroed();
            libc::sched_getparam(0, &mut before);
            let batch = libc::sched_param { sched_priority: 0 };
            if libc::sched_setscheduler(0, libc::SCHED_BATCH, &batch) != 0 {
                return None;
            }
            (libc::sched_getscheduler(0) == libc::SCHED_BATCH).then_some((policy, before))
        }
    }

    pub fn leave_indexing((policy, before): Saved) {
        // SAFETY: as above; putting back what was read.
        unsafe {
            libc::sched_setscheduler(0, policy, &before);
        }
    }
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod platform {
    pub const SUPPORTED: bool = false;

    pub fn set(_background: bool) -> Result<(), String> {
        Ok(())
    }

    pub fn clear_background() {}

    pub const INDEXING_CLASS: &str = "unmanaged";

    pub type Saved = ();

    pub fn enter_indexing() -> Option<Saved> {
        None
    }

    pub fn leave_indexing(_: Saved) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unmanaged process takes guards and gives them back without touching
    /// anything — which is every test binary and every terminal command.
    #[test]
    fn a_guard_outside_the_daemon_does_nothing() {
        let guard = embedding();
        assert!(!guard.0 || MANAGER.get().is_some());
        drop(guard);
    }
}
