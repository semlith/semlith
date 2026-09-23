//! Schedules: the file, the timer, and the failures that have to be recorded.
//!
//! Offline, and in the default set. Generating a report reads SQLite and never
//! the vector index, so a store created and never indexed is enough to prove a
//! real report comes out — and nothing here downloads a model.
//!
//! Every test names its own paths. `schedules.json` lives under the store home,
//! but `Schedules::load_from`/`save_to` are the primitives underneath that
//! lookup, so no test here has to set a process-wide environment variable and
//! race the rest of the binary.

use semlith::Semlith;
use semlith::fleet::Fleet;
use semlith::schedule::{Schedule, Schedules, fire, generating, tick};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// An empty store, opened so `store.db` exists. No model, no vectors.
fn store_at(dir: &Path) -> Fleet {
    Semlith::open(dir, None).expect("a store opens");
    let mut fleet = Fleet::open(&[dir.to_path_buf()]).expect("a fleet opens");
    fleet.quiet = true;
    fleet
}

/// A schedule due a minute ago, writing markdown into `dir`.
fn due_now(dir: &Path, now: i64) -> Schedule {
    let mut schedule = Schedule::new("health", "markdown", "Sonnet 5", 3600, dir);
    schedule.next_run = Some(now - 60);
    schedule
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn written(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

#[test]
fn the_schedules_file_sits_beside_the_registry_not_inside_it() {
    let schedules = semlith::home::schedules_path().expect("a home");
    let registry = semlith::home::registry_path().expect("a home");
    assert_eq!(
        schedules.parent(),
        registry.parent(),
        "a schedule belongs beside the registry"
    );
    assert_eq!(schedules.file_name().unwrap(), "schedules.json");
    assert_ne!(schedules, registry);
}

#[test]
fn a_schedule_round_trips_through_the_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("schedules.json");

    let mut file = Schedules::default();
    let mut schedule = Schedule::new("savings", "csv", "Opus 5", 86_400, temp.path());
    schedule.window = Some("month".into());
    schedule.stores = vec!["one".into(), "two".into()];
    schedule.enabled = false;
    let id = file.add(schedule.clone()).expect("a valid schedule");
    file.save_to(&path).expect("it saves");

    let back = Schedules::load_from(&path).expect("it loads");
    assert_eq!(back, file, "the file round-trips exactly");
    let stored = &back.schedules[&id];
    assert_eq!(stored.kind, "savings");
    assert_eq!(stored.window.as_deref(), Some("month"));
    assert_eq!(stored.stores, ["one", "two"]);
    assert_eq!(stored.format, "csv");
    assert_eq!(stored.model, "Opus 5");
    assert_eq!(stored.every_seconds, 86_400);
    assert_eq!(stored.dir, temp.path());
    assert!(!stored.enabled);
    // `add` gave it a first run one cadence out rather than firing it at once.
    assert!(stored.next_run.unwrap() > now());

    // And the bytes say what they mean, rather than encoding a cadence as a word.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"every_seconds\": 86400"), "{text}");
}

#[test]
fn a_daemon_with_no_schedules_file_creates_none() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("schedules.json");
    assert!(!path.exists());

    // The pass the daemon's timer runs, with no store open and nothing due.
    let until = tick(&path, None, true, now()).expect("a missing file is not an error");
    assert!(!path.exists(), "a pass over no file must create no file");
    // The ceiling, not a poll interval. It came down from an hour to a minute
    // in 0.27.0 for one reason: `Runner::wake` reaches the timer from inside
    // this process, and `semlith schedule add` is a different process writing
    // the same file with no way to reach that condvar. A minute is what makes
    // the terminal and the page behave the same way, and it is still a sleep
    // rather than a loop — the idle-CPU measurement for this release read
    // 0.12 CPU-seconds over five minutes either way, with three schedules
    // registered and with none.
    assert!(
        until >= Duration::from_secs(60),
        "with nothing scheduled the timer sleeps to a ceiling, not in a poll loop: {until:?}"
    );
}

#[test]
fn a_due_schedule_writes_a_real_report_where_it_was_told_to() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("store");
    let out = temp.path().join("reports");
    std::fs::create_dir_all(&out).unwrap();
    let path = temp.path().join("schedules.json");
    let fleet = store_at(&store);

    let at = now();
    let mut file = Schedules::default();
    file.schedules.insert("s1".into(), due_now(&out, at));
    file.save_to(&path).unwrap();

    tick(&path, Some(&fleet), true, at).expect("the pass runs");

    let files = written(&out);
    assert_eq!(files.len(), 1, "one report, at {}", out.display());
    let name = files[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(name.starts_with("semlith-health-"), "{name}");
    assert!(name.ends_with(".md"), "{name}");

    let body = std::fs::read_to_string(&files[0]).unwrap();
    assert!(body.contains("Index health"), "not a real report: {body}");
    assert!(body.len() > 100, "not a real report: {body}");

    // And the record says what it did.
    let back = Schedules::load_from(&path).unwrap();
    let stored = &back.schedules["s1"];
    assert_eq!(stored.last_run, Some(at));
    assert_eq!(stored.next_run, Some(at + 3600));
    assert_eq!(stored.last_path.as_deref(), Some(files[0].as_path()));
    assert_eq!(stored.last_error, None);
}

