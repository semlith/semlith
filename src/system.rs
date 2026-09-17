//! One reading of the machine, and the defaults derived from it.
//!
//! Every number the release proposes — how many stores index at once, how many
//! threads a writer gets, how much memory an index may hold — comes from
//! [`read`] and [`derive`]. There is one reading and one derivation so the
//! admission queue, the portal's Index page and the CLI cannot disagree about
//! the machine they are all running on, and so a user shown a number can be
//! told the sentence that produced it.

/// Memory the derivation refuses to spend, in megabytes.
///
/// The portal, the watcher threads and whatever else the user is doing live
/// here. Spending only what is available *beyond* this is what keeps a tight
/// machine proposing one run rather than a number that swaps.
pub const RESERVE_MB: u64 = 2048;

/// Peak resident memory one indexing run costs, in megabytes.
///
/// A shipped default, measured on the release machine. It is what the first
/// run on any machine is derived from, because the first run is the one that
/// needs a default and there is nothing yet to measure.
///
/// It is not replaced by a per-machine measurement. That was the plan, and
/// nothing implements it: the value reaches [`derive`] from here and from
/// nowhere else. Said plainly rather than left as a doc comment describing
/// behaviour the code does not have.
// ponytail: one shipped figure for every machine. The upgrade path is the
// daemon recording its own peak across a run and deriving from that instead
// when the two differ by more than a quarter — a different corpus, model or
// page size is a different peak, and a laptop is not the release machine.
pub const PER_RUN_PEAK_MB: u64 = 1500;

/// What the platform says about this machine.
///
/// A figure of `0` for either memory field means the read failed — that is not
/// the same as a machine with no memory, and [`derive`] treats it as unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine {
    pub logical_cores: usize,
    /// Physical cores where the platform says; `None` where it does not.
    pub physical_cores: Option<usize>,
    pub total_memory_mb: u64,
    /// Memory free *now*, not at start — the derivation has to live beside
    /// whatever else is already resident.
    pub available_memory_mb: u64,
}

/// One derived default and the sentence that produced it.
///
/// `reason` is rendered verbatim beside the field in the portal, so it is prose
/// a user reads rather than a debug dump.
#[derive(Debug, Clone)]
pub struct Derivation {
    pub value: usize,
    pub reason: String,
}

/// The three defaults the release proposes.
#[derive(Debug, Clone)]
pub struct Derived {
    pub runs_at_once: Derivation,
    pub threads_per_writer: Derivation,
    pub index_memory_mb: Derivation,
}

/// Read this machine, per platform. Never panics; never reports zero cores.
pub fn read() -> Machine {
    let logical_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    platform(logical_cores)
}

// ---------------------------------------------------------------------- linux

#[cfg(target_os = "linux")]
fn platform(logical_cores: usize) -> Machine {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    Machine {
        logical_cores,
        physical_cores: linux_physical_cores(),
        total_memory_mb: meminfo_kb(&meminfo, "MemTotal") / 1024,
        available_memory_mb: meminfo_kb(&meminfo, "MemAvailable") / 1024,
    }
}

/// `MemAvailable` rather than `MemFree`: the kernel's own estimate of what a
/// new workload can have without swapping, which is the question being asked.
#[cfg(target_os = "linux")]
fn meminfo_kb(meminfo: &str, key: &str) -> u64 {
    meminfo
        .lines()
        .find_map(|line| {
            let rest = line.strip_prefix(key)?.strip_prefix(':')?;
            rest.split_whitespace().next()?.parse::<u64>().ok()
        })
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn linux_physical_cores() -> Option<usize> {
    // Keyed on (package, core) rather than `core_id` alone: core ids restart at
    // zero in each socket, so counting them bare halves a two-socket machine.
    let mut cores = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir("/sys/devices/system/cpu").ok()?.flatten() {
        let topology = entry.path().join("topology");
        let Ok(core) = std::fs::read_to_string(topology.join("core_id")) else {
            continue;
        };
        let package = std::fs::read_to_string(topology.join("physical_package_id"))
            .unwrap_or_default()
            .trim()
            .to_string();
        cores.insert((package, core.trim().to_string()));
    }
    (!cores.is_empty()).then_some(cores.len())
}

// ---------------------------------------------------------------------- macos

#[cfg(target_os = "macos")]
fn platform(logical_cores: usize) -> Machine {
    Machine {
        logical_cores,
        physical_cores: sysctl::<i32>(c"hw.physicalcpu")
            .filter(|n| *n > 0)
            .map(|n| n as usize),
        total_memory_mb: sysctl::<u64>(c"hw.memsize").unwrap_or(0) / 1024 / 1024,
        available_memory_mb: macos_available_mb(),
    }
}

#[cfg(target_os = "macos")]
fn sysctl<T: Default>(name: &std::ffi::CStr) -> Option<T> {
    let mut out = T::default();
    let mut len = std::mem::size_of::<T>();
    // SAFETY: name is a NUL-terminated C string, and out/len describe a live
    // T whose size sysctlbyname is told about and will not exceed.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut out as *mut T as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && len == std::mem::size_of::<T>()).then_some(out)
}

/// Free, inactive and purgeable pages. Inactive and purgeable are counted
/// because the kernel hands them to a new allocation without swapping — the
/// bare free count reads as a few hundred megabytes on any machine that has
/// been up an hour, and would propose one run on a workstation.
#[cfg(target_os = "macos")]
// libc deprecates `mach_host_self` in favour of the `mach2` crate. The host
// port is three lines of this one function and a crate is a crate, so the
// deprecation is taken here rather than in `Cargo.toml`.
#[allow(deprecated)]
fn macos_available_mb() -> u64 {
    let mut vm: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: vm is a live vm_statistics64 and count is its size in i32 words,
    // which is what host_statistics64 is told and will not exceed.
    let rc = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            &mut vm as *mut libc::vm_statistics64 as libc::host_info64_t,
            &mut count,
        )
    };
    if rc != 0 {
        return 0;
    }
    // SAFETY: sysconf reads a static system limit and takes no pointer.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page <= 0 {
        return 0;
    }
    let pages =
        u64::from(vm.free_count) + u64::from(vm.inactive_count) + u64::from(vm.purgeable_count);
    pages.saturating_mul(page as u64) / 1024 / 1024
}

