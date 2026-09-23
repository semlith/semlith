//! Schedules: a report the daemon generates on its own, on a cadence, to a
//! directory somebody named.
//!
//! This is the first state in this product that is persistent, daemon-owned and
//! time-driven at once, so three things are settled here rather than left to the
//! surfaces that will sit on it:
//!
//! - **It is its own file.** `~/.semlith/schedules.json`, beside
//!   `registry.json` and not inside it. A schedules file somebody's editor
//!   truncated must cost them their schedules and nothing else; folded into the
//!   registry it would cost them every store on the machine. The path is
//!   resolved through [`crate::home::schedules_path`], which goes through the
//!   one home lookup like everything else.
//! - **The cadence is an interval, in seconds.** The portal will offer daily,
//!   weekly and monthly as chips, but the file's vocabulary is not those three
//!   words: a chip is a shortcut for a number, so a cadence the chips cannot
//!   spell still round-trips and still says what it means.
//! - **No file is not an error.** A daemon that starts with no schedules file
//!   creates none and behaves exactly as it did before this module existed.
//!   Absence is the normal state, which is why nothing here writes the file
//!   until a schedule has actually been added or has actually run.
//!
//! The runner is [`Runner`]: one thread, asleep on a condition variable until
//! the next schedule is due. There is no poll loop — see [`Runner::spawn`].

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// The shortest cadence a schedule may carry.
///
/// A report reads every open store's database, so a schedule that fired every
/// second would be a background process nobody asked for. A minute is short
/// enough to demonstrate the feature and long enough that it cannot become one.
pub const MIN_EVERY: u64 = 60;

/// How long the runner sleeps when nothing is due sooner.
///
/// [`Runner::wake`] covers a change made inside this process — the portal
/// editing a schedule is one function call away from the timer. It cannot cover
/// `semlith schedule add`, which is a *different process* writing the same file
/// and has no way to reach this condvar. Without a ceiling here, a schedule
/// added from a terminal while the daemon slept on an hour-long timeout would
/// not fire for up to an hour, with nothing saying why.
///
/// So the ceiling is a minute. Once a minute the runner reads a file that is a
/// few hundred bytes and almost always finds nothing to do; that is far below
/// the "within noise" the idle-CPU criterion asks for, and it is the whole of
/// what makes the CLI and the portal behave the same way.
const IDLE: Duration = Duration::from_secs(60);

/// How long the runner waits when something is due but the index is not settled.
const SETTLING: Duration = Duration::from_secs(15);

/// Seconds since the epoch.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `2026-09-17-190018` — the same clock [`crate::clock::local_stamp`] prints,
/// reduced to what a filename may hold.
///
/// Derived from that one stamp rather than computed again, so the time in a
/// report's name and the time inside the report cannot disagree, and so there
/// is no second piece of calendar arithmetic in this crate.
fn file_stamp(at: i64) -> String {
    let stamp = crate::clock::local_stamp(at);
    let (date, rest) = stamp.split_once(' ').unwrap_or((stamp.as_str(), ""));
    let time: String = rest
        .chars()
        .take_while(|c| *c != ' ')
        .filter(char::is_ascii_digit)
        .collect();
    format!("{date}-{time}")
}

/// The file extension one of [`crate::report::ALL_FORMATS`] is written under.
fn extension(format: &str) -> &'static str {
    match format {
        "csv" => "csv",
        "json" => "json",
        "html" => "html",
        crate::report::PDF => "pdf",
        _ => "md",
    }
}

