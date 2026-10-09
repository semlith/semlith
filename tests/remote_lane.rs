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
