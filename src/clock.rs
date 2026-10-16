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
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    offset_at(now)
}

/// Seconds east of UTC at the instant `at`, by this machine's zone rules.
///
/// A past row is printed with the offset in force when it was written. Until
/// 0.42.0 every row took today's, so rows from the other side of a
/// daylight-saving change printed an hour off.
pub fn offset_at(at: i64) -> i32 {
    platform_offset(at).unwrap_or(0)
}

#[cfg(unix)]
fn platform_offset(at: i64) -> Option<i32> {
    let now = at as libc::time_t;
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
#[repr(C)]
#[derive(Default, Clone, Copy)]
#[allow(non_snake_case)]
struct SystemTime {
    wYear: u16,
    wMonth: u16,
    wDayOfWeek: u16,
    wDay: u16,
    wHour: u16,
    wMinute: u16,
    wSecond: u16,
    wMilliseconds: u16,
}

#[cfg(windows)]
unsafe extern "system" {
    fn SystemTimeToTzSpecificLocalTime(
        zone: *const TimeZoneInformation,
        universal: *const SystemTime,
        local: *mut SystemTime,
    ) -> i32;
}

#[cfg(windows)]
fn platform_offset(at: i64) -> Option<i32> {
    // The zone's own rules for that date: a null zone is the current one,
    // with its daylight-saving dates applied to `at` rather than to today.
    let (y, mo, d, h, mi, s) = civil_from_unix(at);
    let universal = SystemTime {
        wYear: u16::try_from(y).ok()?,
        wMonth: mo as u16,
        wDay: d as u16,
        wHour: h as u16,
        wMinute: mi as u16,
        wSecond: s as u16,
        ..Default::default()
    };
    let mut local = SystemTime::default();
    // SAFETY: both structures have the documented layout and outlive the
    // call; a null zone pointer is documented to mean the active zone.
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &universal, &mut local) };
    if ok == 0 {
        // Fall back to the zone's current bias, the pre-0.42.0 answer.
        const INVALID: u32 = 0xFFFF_FFFF;
        const DAYLIGHT: u32 = 2;
        let mut info: TimeZoneInformation = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a live, zeroed structure of exactly the layout the
        // API documents, and it outlives the call.
        let which = unsafe { GetTimeZoneInformation(&mut info) };
        if which == INVALID {
            return None;
        }
        let bias = info.Bias
            + if which == DAYLIGHT {
                info.DaylightBias
            } else {
                info.StandardBias
            };
        return Some(-bias * 60);
    }
    let local_secs = days_from_civil(
        i64::from(local.wYear),
        u32::from(local.wMonth),
        u32::from(local.wDay),
    ) * 86_400
        + i64::from(local.wHour) * 3600
        + i64::from(local.wMinute) * 60
        + i64::from(local.wSecond);
    i32::try_from(local_secs - at).ok()
}

/// Days since the epoch for a civil date, Hinnant's `days_from_civil`.
#[cfg(windows)]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(not(any(unix, windows)))]
fn platform_offset(_at: i64) -> Option<i32> {
    None
}

/// `2026-09-17 19:00:18 +05:30` — one timestamp, saying which clock it is on.
///
/// Written out rather than formatted by a date library, because the whole of
/// what is needed is a civil calendar and an offset, and the one thing that
/// must not vary is what the two surfaces print.
pub fn local_stamp(at: i64) -> String {
    let offset = offset_at(at);
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
    let offset = offset_at(at);
    let (_, _, _, h, mi, s) = civil_from_unix(at + offset as i64);
    let sign = if offset < 0 { '-' } else { '+' };
    let off = offset.abs();
    format!(
        "{h:02}:{mi:02}:{s:02} {sign}{:02}:{:02}",
        off / 3600,
        (off % 3600) / 60
    )
}

/// A row's time for a dense table: the clock alone for a row from today,
/// the date as well for anything older. `semlith ledger` printed the clock
/// alone for rows of any age, so a row from last week read as this morning.
pub fn local_when(at: i64, now: i64) -> String {
    let day = |t: i64| {
        let (y, mo, d, ..) = civil_from_unix(t + offset_at(t) as i64);
        (y, mo, d)
    };
    if day(at) == day(now) {
        local_clock(at)
    } else {
        local_stamp(at)
    }
}

/// `2026-10-01T09:14:03Z`: unix seconds as RFC 3339 in UTC, the form the
/// cloud's API takes.
pub fn utc_rfc3339(at: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(at);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
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

    /// A row is printed with the offset in force when it was written, not
    /// today's: across a daylight-saving change the two differ by an hour,
    /// and every row from the other season was an hour off.
    #[cfg(unix)]
    #[test]
    fn a_row_carries_the_offset_of_its_own_day() {
        // SAFETY: the only test in this crate that touches TZ, and it restores
        // nothing other tests read; the libc call re-reads the variable.
        unsafe {
            std::env::set_var("TZ", "America/New_York");
            unsafe extern "C" {
                fn tzset();
            }
            tzset();
        }
        // 2026-01-15 12:00 UTC is EST, 2026-07-15 12:00 UTC is EDT.
        assert_eq!(local_stamp(1_768_478_400), "2026-01-15 07:00:00 -05:00");
        assert_eq!(local_stamp(1_784_116_800), "2026-07-15 08:00:00 -04:00");
        // Today's rows print a time; older ones print their date as well.
        let now = 1_784_116_800 + 3_600;
        assert_eq!(local_when(1_784_116_800, now), "08:00:00 -04:00");
        assert_eq!(local_when(1_768_478_400, now), "2026-01-15 07:00:00 -05:00");
    }
}