/// One schedule, exactly as the file holds it.
///
/// The first block of fields is what somebody asked for; the second is what the
/// runner recorded about what happened. They are one record rather than two
/// because a page showing a schedule has to show both at once — a cadence with
/// no outcome beside it cannot say whether it is working.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// One of [`crate::report::KINDS`].
    pub kind: String,
    /// One of [`crate::report::WINDOWS`]; absent is `Window::Unset`, which is
    /// what a caller naming no window has always got.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Store labels the report is narrowed to. Empty is every open store, which
    /// is what `semlith report` and `/api/report` mean by no scope.
    #[serde(default)]
    pub stores: Vec<String>,
    /// One of [`crate::report::FORMATS`].
    pub format: String,
    /// The model the savings report prices at — the one builder toggle that
    /// changes a figure in money, which is why an unknown one is refused rather
    /// than defaulted.
    pub model: String,
    /// The cadence, as an interval in seconds.
    ///
    /// Named for what it is rather than for a chip: `every_seconds: 604800` is
    /// a week whether or not any surface offers the word.
    pub every_seconds: u64,
    /// Where the file goes. Absolute — see [`Schedule::check`].
    pub dir: PathBuf,
    /// A schedule that is off keeps its record and its timing and is not run.
    pub enabled: bool,

    /// Unix seconds of the last attempt, successful or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<i64>,
    /// Unix seconds of the next attempt. This is what survives a restart: the
    /// runner reads it off disk and owes nothing to how long the process lived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run: Option<i64>,
    /// The file the last successful run wrote. `None` when the last run failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_path: Option<PathBuf>,
    /// Why the last run wrote nothing, in full.
    ///
    /// The whole point of the field: a destination that has been deleted,
    /// renamed or unmounted is the failure this feature will actually meet, and
    /// silently skipping it is the worst available outcome — a user would see a
    /// schedule that says it is running and a folder that never fills. Written
    /// with the whole error chain, so the message names the directory as well
    /// as the errno.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Schedule {
    /// A new schedule with no history and no window or scope.
    ///
    /// The three toggles that have defaults are set by assigning the fields,
    /// which is what a builder with five controls wants; the five arguments
    /// here are the ones with no sensible default.
    pub fn new(kind: &str, format: &str, model: &str, every_seconds: u64, dir: &Path) -> Self {
        Self {
            kind: kind.to_string(),
            window: None,
            stores: Vec::new(),
            format: format.to_string(),
            model: model.to_string(),
            every_seconds,
            dir: dir.to_path_buf(),
            enabled: true,
            last_run: None,
            next_run: None,
            last_path: None,
            last_error: None,
        }
    }

    /// Refuse a schedule that cannot work, naming what would have worked.
    ///
    /// Run when one is added and again before every firing: a report kind can
    /// be removed by an upgrade, and a schedule written by a newer binary can
    /// land in an older one's home.
    pub fn check(&self) -> Result<()> {
        crate::report::kind_of(&self.kind)?;
        crate::report::check_any_format(&self.format)?;
        crate::report::window_of(self.window.as_deref())?;
        if crate::report::price_named(&self.model).is_none() {
            bail!(
                "no prices for model {:?} — this binary prices {}",
                self.model,
                crate::report::price_names()
            );
        }
        if self.every_seconds < MIN_EVERY {
            bail!(
                "a cadence of {}s is below the floor of {MIN_EVERY}s; give a longer interval",
                self.every_seconds
            );
        }
        // Nothing in this crate falls back to the working directory, and a
        // schedule is the case that would tempt it: the daemon's working
        // directory is wherever `semlith start` was typed, months ago, and a
        // report that landed there would be a file nobody can find.
        if !self.dir.is_absolute() {
            bail!(
                "the destination {} is relative; a schedule is run by the daemon, \
                 which has no working directory of yours — give an absolute path",
                self.dir.display()
            );
        }
        Ok(())
    }

    /// Whether this schedule wants running at `now`.
    pub fn due(&self, now: i64) -> bool {
        self.enabled && self.next_run.is_some_and(|at| at <= now)
    }

    /// Move the clock on from `now`, whatever the outcome was.
    ///
    /// Called on a failure as well as a success, because a destination that
    /// came back deserves the next run rather than a schedule that gave up.
    fn advance(&mut self, now: i64) {
        self.last_run = Some(now);
        self.next_run = Some(now + self.every_seconds as i64);
    }
}

/// `schedules.json` — an id to the schedule it names.
///
/// A map rather than a list so a surface can address one schedule across a
/// restart, and so two concurrent edits of different schedules do not renumber
/// each other's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedules {
    #[serde(default)]
    pub schedules: BTreeMap<String, Schedule>,
}

impl Schedules {
    /// Read `~/.semlith/schedules.json`.
    pub fn load() -> Result<Self> {
        Self::load_from(&crate::home::schedules_path()?)
    }

