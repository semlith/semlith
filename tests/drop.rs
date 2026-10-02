//! `/api/drop/resolve`: a dropped name and fingerprint back to a real path.
//!
//! No browser gives a page the path of a dropped file, so the daemon finds it.
//! Every tier is fed through `drop::Sources`, so the pasteboard and Explorer
//! tiers run here on all three operating systems against fake contents, and the
//! index, roots and walk tiers against a temporary tree. Nothing here reads the
//! real pasteboard, Explorer, Spotlight or the user's home.

use semlith::drop::{self, Item, Kind, Pasteboard, Request, Sources};
use std::path::Path;
use std::time::Duration;

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn mtime_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// The fingerprint a page sends for a file it was handed.
fn file_item(path: &Path) -> Item {
    Item {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        kind: Kind::File,
        size: Some(std::fs::metadata(path).unwrap().len()),
        mtime: Some(mtime_ms(path)),
        children: None,
        child_files: None,
    }
}

/// The fingerprint a page sends for a folder: its first-level names.
fn dir_item(path: &Path) -> Item {
    let mut children: Vec<String> = std::fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    children.sort();
    Item {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        kind: Kind::Dir,
        size: None,
        mtime: None,
        children: Some(children),
        child_files: None,
    }
}

/// Sources that find nothing anywhere, over a home of the test's choosing.
fn quiet(home: &Path) -> Sources {
    Sources {
        pasteboard: Box::new(|| None),
        explorer: Box::new(Vec::new),
        index: Box::new(|_, _| Vec::new()),
        roots: Vec::new(),
        home: Some(home.to_path_buf()),
        volumes: Vec::new(),
        temp: Vec::new(),
        budget: Duration::from_secs(2),
    }
}

fn request(items: Vec<Item>) -> Request {
    Request {
        items,
        pasteboard_change: None,
    }
}

fn canon(p: &Path) -> String {
    semlith::plain(&p.display().to_string())
}

#[test]
fn the_pasteboard_answers_when_it_moved_and_every_item_matches() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let a = base.join("deep/a.txt");
    let folder = base.join("deep/proj");
    write(&a, "alpha");
    write(&folder.join("x.rs"), "fn x() {}");

    let mut sources = quiet(Path::new("/nonexistent-home"));
    let (pa, pf) = (a.clone(), folder.clone());
    sources.pasteboard = Box::new(move || {
        Some(Pasteboard {
            change: 8,
            paths: vec![pf.clone(), pa.clone()],
        })
    });

    let mut req = request(vec![file_item(&a), dir_item(&folder)]);
    req.pasteboard_change = Some(7);
    let answer = drop::resolve(&req, &sources, None);

    assert_eq!(answer.pasteboard_change, Some(8));
    assert_eq!(answer.results.len(), 2);
    for (result, path) in answer.results.iter().zip([&a, &folder]) {
        assert_eq!(result.status, "resolved", "{result:?}");
        assert_eq!(result.tier, "pasteboard");
        assert_eq!(result.path.as_deref(), Some(canon(path).as_str()));
    }
}

#[test]
fn a_pasteboard_that_did_not_move_is_stale_and_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let a = base.join("a.txt");
    write(&a, "alpha");

    let mut sources = quiet(&base);
    let pa = a.clone();
    sources.pasteboard = Box::new(move || {
        Some(Pasteboard {
            change: 8,
            paths: vec![pa.clone()],
        })
    });

    // The page's own baseline.
    let mut req = request(vec![file_item(&a)]);
    req.pasteboard_change = Some(8);
    let answer = drop::resolve(&req, &sources, None);
    assert_eq!(answer.results[0].tier, "walk", "{:?}", answer.results[0]);

    // The daemon's remembered baseline, when the page sent none.
    let req = request(vec![file_item(&a)]);
    let answer = drop::resolve(&req, &sources, Some(8));
    assert_eq!(answer.results[0].tier, "walk", "{:?}", answer.results[0]);
}

