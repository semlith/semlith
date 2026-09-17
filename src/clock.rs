//! Local time, with the offset that says which local.
//!
//! The ledger is one dataset read from two places. The CLI printed
//! `13:40:32` from UTC arithmetic while the portal printed `19:00:18` from the
//! browser's clock, and neither said which zone it meant, so correlating a
//! portal event with a ledger row was off by the offset and nothing on either
//! screen admitted it. Both now print local time and both name the offset.
//!
//! The offset comes from the operating system, because that is the only thing
//! that knows it: a zone is a political fact with a history, not a formula.
//! There is no new dependency for it — `libc` is already in the tree for the
//! Unix side, and the Windows side is one documented call.

/// Seconds east of UTC, right now, as this machine reckons it.
///
/// Read per call rather than cached at startup: a daemon left running across a
/// daylight-saving change would otherwise print the old offset for months, and
/// the call is a few microseconds.
pub fn utc_offset_secs() -> i32 {
    platform_offset().unwrap_or(0)
}

#[cfg(unix)]
fn platform_offset() -> Option<i32> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as libc::time_t;
    let mut out: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `now` is a valid `time_t` and `out` is a live, zeroed `tm` that
    // outlives the call. `localtime_r` is the reentrant form precisely so it
    // writes into the caller's buffer and touches no global state.
    let filled = unsafe { libc::localtime_r(&now, &mut out) };
    if filled.is_null() {
        return None;
    }
    Some(out.tm_gmtoff as i32)
}

#[cfg(windows)]
#[repr(C)]
#[allow(non_snake_case)]
struct TimeZoneInformation {
    Bias: i32,
    StandardName: [u16; 32],
    StandardDate: [u16; 8],
    StandardBias: i32,
    DaylightName: [u16; 32],
    DaylightDate: [u16; 8],
    DaylightBias: i32,
}

#[cfg(windows)]
unsafe extern "system" {
    fn GetTimeZoneInformation(info: *mut TimeZoneInformation) -> u32;
}

#[cfg(windows)]
fn platform_offset() -> Option<i32> {
    // 0 unknown, 1 standard, 2 daylight, 0xFFFF_FFFF failed.
    const INVALID: u32 = 0xFFFF_FFFF;
    const DAYLIGHT: u32 = 2;
    let mut info: TimeZoneInformation = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a live, zeroed structure of exactly the layout the API
    // documents, and it outlives the call.
    let which = unsafe { GetTimeZoneInformation(&mut info) };
    if which == INVALID {
        return None;
    }
    // Windows reports minutes *west* of UTC, and splits the seasonal part out.
    let bias = info.Bias
        + if which == DAYLIGHT {
            info.DaylightBias
        } else {
            info.StandardBias
        };
    Some(-bias * 60)
}

#[cfg(not(any(unix, windows)))]
fn platform_offset() -> Option<i32> {
    None
}

/// `2026-09-17 19:00:18 +05:30` — one timestamp, saying which clock it is on.
///
/// Written out rather than formatted by a date library, because the whole of
/// what is needed is a civil calendar and an offset, and the one thing that
/// must not vary is what the two surfaces print.
pub fn local_stamp(at: i64) -> String {
    let offset = utc_offset_secs();
    let (y, mo, d, h, mi, s) = civil_from_unix(at + offset as i64);
    let sign = if offset < 0 { '-' } else { '+' };
    let off = offset.abs();
    format!(
        "{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} {sign}{:02}:{:02}",
        off / 3600,
        (off % 3600) / 60
    )
}

/// `19:00:18 +05:30` — the same clock, without the date, for a dense table.
pub fn local_clock(at: i64) -> String {
    let offset = utc_offset_secs();
    let (_, _, _, h, mi, s) = civil_from_unix(at + offset as i64);
    let sign = if offset < 0 { '-' } else { '+' };
    let off = offset.abs();
    format!(
        "{h:02}:{mi:02}:{s:02} {sign}{:02}:{:02}",
        off / 3600,
        (off % 3600) / 60
    )
}

/// Seconds since the epoch to a civil date, by Howard Hinnant's `civil_from_days`.
///
/// Proleptic Gregorian, valid either side of the epoch, no table and no leap
/// seconds — which is what a Unix timestamp already assumes.
fn civil_from_unix(at: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = at.div_euclid(86_400);
    let rest = at.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rest / 3600) as u32,
        ((rest % 3600) / 60) as u32,
        (rest % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_and_a_known_instant_convert() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        // 2026-09-17T13:52:00Z, the hour of the drive that found this.
        assert_eq!(civil_from_unix(1_789_653_120), (2026, 9, 17, 13, 52, 0));
        // Before the epoch, which the formula handles and a naive one does not.
        assert_eq!(civil_from_unix(-1), (1969, 12, 31, 23, 59, 59));
    }

    #[test]
    fn a_stamp_carries_its_offset() {
        let stamp = local_stamp(1_789_653_120);
        let offset = &stamp[stamp.len() - 6..];
        assert!(
            offset.starts_with('+') || offset.starts_with('-'),
            "a timestamp with no offset is the bug this exists to fix: {stamp}"
        );
        assert_eq!(&offset[3..4], ":", "the offset is written +HH:MM: {stamp}");
        // The clock form is the tail of the full one.
        assert!(local_stamp(0).ends_with(&local_clock(0)));
    }
}