    /// Write `~/.semlith/schedules.json`.
    pub fn save(&self) -> Result<()> {
        self.save_to(&crate::home::schedules_path()?)
    }

    /// The same, at a path named outright.
    ///
    /// The primitive the two above are wrappers over, so the file format can be
    /// tested without a process-wide environment variable — and so the home is
    /// still looked up in exactly one place.
    pub fn load_from(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            // No file is the normal state, not an error. Nothing is created
            // here: a home that has never been given a schedule stays a home
            // with no schedules file in it.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        // A file that cannot be parsed is an error rather than an empty set,
        // for the reason the registry gives: starting over silently would drop
        // schedules somebody is relying on and say nothing.
        serde_json::from_str(&text).with_context(|| {
            format!(
                "{} is not readable as a schedules file; move it aside to start over",
                path.display()
            )
        })
    }

    /// Write to a named path, through a temporary file and a rename.
    ///
    /// The same shape the registry and the settings use, for the same reason: a
    /// process killed mid-write leaves the previous schedules rather than half
    /// of a new set. The temporary name carries this process's pid so two
    /// semliths saving at once cannot rename each other's half-written bytes
    /// into place.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        crate::home::secure_dir(dir)
            .with_context(|| format!("creating the store home {}", dir.display()))?;
        let temp = path.with_extension(format!("json.{}.new", std::process::id()));
        let body = serde_json::to_string_pretty(self)? + "\n";
        crate::home::write_private(&temp, body.as_bytes())
            .with_context(|| format!("writing {}", temp.display()))?;
        if let Err(e) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(e).with_context(|| format!("writing {}", path.display()));
        }
        Ok(())
    }

    /// Add a schedule, returning the id it was given.
    ///
    /// The first run is one cadence away rather than immediate: adding a weekly
    /// report and having it fire at once is a surprise, and a surface that
    /// wants that has [`fire`].
    pub fn add(&mut self, mut schedule: Schedule) -> Result<String> {
        schedule.check()?;
        // A destination that does not exist *yet* is a typo, and this is the
        // one moment it can be caught while somebody is still looking at it.
        //
        // Here rather than in `check`, which is deliberately about whether the
        // record is valid: the runner calls `check` before every firing, and a
        // directory that has gone away since is the runner's recorded failure
        // rather than a refusal. Adding a schedule that can never run and
        // saying nothing is how a user ends up with a row reading `on` beside
        // a folder that never fills — the same outcome `last_error` exists to
        // prevent, arriving a different way.
        if !schedule.dir.is_dir() {
            bail!(
                "the destination {} is not a directory that exists — create it first, \
                 or name one that is already there",
                schedule.dir.display()
            );
        }
        if schedule.next_run.is_none() {
            schedule.next_run = Some(unix_now() + schedule.every_seconds as i64);
        }
        let id = self.free_id();
        self.schedules.insert(id.clone(), schedule);
        Ok(id)
    }

    /// Drop a schedule. `false` if there was none by that id.
    pub fn remove(&mut self, id: &str) -> bool {
        self.schedules.remove(id).is_some()
    }

    /// The ids of every schedule due at `now`, in id order.
    pub fn due(&self, now: i64) -> Vec<String> {
        self.schedules
            .iter()
            .filter(|(_, s)| s.due(now))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// How long until the earliest enabled schedule is due.
    ///
    /// [`IDLE`] when none is, which is what makes an empty set cost nothing.
    pub fn until_next(&self, now: i64) -> Duration {
        self.schedules
            .values()
            .filter(|s| s.enabled)
            .filter_map(|s| s.next_run)
            .min()
            .map(|at| Duration::from_secs((at - now).max(1) as u64))
            .unwrap_or(IDLE)
    }

    /// How long the runner should sleep before looking again.
    ///
    /// Not the same question as [`Self::until_next`], which is why it is not
    /// the same function. `until_next` says when the next schedule is due, and
    /// a page or a command asking that wants the real answer — a schedule a
    /// week out is a week out. This says how long the timer may safely sleep,
    /// which is that distance capped at [`IDLE`], because `semlith schedule
    /// add` is another process and cannot reach this one's condition variable.
    ///
    /// Folding the cap into `until_next` made it lie about a weekly schedule,
    /// which is exactly the sort of quiet wrongness a name like that invites.
    pub fn sleep_for(&self, now: i64) -> Duration {
        self.until_next(now).min(IDLE)
    }

    /// The lowest `s<n>` this set is not already using.
    fn free_id(&self) -> String {
        (1..)
            .map(|n| format!("s{n}"))
            .find(|id| !self.schedules.contains_key(id))
            .unwrap_or_default()
    }
}