// -------------------------------------------------------------------- windows

#[cfg(windows)]
fn platform(logical_cores: usize) -> Machine {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: status is a live MEMORYSTATUSEX whose dwLength the call is told,
    // which is the whole of that API's contract.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    Machine {
        logical_cores,
        // Windows reports physical cores only through a variable-length buffer
        // walk; the derivation does not use the field, so it stays unanswered
        // rather than guessed.
        physical_cores: None,
        total_memory_mb: if ok {
            status.ullTotalPhys / 1024 / 1024
        } else {
            0
        },
        available_memory_mb: if ok {
            status.ullAvailPhys / 1024 / 1024
        } else {
            0
        },
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform(logical_cores: usize) -> Machine {
    Machine {
        logical_cores,
        physical_cores: None,
        total_memory_mb: 0,
        available_memory_mb: 0,
    }
}

// ------------------------------------------------------------------ derivation

/// Propose the three defaults for this machine, with the sentence behind each.
///
/// Runs and threads are solved together rather than in sequence: capping runs
/// by `cores / threads` after computing threads from runs lets the product run
/// past the core count, which is the bound that matters.
pub fn derive(machine: &Machine, per_run_peak_mb: u64) -> Derived {
    let peak = per_run_peak_mb.max(1);
    // One core stays free whatever the memory says, so the portal still answers
    // while every writer is busy. A single-core machine keeps the floor of 1.
    let core_budget = machine.logical_cores.saturating_sub(1).max(1);
    let unknown_memory = machine.available_memory_mb == 0;
    let headroom = machine.available_memory_mb.saturating_sub(RESERVE_MB);

    let memory_runs = if unknown_memory {
        1
    } else {
        usize::try_from(headroom / peak)
            .unwrap_or(usize::MAX)
            .max(1)
    };
    let runs = memory_runs.min(core_budget);

    let runs_reason = if unknown_memory {
        format!(
            "the memory reading failed, so one run at a time until there is a reading, on {} logical cores",
            machine.logical_cores
        )
    } else if headroom < peak {
        // The floor, said out loud. This used to read "1.4 GiB minus a 2 GiB
        // reserve, at 1.5 GiB a run, allows 1" — a subtraction that is
        // negative concluding that it allows one, which is the floor applying
        // and the sentence not admitting it.
        format!(
            "{} free is under the {} reserve plus {} for a run, so the floor of 1 applies; {} logical cores with one kept free would allow {core_budget}",
            gib(machine.available_memory_mb),
            gib(RESERVE_MB),
            gib(peak),
            machine.logical_cores,
        )
    } else {
        format!(
            "{} free minus a {} reserve, at {} a run, allows {memory_runs}, and {} logical cores with one kept free allows {core_budget}, so {runs}",
            gib(machine.available_memory_mb),
            gib(RESERVE_MB),
            gib(peak),
            machine.logical_cores,
        )
    };

    let index_memory = index_memory_mb(headroom);
    Derived {
        runs_at_once: Derivation {
            value: runs,
            reason: runs_reason,
        },
        threads_per_writer: threads_for(machine, runs),
        index_memory_mb: Derivation {
            value: index_memory,
            reason: format!(
                "{index_memory} MiB a store: the {} MiB floor, doubled once past 16 GiB beyond the reserve and again past 64 GiB, and {} is free beyond the reserve out of the {} free now",
                crate::index::INDEX_MEMORY_MB,
                gib(headroom),
                gib(machine.available_memory_mb),
            ),
        },
    }
}

/// The threads-per-writer derivation for a given number of runs at once.
///
/// Split out because the panel must recompute it when the number of runs
/// changes. Set runs to 3 and the help text under "threads each" went on
/// saying "split between 1 run", because it was the derivation for the runs
/// this machine would have chosen rather than for the runs in force.
pub fn threads_for(machine: &Machine, runs: usize) -> Derivation {
    let core_budget = machine.logical_cores.saturating_sub(1).max(1);
    let runs = runs.clamp(1, core_budget);
    let threads = (crate::embed::embed_threads() / runs).clamp(1, core_budget / runs);
    Derivation {
        value: threads,
        reason: format!(
            "{} embedding threads split between {} and held inside {core_budget} cores, so {threads} each",
            crate::embed::embed_threads(),
            runs_phrase(runs),
        ),
    }
}

/// The index budget rises in two steps rather than continuously, because a
/// figure a user can recognise is worth more here than a fitted curve.
fn index_memory_mb(headroom_mb: u64) -> usize {
    let base = crate::index::INDEX_MEMORY_MB;
    match headroom_mb {
        0..16_384 => base,
        16_384..65_536 => base * 2,
        _ => base * 4,
    }
}

/// These strings are read by a person in the portal, so "1 runs" is a bug.
fn runs_phrase(runs: usize) -> String {
    if runs == 1 {
        "1 run".to_string()
    } else {
        format!("{runs} runs")
    }
}

fn gib(mb: u64) -> String {
    let text = format!("{:.1}", mb as f64 / 1024.0);
    format!("{} GiB", text.trim_end_matches(".0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(logical_cores: usize, total_gib: u64, available_gib: u64) -> Machine {
        Machine {
            logical_cores,
            physical_cores: None,
            total_memory_mb: total_gib * 1024,
            available_memory_mb: available_gib * 1024,
        }
    }

    /// The three bounds the admission queue is allowed to assume.
    fn assert_bounds(m: &Machine, d: &Derived) {
        let runs = d.runs_at_once.value;
        let threads = d.threads_per_writer.value;
        assert!(runs >= 1, "{m:?} proposed {runs} runs");
        assert!(threads >= 1, "{m:?} proposed {threads} threads");
        assert!(
            runs * threads <= m.logical_cores,
            "{m:?} proposed {runs} x {threads} on {} cores",
            m.logical_cores
        );
        assert!(
            !d.runs_at_once.reason.is_empty()
                && !d.threads_per_writer.reason.is_empty()
                && !d.index_memory_mb.reason.is_empty()
        );
    }

    #[test]
    fn bounds_hold_on_every_shape_of_machine() {
        let machines = [
            machine(1, 2, 2), // the tiny one: a single core and 2 GiB
            machine(2, 4, 3), // available is under the reserve
            machine(4, 8, 6),
            machine(8, 16, 12),
            machine(10, 32, 24),
            machine(32, 128, 100), // the large one
            machine(64, 512, 480),
        ];
        for m in machines {
            let d = derive(&m, PER_RUN_PEAK_MB);
            assert_bounds(&m, &d);
        }
    }

    #[test]
    fn one_free_core_survives_a_machine_with_more_than_one() {
        let m = machine(32, 128, 100);
        let d = derive(&m, PER_RUN_PEAK_MB);
        assert!(d.runs_at_once.value * d.threads_per_writer.value < m.logical_cores);
    }

    #[test]
    fn a_failed_memory_read_still_proposes_one_run() {
        let m = Machine {
            logical_cores: 8,
            physical_cores: None,
            total_memory_mb: 0,
            available_memory_mb: 0,
        };
        let d = derive(&m, PER_RUN_PEAK_MB);
        assert_bounds(&m, &d);
        assert_eq!(d.runs_at_once.value, 1);
        assert!(
            d.runs_at_once.reason.contains("reading failed"),
            "reason must say the read failed, got: {}",
            d.runs_at_once.reason
        );
    }

    /// A peak of zero is a measurement that went wrong, not a free run.
    #[test]
    fn a_zero_peak_does_not_divide_by_zero() {
        let m = machine(8, 16, 12);
        let d = derive(&m, 0);
        assert_bounds(&m, &d);
    }

    #[test]
    fn reads_this_machine() {
        let m = read();
        assert!(m.logical_cores >= 1);
        let d = derive(&m, PER_RUN_PEAK_MB);
        assert_bounds(&m, &d);
        println!("machine: {m:?}");
        println!(
            "runs at once      = {} ({})",
            d.runs_at_once.value, d.runs_at_once.reason
        );
        println!(
            "threads per writer = {} ({})",
            d.threads_per_writer.value, d.threads_per_writer.reason
        );
        println!(
            "index memory MB    = {} ({})",
            d.index_memory_mb.value, d.index_memory_mb.reason
        );
    }

    /// Against the platform's own tool, and skipped where the tool is absent.
    #[test]
    #[cfg(target_os = "macos")]
    fn total_memory_matches_sysctl() {
        let Ok(out) = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        else {
            return;
        };
        let Ok(bytes) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>() else {
            return;
        };
        assert_eq!(read().total_memory_mb, bytes / 1024 / 1024);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn total_memory_matches_proc_meminfo() {
        let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") else {
            return;
        };
        assert_eq!(
            read().total_memory_mb,
            meminfo_kb(&meminfo, "MemTotal") / 1024
        );
    }
}
