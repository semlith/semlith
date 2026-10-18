//! The remote lane as a program that embeds the library sees it: absent until
//! a [`semlith::accel::Remote`] is registered, then listed, switchable, and
//! started over the channel the registration opens. The worker here is this
//! machine's own CPU-backed worker on a pipe, which speaks the same frames a
//! worker on another machine would.

use semlith::accel::{self, Remote, RemoteChannel};
use std::sync::Arc;
use std::sync::mpsc;

/// A remote whose "other machine" is a local worker process on a pipe.
struct Pipe {
    missing: Option<String>,
}

impl Remote for Pipe {
    fn open(&self) -> anyhow::Result<RemoteChannel> {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(accel::worker_command("worker")?)
            // Several of these share one machine's cores in the run test.
            .env(semlith::embed::THREADS_ENV, "2")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        let send = child.stdin.take().unwrap();
        let mut out = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let frame = accel::read_frame(&mut out).map_err(|e| e.to_string());
                let end = frame.is_err();
                if tx.send(frame).is_err() || end {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        });
        Ok(RemoteChannel {
            send: Box::new(send),
            frames,
            device: "a pipe".into(),
        })
    }

    fn missing(&self) -> Option<String> {
        self.missing.clone()
    }

    fn describe(&self) -> serde_json::Value {
        serde_json::json!({ "endpoint": "pipe" })
    }
}

fn remote_row(snapshot: &serde_json::Value) -> Option<serde_json::Value> {
    snapshot["lanes"]
        .as_array()?
        .iter()
        .find(|row| row["lane"] == "remote")
        .cloned()
}

/// The registration is process-wide, so the tests here take turns, each in
/// the same scratch home.
fn alone() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let home = HOME.get_or_init(|| tempfile::tempdir().unwrap());
    // SAFETY: set while holding the turn, before anything in this test reads
    // it; every test sets the same values.
    unsafe {
        std::env::set_var("SEMLITH_HOME", home.path());
        std::env::remove_var(accel::ACCEL_ENV);
    }
    turn
}

#[test]
fn a_registered_remote_is_listed_switched_and_its_absence_hides_the_lane() {
    let _turn = alone();

    assert!(
        remote_row(&accel::snapshot()).is_none(),
        "listed unregistered"
    );
    let refused = accel::set("remote", true).unwrap_err().to_string();
    assert!(
        refused.contains("there is no lane called remote"),
        "{refused}"
    );

    accel::set_remote(Some(Arc::new(Pipe {
        missing: Some("no worker address yet".into()),
    })));
    let row = remote_row(&accel::snapshot()).expect("registered but not listed");
    assert_eq!(row["endpoint"], "pipe", "describe() reaches the row");
    assert_eq!(row["enabled"], false, "off until switched on");
    let refused = accel::set("remote", true).unwrap_err().to_string();
    assert!(refused.contains("no worker address yet"), "{refused}");

    accel::set_remote(Some(Arc::new(Pipe { missing: None })));
    accel::set("remote", true).unwrap();
    assert_eq!(remote_row(&accel::snapshot()).unwrap()["enabled"], true);

    accel::set_remote(None);
    assert!(
        remote_row(&accel::snapshot()).is_none(),
        "listed after removal"
    );
}

fn row(snapshot: &serde_json::Value, id: &str) -> Option<serde_json::Value> {
    snapshot["lanes"]
        .as_array()?
        .iter()
        .find(|row| row["lane"] == id)
        .cloned()
}

fn saved() -> serde_json::Value {
    let path = semlith::home::settings_path().unwrap();
    serde_json::from_slice(&std::fs::read(path).unwrap_or_default()).unwrap_or_default()
}

fn pipe() -> Arc<Pipe> {
    Arc::new(Pipe { missing: None })
}

/// Any number of remotes, each a lane of its own: listed, switched and saved
/// under its own id, and gone with its registration (0.44.0).
#[test]
fn any_number_of_remotes_are_lanes_of_their_own() {
    let _turn = alone();
    std::fs::remove_file(semlith::home::settings_path().unwrap()).ok();

    for bad in ["gpu", "remote-", "remote-A", "remote_a", "remotes"] {
        let refused = accel::add_remote(bad, pipe()).unwrap_err().to_string();
        assert!(refused.contains("remote-"), "{bad}: {refused}");
    }
    let refused = accel::set("remote-x", true).unwrap_err().to_string();
    assert!(
        refused.contains("there is no lane called remote-x"),
        "{refused}"
    );

    let ids = ["remote", "remote-a", "remote-b"];
    for id in ids {
        accel::add_remote(id, pipe()).unwrap();
    }
    let snapshot = accel::snapshot();
    for id in ids {
        let row = row(&snapshot, id).unwrap_or_else(|| panic!("{id} not listed"));
        assert_eq!(row["endpoint"], "pipe", "{id}: describe() reaches the row");
        assert_eq!(row["enabled"], false, "{id}: off until switched on");
    }

    accel::set("remote-a", true).unwrap();
    let snapshot = accel::snapshot();
    assert_eq!(row(&snapshot, "remote-a").unwrap()["enabled"], true);
    assert_eq!(row(&snapshot, "remote").unwrap()["enabled"], false);
    assert_eq!(row(&snapshot, "remote-b").unwrap()["enabled"], false);
    assert_eq!(saved()["accelerators"]["remotes"]["remote-a"], true);
    assert!(saved()["accelerators"]["remotes"].get("remote-b").is_none());

    assert!(accel::remove_remote("remote-a"));
    assert!(!accel::remove_remote("remote-a"), "removed twice");
    let snapshot = accel::snapshot();
    assert!(row(&snapshot, "remote-a").is_none(), "listed after removal");
    assert!(row(&snapshot, "remote-b").is_some(), "the others stay");
    let refused = accel::set("remote-a", true).unwrap_err().to_string();
    assert!(
        refused.contains("there is no lane called remote-a"),
        "{refused}"
    );

    accel::add_remote("remote-a", pipe()).unwrap();
    assert_eq!(
        row(&accel::snapshot(), "remote-a").unwrap()["enabled"],
        true,
        "its saved switch comes back with it"
    );

    for id in ids {
        accel::remove_remote(id);
    }
}