/// The one report-generating slot, process wide.
///
/// **This is how "two schedules never run at once" is guaranteed.** The runner
/// is a single thread walking the due list in a `for` loop, which is already
/// enough for two *scheduled* reports; the mutex is what extends the guarantee
/// to a scheduled run and a clicked one, or to two clicks, once the portal has
/// a Run now button. Whoever asks second waits rather than being refused, so
/// nothing is silently dropped.
static GENERATING: Mutex<()> = Mutex::new(());

/// Take the generating slot, waiting for it if somebody else has it.
///
/// Public because it is part of the contract the portal and the CLI sit on: any
/// surface that generates a scheduled report outside [`fire`] takes this first.
pub fn generating() -> MutexGuard<'static, ()> {
    GENERATING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Generate one schedule's report and record what happened on the record.
///
/// Never returns an error: a schedule that failed is a schedule with
/// `last_error` set, because the caller is a timer with nobody to return to and
/// the whole requirement is that the failure lands somewhere a page can read
/// it. The clock is advanced either way.
///
/// The read path is the portal's own — [`crate::report::generate_over`] over a
/// [`crate::fleet::Fleet`] the caller has already opened and locked, which is
/// exactly what `routes::report` does through `with_fleet`. There is no second
/// report code path and there must not be one.
pub fn fire(schedule: &mut Schedule, fleet: &crate::fleet::Fleet, now: i64) {
    let _slot = generating();
    schedule.advance(now);
    match write_report(schedule, fleet, now) {
        Ok(path) => {
            schedule.last_path = Some(path);
            schedule.last_error = None;
        }
        Err(e) => {
            // `{e:#}` rather than `{e}`: anyhow's plain form prints only the
            // outermost context, and the context is where the directory's name
            // is. A page saying "No such file or directory" and not which one
            // would be the silent failure wearing a different hat.
            schedule.last_path = None;
            schedule.last_error = Some(format!("{e:#}"));
        }
    }
}