#[test]
fn a_destination_that_has_gone_away_is_recorded_not_swallowed() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("store");
    let out = temp.path().join("on-a-volume-that-is-not-mounted");
    let path = temp.path().join("schedules.json");
    let fleet = store_at(&store);

    let at = now();
    let mut file = Schedules::default();
    file.schedules.insert("s1".into(), due_now(&out, at));
    file.save_to(&path).unwrap();

    tick(&path, Some(&fleet), true, at).expect("a bad destination is not a pass failure");

    assert!(
        !out.exists(),
        "a destination that has gone must not be recreated"
    );
    let back = Schedules::load_from(&path).unwrap();
    let stored = &back.schedules["s1"];
    let error = stored
        .last_error
        .as_deref()
        .expect("the failure is on the record");
    assert!(
        error.contains("on-a-volume-that-is-not-mounted"),
        "a page must be able to say which directory: {error}"
    );
    assert_eq!(
        stored.last_path, None,
        "no path is claimed for a run that wrote nothing"
    );
    // The clock still moved, so the schedule tries again when the disk is back.
    assert_eq!(stored.last_run, Some(at));
    assert_eq!(stored.next_run, Some(at + 3600));
}

#[test]
fn a_daemon_with_no_store_open_records_that_too() {
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path().join("reports");
    std::fs::create_dir_all(&out).unwrap();
    let path = temp.path().join("schedules.json");

    let at = now();
    let mut file = Schedules::default();
    file.schedules.insert("s1".into(), due_now(&out, at));
    file.save_to(&path).unwrap();

    tick(&path, None, true, at).expect("the pass runs");

    let back = Schedules::load_from(&path).unwrap();
    let error = back.schedules["s1"]
        .last_error
        .as_deref()
        .expect("recorded");
    assert!(error.contains("no store is open"), "{error}");
    assert!(written(&out).is_empty());
}

#[test]
fn a_half_written_index_is_never_reported_on() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("store");
    let out = temp.path().join("reports");
    std::fs::create_dir_all(&out).unwrap();
    let path = temp.path().join("schedules.json");
    let fleet = store_at(&store);

    let at = now();
    let mut file = Schedules::default();
    file.schedules.insert("s1".into(), due_now(&out, at));
    file.save_to(&path).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();

    // `settled: false` is the daemon saying one of its stores has an index run
    // going. Nothing is generated, and the record is not touched, so the
    // schedule is still due when the run finishes.
    let until = tick(&path, Some(&fleet), false, at).expect("the pass runs");
    assert!(
        until <= Duration::from_secs(60),
        "it comes back soon: {until:?}"
    );
    assert!(written(&out).is_empty(), "no report from a store mid-write");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

    // Settled, it runs.
    tick(&path, Some(&fleet), true, at).expect("the pass runs");
    assert_eq!(written(&out).len(), 1);
}

#[test]
fn two_schedules_due_at_once_do_not_run_concurrently() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("store");
    let out = temp.path().join("reports");
    std::fs::create_dir_all(&out).unwrap();
    let fleet = store_at(&store);

    let at = now();
    let mut one = due_now(&out, at);
    let mut two = due_now(&out, at);

    // The slot every generator takes. Held here, a `fire` on another thread
    // must wait for it — which is what stops a scheduled run and a clicked one
    // from generating at the same moment.
    let slot = generating();
    let handle = std::thread::spawn(move || {
        fire(&mut one, &fleet, at);
        one
    });
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        written(&out).is_empty(),
        "a second generator ran while the slot was held"
    );
    drop(slot);

    let one = handle.join().expect("it finishes once the slot is free");
    assert_eq!(one.last_error, None, "{:?}", one.last_error);
    assert!(one.last_path.is_some());

    // And the runner's own path is a loop on one thread, so the second
    // schedule's report is written after the first one's, never beside it.
    let fleet = store_at(&store);
    fire(&mut two, &fleet, at + 1);
    assert_eq!(written(&out).len(), 2);
    assert!(two.last_run.unwrap() > one.last_run.unwrap());
}

#[test]
fn a_restart_loses_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("store");
    let out = temp.path().join("reports");
    std::fs::create_dir_all(&out).unwrap();
    let path = temp.path().join("schedules.json");
    let fleet = store_at(&store);

    let at = now();
    let mut file = Schedules::default();
    file.schedules.insert("s1".into(), due_now(&out, at));
    // A second one that is not due for a week.
    let mut later = Schedule::new("gaps", "json", "Sonnet 5", 604_800, &out);
    later.next_run = Some(at + 604_800);
    file.schedules.insert("s2".into(), later);
    file.save_to(&path).unwrap();

    tick(&path, Some(&fleet), true, at).expect("the pass runs");
    let after_first = Schedules::load_from(&path).unwrap();

    // The process ends here. Everything the timer knew is on disk.
    drop(file);
    drop(fleet);

    let restarted = Schedules::load_from(&path).expect("it loads again");
    assert_eq!(
        restarted, after_first,
        "a restart reads back exactly what ran"
    );
    assert_eq!(restarted.schedules["s1"].next_run, Some(at + 3600));
    assert_eq!(
        restarted.schedules["s1"].last_path,
        after_first.schedules["s1"].last_path
    );
    assert_eq!(restarted.schedules["s2"].next_run, Some(at + 604_800));
    assert_eq!(restarted.schedules["s2"].last_run, None);

    // And the timer picks them up: nothing is due yet, and the sleep is the
    // distance to the nearer one rather than a fixed interval.
    //
    // Capped at the ceiling: the nearer schedule is an hour out and the timer
    // will not sleep past a minute, so what this asserts is that the record
    // survived, which `due` below says exactly.
    let until = restarted.until_next(at);
    assert_eq!(until, Duration::from_secs(3600), "{until:?}");
    assert_eq!(
        restarted.schedules["s1"].next_run,
        Some(at + 3600),
        "the schedule's own next run is what a restart must preserve, not how \
         long the timer happens to sleep before it looks again"
    );
    assert!(restarted.due(at).is_empty());
    assert_eq!(restarted.due(at + 3600), ["s1"]);
}
