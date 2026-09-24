//! `system::read` checked against the operating system's own figures.
//!
//! Every default the release proposes is derived from this one reading, so a
//! reading that is wrong on one OS is a wrong default for everybody on it. The
//! in-crate tests reuse the crate's own parser; these read the figure a second,
//! independent way, which is what catches a parser that is consistently wrong.

use semlith::system;

/// The derivation budgets threads out of this number, so it has to be the one
/// the standard library would hand any other part of the process.
#[test]
fn logical_cores_are_available_parallelism() {
    let expected = std::thread::available_parallelism().unwrap().get();
    assert_eq!(system::read().logical_cores, expected);
}

/// Within 1% rather than exact: the crate may round or read a different unit,
/// and a percent is far below any difference a derivation would notice.
#[cfg(any(target_os = "linux", windows))]
fn assert_within_one_percent(read_mb: u64, os_mb: u64) {
    assert!(os_mb > 0, "the OS reported no memory");
    assert!(
        read_mb.abs_diff(os_mb) * 100 <= os_mb,
        "system::read says {read_mb} MiB, the OS says {os_mb} MiB"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn total_memory_matches_proc_meminfo() {
    let meminfo = std::fs::read_to_string("/proc/meminfo").expect("/proc/meminfo");
    let kb: u64 = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("a MemTotal line in /proc/meminfo");
    assert_within_one_percent(system::read().total_memory_mb, kb / 1024);
}

#[test]
#[cfg(windows)]
fn total_memory_matches_global_memory_status() {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: status is a live MEMORYSTATUSEX whose dwLength the call is told,
    // which is the whole of that API's contract.
    assert!(unsafe { GlobalMemoryStatusEx(&mut status) } != 0);
    assert_within_one_percent(
        system::read().total_memory_mb,
        status.ullTotalPhys / 1024 / 1024,
    );
}