/// The work [`fire`] records the outcome of.
fn write_report(schedule: &Schedule, fleet: &crate::fleet::Fleet, now: i64) -> Result<PathBuf> {
    schedule.check()?;
    let window = crate::report::window_of(schedule.window.as_deref())?;
    let report = crate::report::generate_over(
        fleet,
        &schedule.kind,
        &schedule.model,
        window,
        &schedule.stores,
    )?;
    // Bytes, not a string: a scheduled PDF is a file like any other, and
    // `render_bytes` is the one renderer over the same blocks for all five
    // formats. Nothing here knows which of them is text.
    let body = report.render_bytes(&schedule.format)?;
    // Checked, never created. A destination that has gone away — deleted,
    // renamed, on a volume that is not mounted this morning — is a fact about
    // the user's disk that they have to be told; `create_dir_all` here would
    // answer an unmounted volume by writing the report into the empty mount
    // point, which looks like success and loses the file.
    if !schedule.dir.is_dir() {
        bail!(
            "the destination {} is not a directory that exists — it may have been \
             moved, renamed, or be on a volume that is not mounted",
            schedule.dir.display()
        );
    }
    let name = format!(
        "semlith-{}-{}.{}",
        schedule.kind,
        file_stamp(now),
        extension(&schedule.format)
    );
    let path = schedule.dir.join(name);
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// One pass of the timer: run what is due and say how long until the next wake.
///
/// The whole of the runner's behaviour, with the daemon's two facts passed in
/// rather than read out of a `State`, so it can be driven directly.
///
/// - `fleet` is the stores to report over, already opened and locked by the
///   caller. `None` is a daemon with no store open, which is recorded on every
///   due schedule rather than passed over.
/// - `settled` is whether no store has an index run going. When something is
///   due and the index is not settled, nothing is generated and nothing is
///   written: the pass comes back in [`SETTLING`] instead. **This is how "never
///   a report from a half-written index" is guaranteed** — the daemon is the
///   one writer for every store it opened (AGENTS.md), so its own view of its
///   own runs is complete, and a report is only ever taken between them.
///
/// Nothing is written when nothing is due, which is what keeps a home with no
/// schedules file a home with no schedules file.
pub fn tick(
    path: &Path,
    fleet: Option<&crate::fleet::Fleet>,
    settled: bool,
    now: i64,
) -> Result<Duration> {
    let mut file = Schedules::load_from(path)?;
    let due = file.due(now);
    if due.is_empty() {
        return Ok(file.sleep_for(now));
    }
    if !settled {
        return Ok(SETTLING);
    }
    for id in due {
        let Some(schedule) = file.schedules.get_mut(&id) else {
            continue;
        };
        match fleet {
            Some(fleet) => fire(schedule, fleet, now),
            None => {
                // Recorded, not skipped, for the same reason a missing
                // destination is: a schedule that quietly does nothing is
                // indistinguishable from one that is working.
                schedule.advance(now);
                schedule.last_path = None;
                schedule.last_error = Some(
                    "no store is open in this daemon, so there is nothing to report on".into(),
                );
            }
        }
    }
    file.save_to(path)?;
    Ok(file.sleep_for(unix_now()))
}

/// The timer thread's handle: stop it, or tell it the file changed.
///
/// Held on [`crate::daemon::State`] so a route can reach it. Construction and
/// starting are separate calls because the state the thread reads holds the
/// handle: [`Runner::new`] makes the handle, the state takes it, and
/// [`Runner::spawn`] starts the thread over the finished state.
pub struct Runner {
    stop: AtomicBool,
    /// The condition variable's partner, carrying a counter that [`Runner::wake`]
    /// bumps. Without the counter a wake landing between a pass and the sleep
    /// that follows it would be lost, and a schedule added from the portal
    /// would wait out the hour.
    generation: Mutex<u64>,
    wake: Condvar,
}

impl Runner {
    /// A handle for a thread that has not started yet.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            stop: AtomicBool::new(false),
            generation: Mutex::new(0),
            wake: Condvar::new(),
        })
    }

    /// Re-read the schedules file now.
    ///
    /// What a surface calls after it has added, changed or removed a schedule.
    /// Cheap: the thread wakes, reads one small JSON file, and goes back to
    /// sleep until the earliest `next_run`.
    pub fn wake(&self) {
        let mut generation = self.generation.lock().unwrap_or_else(|e| e.into_inner());
        *generation += 1;
        drop(generation);
        self.wake.notify_all();
    }

    /// Stop the thread at its next wake. A run already in flight finishes.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// Start the timer over a built daemon state.
    ///
    /// **This is how "no measurable idle CPU" is guaranteed.** The thread does
    /// not poll. It sleeps in one `Condvar::wait_timeout` whose timeout is the
    /// distance to the earliest `next_run` — an hour when nothing is scheduled,
    /// a week when a weekly report is — and is woken early only by
    /// [`Runner::wake`] or [`Runner::stop`]. Three schedules registered cost
    /// three wakes per cadence, which is why the idle measurement with three is
    /// the same as the idle measurement with none.
    ///
    /// The thread is detached rather than joined: it holds no lock and owns no
    /// store, so there is nothing for shutdown to wait on. The daemon calls
    /// [`Runner::stop`] alongside the watchers so a sleeping one exits promptly.
    pub fn spawn(state: Arc<crate::daemon::State>) {
        std::thread::spawn(move || {
            let runner = Arc::clone(&state.schedules);
            let Ok(path) = crate::home::schedules_path() else {
                // No home means no schedules file to read, and the daemon has
                // already said so about everything else it keeps there.
                return;
            };
            while !runner.stop.load(Ordering::SeqCst) {
                let seen = *runner.generation.lock().unwrap_or_else(|e| e.into_inner());
                let until = runner.pass(&state, &path);
                let generation = runner.generation.lock().unwrap_or_else(|e| e.into_inner());
                if *generation == seen && !runner.stop.load(Ordering::SeqCst) {
                    let _ = runner.wake.wait_timeout(generation, until);
                }
            }
        });
    }

    /// One pass, with the fleet opened and locked the way `/api/report` does.
    fn pass(&self, state: &Arc<crate::daemon::State>, path: &Path) -> Duration {
        // A store with an index run going, or work queued behind one, is a
        // store whose vectors and rows are mid-write. See `tick`.
        let settled = state
            .stores()
            .iter()
            .all(|store| !store.run_live() && store.queue_depth() == 0);
        // The same two steps `routes::with_fleet` takes before it calls the
        // generator: open the fleet if this is the first reader, then hold its
        // mutex for the length of the read.
        if let Err(e) = state.open_fleet() {
            state.say(&format!("schedules: could not open the stores: {e:#}"));
            return SETTLING;
        }
        let fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
        match tick(path, fleet.as_ref(), settled, unix_now()) {
            Ok(until) => until,
            Err(e) => {
                state.say(&format!("schedules: {e:#}"));
                IDLE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The temporary directory rather than `/tmp`: on Windows `/tmp` has no
    // drive, so it is relative, and `Schedules::add` refuses it.
    fn weekly() -> Schedule {
        Schedule::new(
            "health",
            "markdown",
            "Sonnet 5",
            7 * 24 * 60 * 60,
            &std::env::temp_dir(),
        )
    }

    #[test]
    fn a_cadence_is_an_interval_not_a_chip() {
        let mut s = weekly();
        // Not one of the three the portal will offer, and it still round-trips.
        s.every_seconds = 5 * 60 * 60 + 17;
        let back: Schedule = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.every_seconds, 18_017);
    }

    #[test]
    fn a_schedule_that_cannot_work_says_what_would_have() {
        let mut s = weekly();
        s.kind = "team-rollup".into();
        let e = s.check().unwrap_err().to_string();
        assert!(e.contains("savings"), "{e}");

        let mut s = weekly();
        s.model = "GPT-9".into();
        assert!(s.check().unwrap_err().to_string().contains("Sonnet 5"));

        let mut s = weekly();
        s.every_seconds = 5;
        assert!(
            s.check()
                .unwrap_err()
                .to_string()
                .contains("below the floor")
        );

        let mut s = weekly();
        s.dir = PathBuf::from("reports");
        assert!(s.check().unwrap_err().to_string().contains("absolute"));
    }

    #[test]
    fn a_file_stamp_is_the_clock_stamp_without_the_punctuation() {
        let stamp = file_stamp(1_789_653_120);
        assert_eq!(stamp.len(), "2026-09-17-135200".len(), "{stamp}");
        assert!(
            stamp.chars().all(|c| c.is_ascii_digit() || c == '-'),
            "{stamp}"
        );
    }

    #[test]
    fn ids_fill_the_gaps() {
        let mut file = Schedules::default();
        assert_eq!(file.add(weekly()).unwrap(), "s1");
        assert_eq!(file.add(weekly()).unwrap(), "s2");
        assert!(file.remove("s1"));
        assert_eq!(file.add(weekly()).unwrap(), "s1");
        assert!(!file.remove("s9"));
    }

    #[test]
    fn an_empty_set_sleeps_for_the_ceiling() {
        assert_eq!(Schedules::default().until_next(0), IDLE);
        assert_eq!(Schedules::default().sleep_for(0), IDLE);
        let mut file = Schedules::default();
        let mut s = weekly();
        s.next_run = Some(1_000);
        file.schedules.insert("s1".into(), s);
        // The two answer different questions, and this is the case that shows
        // it: the schedule is ten minutes out, and the timer will not sleep
        // past a minute because another process may have written the file.
        assert_eq!(file.until_next(400), Duration::from_secs(600));
        assert_eq!(file.sleep_for(400), IDLE);
        // A disabled schedule is not a wake.
        file.schedules.get_mut("s1").unwrap().enabled = false;
        assert_eq!(file.until_next(400), IDLE);
        assert_eq!(file.sleep_for(400), IDLE);
    }
}