#[test]
fn a_pasteboard_holding_anything_else_is_not_believed() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let a = base.join("one/a.txt");
    let b = base.join("one/b.txt");
    write(&a, "alpha");
    write(&b, "beta");

    // One extra item on the pasteboard: not this drop.
    let mut sources = quiet(Path::new("/nonexistent-home"));
    let (pa, pb) = (a.clone(), b.clone());
    sources.pasteboard = Box::new(move || {
        Some(Pasteboard {
            change: 2,
            paths: vec![pa.clone(), pb.clone()],
        })
    });
    let answer = drop::resolve(&request(vec![file_item(&a)]), &sources, Some(1));
    assert_eq!(answer.results[0].status, "none", "{:?}", answer.results[0]);

    // The right name in the wrong kind: a file where a folder was dropped.
    let mut folder = dir_item(&base.join("one"));
    folder.name = "a.txt".into();
    let pa = a.clone();
    sources.pasteboard = Box::new(move || {
        Some(Pasteboard {
            change: 3,
            paths: vec![pa.clone()],
        })
    });
    let answer = drop::resolve(&request(vec![folder]), &sources, Some(1));
    assert_eq!(answer.results[0].status, "none", "{:?}", answer.results[0]);
}

#[test]
fn explorer_answers_from_the_one_window_whose_selection_matches() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let a = base.join("w2/report.txt");
    let other = base.join("w1/notes.txt");
    write(&a, "report");
    write(&other, "notes");

    let mut sources = quiet(Path::new("/nonexistent-home"));
    let (pa, po) = (a.clone(), other.clone());
    sources.explorer = Box::new(move || vec![vec![po.clone()], vec![pa.clone()]]);

    let answer = drop::resolve(&request(vec![file_item(&a)]), &sources, None);
    let result = &answer.results[0];
    assert_eq!(result.status, "resolved", "{result:?}");
    assert_eq!(result.tier, "explorer");
    assert_eq!(result.path.as_deref(), Some(canon(&a).as_str()));
}

#[test]
fn the_index_tier_is_filtered_by_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let real = base.join("x/data.csv");
    let copy = base.join("y/data.csv");
    let other = base.join("z/data.csv");
    write(&real, "1,2,3");
    write(&copy, "1,2,3");
    write(&other, "a much longer and different file");

    let mut sources = quiet(&base);
    let candidates = vec![real.clone(), other.clone()];
    sources.index = Box::new(move |name, _| {
        assert_eq!(name, "data.csv");
        candidates.clone()
    });
    let answer = drop::resolve(&request(vec![file_item(&real)]), &sources, None);
    let result = &answer.results[0];
    assert_eq!(result.status, "resolved", "{result:?}");
    assert_eq!(result.tier, "index");
    assert_eq!(result.path.as_deref(), Some(canon(&real).as_str()));

    // Two that fit, after the mtimes are made to agree: a pick, not a guess.
    let when = std::fs::metadata(&real).unwrap().modified().unwrap();
    std::fs::File::options()
        .write(true)
        .open(&copy)
        .unwrap()
        .set_modified(when)
        .unwrap();
    let candidates = vec![real.clone(), copy.clone(), other.clone()];
    sources.index = Box::new(move |_, _| candidates.clone());
    let answer = drop::resolve(&request(vec![file_item(&real)]), &sources, None);
    let result = &answer.results[0];
    assert_eq!(result.status, "ambiguous", "{result:?}");
    assert_eq!(result.tier, "index");
    let mut got = result.candidates.clone().unwrap();
    got.sort();
    let mut want = vec![canon(&real), canon(&copy)];
    want.sort();
    assert_eq!(got, want);
    assert!(result.path.is_none());
}

#[test]
fn an_index_answer_outside_home_volumes_and_roots_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let outside = base.join("elsewhere/a.txt");
    write(&outside, "alpha");
    let home = base.join("home");
    std::fs::create_dir_all(&home).unwrap();

    let mut sources = quiet(&home);
    let p = outside.clone();
    sources.index = Box::new(move |_, _| vec![p.clone()]);
    let answer = drop::resolve(&request(vec![file_item(&outside)]), &sources, None);
    assert_eq!(answer.results[0].status, "none", "{:?}", answer.results[0]);
}

