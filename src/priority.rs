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
//! On Windows the same edges drive the priority class and EcoQoS power
//! throttling. Linux is not managed: an unprivileged process cannot lower its
//! nice value again once it has raised it, and an idle daemon costs no CPU.

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
/// it stands now, and what the last switch cost.
pub fn snapshot() -> serde_json::Value {
    match MANAGER.get() {
        None => serde_json::json!({
            "managed": false,
            "state": "normal",
            "embedding": 0,
            "switches": 0,
        }),
        Some(manager) => {
            let inner = manager.lock();
            serde_json::json!({
                "managed": true,
                "state": if inner.background { "background" } else { "normal" },
                "embedding": inner.active,
                "switches": inner.switches,
                "last_switch_us": inner.last_switch_us,
            })
        }
    }
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
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Threading::{
        BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, NORMAL_PRIORITY_CLASS,
        PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling, SetPriorityClass,
        SetProcessInformation,
    };

    pub const SUPPORTED: bool = true;

    pub fn set(background: bool) -> Result<(), String> {
        let class = if background {
            BELOW_NORMAL_PRIORITY_CLASS
        } else {
            NORMAL_PRIORITY_CLASS
        };
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
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    pub const SUPPORTED: bool = false;

    pub fn set(_background: bool) -> Result<(), String> {
        Ok(())
    }
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