/// A settings file from 0.43.0 keeps meaning what it meant, and one with no
/// remote lane switched gains no `remotes` key.
#[test]
fn the_lane_called_remote_keeps_its_saved_field() {
    let _turn = alone();
    let path = semlith::home::settings_path().unwrap();
    std::fs::write(&path, r#"{"accelerators":{"remote":true}}"#).unwrap();
    accel::set_remote(Some(pipe()));
    assert_eq!(remote_row(&accel::snapshot()).unwrap()["enabled"], true);

    accel::set("remote", false).unwrap();
    assert_eq!(saved()["accelerators"]["remote"], false);
    assert!(
        saved()["accelerators"].get("remotes").is_none(),
        "{}",
        saved()
    );
    accel::set_remote(None);
}

/// A folder of `files` reports. A remote lane's first batch is a GPU's
/// (16,384 tokens), so 400 of them make several batches for each of three.
fn corpus(files: usize, word: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for f in 0..files {
        let mut text = format!("# {word} report {f}\n\n");
        for p in 0..12 {
            text.push_str(&format!(
                "Paragraph {p} of {word} report {f} covers supply lines, tariffs and the \
                 weather over the harbour, at length, for the record.\n\n"
            ));
        }
        std::fs::write(dir.path().join(format!("{word}_{f:03}.md")), text).unwrap();
    }
    dir
}

/// Every chunk of a finished run has a sound vector from some lane.
fn every_chunk_embedded(s: &semlith::Semlith, report: &semlith::IndexReport) {
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.remaining, 0, "chunks left unembedded");
    assert_eq!(
        report.lanes.values().sum::<usize>(),
        report.chunks,
        "lanes {:?} for {} chunks",
        report.lanes,
        report.chunks
    );
    assert_eq!(
        s.variants().values().sum::<u64>() as usize,
        report.chunks,
        "{:?}",
        s.variants()
    );
}

/// An index run spreads its batches over every remote lane that is on, and
/// one removed while the run goes leaves no chunk behind (0.44.0).
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_run_spreads_over_every_remote_lane_and_survives_one_removed() {
    let _turn = alone();
    let ids = ["remote", "remote-a", "remote-b"];
    for id in ids {
        accel::add_remote(id, pipe()).unwrap();
    }
    accel::manage();
    // SAFETY: set while holding the turn; read by this test's runs only.
    // The deadline is for a GPU's batch; three CPU workers on one machine
    // take longer over the same budget.
    unsafe {
        std::env::set_var(accel::ACCEL_ENV, ids.join(","));
        std::env::set_var("SEMLITH_ACCEL_DEADLINE_MS", "300000");
    }

    let folder = corpus(400, "first");
    let store = tempfile::tempdir().unwrap();
    let mut s = semlith::Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let report = s
        .index_paths(&[folder.path().to_path_buf()], |_, _| {})
        .unwrap();
    every_chunk_embedded(&s, &report);
    for id in ids {
        assert!(
            report.lanes.get(id).is_some_and(|n| *n > 0),
            "{id} took no batch: {:?}\n{}",
            report.lanes,
            accel::snapshot()["lanes"]
        );
    }

    // Again on new text, and remote-b goes once it has embedded something.
    let folder = corpus(400, "second");
    let store = tempfile::tempdir().unwrap();
    let mut s = semlith::Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let mut removed = false;
    let report = s
        .index_paths(&[folder.path().to_path_buf()], |_, p| {
            if !removed && p.lanes.get("remote-b").is_some_and(|n| *n > 0) {
                removed = accel::remove_remote("remote-b");
            }
        })
        .unwrap();
    assert!(removed, "remote-b never embedded before the run ended");
    every_chunk_embedded(&s, &report);

    unsafe {
        std::env::remove_var(accel::ACCEL_ENV);
        std::env::remove_var("SEMLITH_ACCEL_DEADLINE_MS");
    }
    for id in ids {
        accel::remove_remote(id);
    }
}

/// The lane starts over the registered channel and passes the known-answer
/// check through it, as a worker on another machine would.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_remote_lane_starts_over_the_registered_channel() {
    let _turn = alone();
    accel::set_remote(Some(Arc::new(Pipe { missing: None })));
    let hello = accel::check_lane("remote").unwrap();
    assert_eq!(hello["ok"], true, "{hello}");
    accel::set_remote(None);
}