#[test]
fn store_roots_are_walked_and_their_parents_looked_into() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let root = base.join("work/repo");
    let nested = root.join("src/deep/lib.rs");
    let sibling = base.join("work/sibling");
    let far = base.join("work/other/far.txt");
    write(&nested, "fn lib() {}");
    write(&sibling.join("README.md"), "# sibling");
    write(&far, "far");

    let mut sources = quiet(Path::new("/nonexistent-home"));
    sources.roots = vec![root.clone()];

    let answer = drop::resolve(&request(vec![file_item(&nested)]), &sources, None);
    assert_eq!(
        answer.results[0].status, "resolved",
        "{:?}",
        answer.results[0]
    );
    assert_eq!(answer.results[0].tier, "roots");
    assert_eq!(
        answer.results[0].path.as_deref(),
        Some(canon(&nested).as_str())
    );

    // A folder beside the root: one level of the parent.
    let answer = drop::resolve(&request(vec![dir_item(&sibling)]), &sources, None);
    assert_eq!(
        answer.results[0].status, "resolved",
        "{:?}",
        answer.results[0]
    );
    assert_eq!(answer.results[0].tier, "roots");

    // Two levels under the parent is not "beside the root".
    let answer = drop::resolve(&request(vec![file_item(&far)]), &sources, None);
    assert_eq!(answer.results[0].status, "none", "{:?}", answer.results[0]);
}

#[test]
fn the_walk_tells_two_folders_of_one_name_apart_by_their_children() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let home = base.as_path();
    let mine = home.join("code/website");
    let theirs = home.join("old/website");
    write(&mine.join("index.html"), "<p>mine</p>");
    write(&mine.join("astro.config.mjs"), "export default {}");
    write(&theirs.join("index.html"), "<p>theirs</p>");
    // Skipped by the walk: a dot-directory and a build directory.
    write(&home.join(".cache/website/index.html"), "");
    write(&home.join("node_modules/website/index.html"), "");

    let sources = quiet(home);
    let answer = drop::resolve(&request(vec![dir_item(&mine)]), &sources, None);
    let result = &answer.results[0];
    assert_eq!(result.status, "resolved", "{result:?}");
    assert_eq!(result.tier, "walk");
    assert_eq!(result.path.as_deref(), Some(canon(&mine).as_str()));

    // A name only, no children: both are candidates.
    let mut bare = dir_item(&mine);
    bare.children = None;
    let answer = drop::resolve(&request(vec![bare]), &sources, None);
    let result = &answer.results[0];
    assert_eq!(result.status, "ambiguous", "{result:?}");
    assert_eq!(result.candidates.as_ref().unwrap().len(), 2);
}

#[test]
fn child_file_sizes_and_mtimes_narrow_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let home = base.as_path();
    let a = home.join("a/site");
    let b = home.join("b/site");
    write(&a.join("page.html"), "short");
    write(
        &b.join("page.html"),
        "a good deal longer than the other one",
    );

    let mut item = dir_item(&a);
    item.child_files = Some(vec![drop::ChildFile {
        name: "page.html".into(),
        size: 5,
        mtime: mtime_ms(&a.join("page.html")),
    }]);
    let answer = drop::resolve(&request(vec![item]), &quiet(home), None);
    assert_eq!(
        answer.results[0].status, "resolved",
        "{:?}",
        answer.results[0]
    );
    assert_eq!(answer.results[0].path.as_deref(), Some(canon(&a).as_str()));
}

#[test]
fn a_path_under_a_temp_directory_is_refused_until_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let staging = base.join("staging");
    let a = staging.join("Archive/a.txt");
    write(&a, "alpha");

    let mut sources = quiet(Path::new("/nonexistent-home"));
    sources.temp = vec![staging.clone()];
    let pa = a.clone();
    sources.pasteboard = Box::new(move || {
        Some(Pasteboard {
            change: 1,
            paths: vec![pa.clone()],
        })
    });
    let answer = drop::resolve(&request(vec![file_item(&a)]), &sources, None);
    let result = &answer.results[0];
    assert_eq!(result.status, "refused", "{result:?}");
    assert_eq!(result.reason.as_deref(), Some("extract first"));
    assert!(result.path.is_none(), "a refused answer names no path");
}

#[test]
fn nothing_found_is_none_and_the_walk_keeps_to_its_budget() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let home = base.as_path();
    for i in 0..200 {
        write(&home.join(format!("d{i}/e/f/g.txt")), "g");
    }
    let mut sources = quiet(home);
    sources.budget = Duration::ZERO;

    let item = Item {
        name: "never-there.bin".into(),
        kind: Kind::File,
        size: Some(1),
        mtime: None,
        children: None,
        child_files: None,
    };
    let started = std::time::Instant::now();
    let answer = drop::resolve(&request(vec![item]), &sources, None);
    assert!(started.elapsed() < Duration::from_millis(500));
    let result = &answer.results[0];
    assert_eq!(result.status, "none");
    assert_eq!(result.tier, "walk");
    assert!(result.path.is_none() && result.candidates.is_none());
}

#[test]
fn the_answer_has_the_contract_shape() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let a = base.join("a.txt");
    write(&a, "alpha");
    let answer = drop::resolve(&request(vec![file_item(&a)]), &quiet(&base), None);
    let json = serde_json::to_value(&answer).unwrap();

    let hint = json["os_copy_hint"].as_str().unwrap();
    let want = if cfg!(target_os = "macos") {
        "⌥⌘C"
    } else if cfg!(windows) {
        "Ctrl+Shift+C"
    } else {
        "Ctrl+L, Ctrl+C"
    };
    assert_eq!(hint, want);
    let row = &json["results"][0];
    assert_eq!(row["name"], "a.txt");
    assert_eq!(row["status"], "resolved");
    assert_eq!(row["tier"], "walk");
    assert!(row["path"].is_string());
    // Absent rather than null when there is nothing to say.
    assert!(row.get("candidates").is_none() && row.get("reason").is_none());

    // The request parses from the page's JSON, optional fields and all.
    let parsed: Request = serde_json::from_str(
        r#"{"items":[{"name":"p","kind":"dir","children":["a"],
            "child_files":[{"name":"a","size":1,"mtime":2}]},
            {"name":"f","kind":"file","size":3,"mtime":4}],
            "pasteboard_change":9}"#,
    )
    .unwrap();
    assert_eq!(parsed.items.len(), 2);
    assert_eq!(parsed.pasteboard_change, Some(9));
}

/// On Windows every drive is a place a drop may come from.
#[test]
#[cfg(windows)]
fn every_windows_drive_is_a_place_a_drop_may_come_from() {
    // The system drive at least, and only drive roots.
    let drives = drop::windows_drives();
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    assert!(
        drives.iter().any(|d| d
            .to_string_lossy()
            .eq_ignore_ascii_case(&format!("{system}\\"))),
        "{drives:?}"
    );
    assert!(drives.iter().all(|d| d.parent().is_none()), "{drives:?}");
}

/// The real macOS drag pasteboard, read-only. Run by hand after dragging
/// something out of Finder: `cargo test --test drop -- --ignored --nocapture`.
#[test]
#[ignore]
#[cfg(target_os = "macos")]
fn the_real_drag_pasteboard_is_readable() {
    let pasteboard = drop::read_drag_pasteboard().expect("osascript read the drag pasteboard");
    println!(
        "changeCount {} with {} file(s)",
        pasteboard.change,
        pasteboard.paths.len()
    );
    assert!(pasteboard.change >= 0);

    // The index tier for real: this checkout's README by its fingerprint.
    // A checkout under a dot-directory (a worktree) is invisible to both the
    // index and the walk; point `SEMLITH_DROP_PROBE` at any file to try it.
    let readme = std::env::var_os("SEMLITH_DROP_PROBE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"));
    let started = std::time::Instant::now();
    let answer = drop::resolve(
        &Request {
            items: vec![file_item(&readme)],
            pasteboard_change: Some(pasteboard.change),
        },
        &Sources::system(Vec::new()),
        None,
    );
    let row = &answer.results[0];
    println!(
        "probe: {} by {} in {:?}",
        row.status,
        row.tier,
        started.elapsed()
    );

    // The whole resolver on this machine's own sources: the pasteboard's
    // first file by its fingerprint, then the same file with the pasteboard
    // stale, so the index or walk has to find it. Reads only; prints no path.
    let Some(first) = pasteboard.paths.first() else {
        return;
    };
    let item = if first.is_dir() {
        dir_item(first)
    } else {
        file_item(first)
    };
    for baseline in [None, Some(pasteboard.change)] {
        let started = std::time::Instant::now();
        let answer = drop::resolve(
            &Request {
                items: vec![item.clone()],
                pasteboard_change: baseline,
            },
            &Sources::system(Vec::new()),
            None,
        );
        let elapsed = started.elapsed();
        let row = &answer.results[0];
        println!(
            "baseline {baseline:?}: {} by {} in {elapsed:?}",
            row.status, row.tier
        );
        assert!(elapsed < Duration::from_millis(2500));
    }
}

/// A daemon on a scratch home, as `tests/endpoint.rs` starts one.
struct Daemon {
    child: std::process::Child,
    port: u16,
    token: String,
}

impl Daemon {
    fn start(home: &Path) -> Self {
        use std::io::BufRead;
        std::fs::create_dir_all(home).unwrap();
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(["start", "--port", "0"])
            .env("SEMLITH_HOME", home.join(".semlith"))
            .env("HOME", home)
            .env("USERPROFILE", home)
            // The walk and the temp rule only: on a cold Windows runner the
            // Explorer and Windows Search tiers can use the whole budget.
            .env("SEMLITH_DROP_TIERS", "walk")
            .env_remove("SEMLITH_STORE")
            .env_remove("SEMLITH_PORT")
            .current_dir(home)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("semlith start runs");
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .expect("the daemon prints its URL");
        let rest = line
            .trim()
            .strip_prefix("http://127.0.0.1:")
            .expect("a loopback URL");
        let (port, token) = rest.split_once("/?token=").expect("a token in the URL");
        Self {
            child,
            port: port.parse().unwrap(),
            token: token.to_string(),
        }
    }

    fn send(&self, method: &str, path: &str, token: bool, body: &str) -> (u16, serde_json::Value) {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let auth = if token {
            format!("Semlith-Token: {}\r\n", self.token)
        } else {
            String::new()
        };
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{auth}Content-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
        )
        .unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let status = raw
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let json = raw
            .split_once("\r\n\r\n")
            .and_then(|(_, b)| serde_json::from_str(b).ok())
            .unwrap_or(serde_json::Value::Null);
        (status, json)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Both routes behind the session token, answering in the contract's shape.
/// The scratch home is under the system temp directory, so a file found there
/// is refused with "extract first" — the temp rule, live.
#[test]
fn the_routes_are_token_guarded_and_answer_in_shape() {
    let dir = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(dir.path()).unwrap().join("home");
    let daemon = Daemon::start(&home);
    let probe = home.join("semlith-drop-probe.txt");
    write(&probe, "probe");
    let body = serde_json::json!({ "items": [{
        "name": "semlith-drop-probe.txt", "kind": "file",
        "size": 5, "mtime": mtime_ms(&probe),
    }]})
    .to_string();

    let (status, _) = daemon.send("POST", "/api/drop/resolve", false, &body);
    assert_eq!(status, 401, "the drop route answered without the token");

    let (status, json) = daemon.send("POST", "/api/drop/resolve", true, &body);
    assert_eq!(status, 200, "{json}");
    assert_eq!(json["os_copy_hint"], drop::os_copy_hint());
    let row = &json["results"][0];
    assert_eq!(row["name"], "semlith-drop-probe.txt", "{json}");
    assert_eq!(row["status"], "refused", "{json}");
    assert_eq!(row["reason"], "extract first", "{json}");

    let (status, json) = daemon.send("POST", "/api/drop/resolve", true, "{\"items\": 3}");
    assert_eq!(status, 400, "{json}");
}
