//! `/api/drop/resolve`: the real path of something dropped on the portal.
//!
//! No browser hands a page the path of a dropped file or folder, on any
//! operating system — only its name, size and `lastModified`, and for a folder
//! what is inside it. The page sends that fingerprint here and the daemon finds
//! the path, in tiers, first that answers:
//!
//! 1. **pasteboard** (macOS): Finder writes the dragged items to the drag
//!    pasteboard, readable from another process with no prompt. Believed only
//!    when its change count moved since the page's last call and every dropped
//!    name and kind is on it, and nothing else is.
//! 2. **explorer** (Windows): the selection of each open Explorer window, the
//!    same rule per window. Untested on a real Windows machine (research note
//!    of 2026-10-01).
//! 3. **index**: Spotlight, Windows Search, `plocate`/`locate`, GNOME
//!    localsearch or KDE Baloo, whichever is there.
//! 4. **roots**: every store's roots walked, and one level of each root's
//!    parent — a folder beside one already indexed.
//! 5. **walk**: the home directory, breadth first, until the budget runs out.
//!
//! Every candidate past the first two tiers is filtered by the fingerprint.
//! One left is `resolved`, several `ambiguous`, none `none`. A path under a
//! temporary directory — an archive opened in place, a file promise — is
//! `refused` with "extract first": indexing a copy that vanishes is worse than
//! asking. Nothing is uploaded, and no path is logged.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The whole call stays under the 2.5 s the page waits for; this is what the
/// tiers share, the external commands included.
pub const BUDGET: Duration = Duration::from_millis(2000);

/// How many first-level names a page may send for a folder.
pub const MAX_CHILDREN: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Dir,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChildFile {
    pub name: String,
    pub size: u64,
    /// Milliseconds since the epoch, as `File.lastModified` gives it.
    pub mtime: i64,
}

/// One dropped item, as the page saw it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Item {
    pub name: String,
    pub kind: Kind,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub mtime: Option<i64>,
    #[serde(default)]
    pub children: Option<Vec<String>>,
    #[serde(default)]
    pub child_files: Option<Vec<ChildFile>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Request {
    pub items: Vec<Item>,
    /// The pasteboard change count the page last saw, from the previous answer.
    #[serde(default)]
    pub pasteboard_change: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Outcome {
    pub name: String,
    /// `resolved`, `ambiguous`, `none` or `refused`.
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<String>>,
    /// The tier that answered; `walk` for `none`, the last one tried.
    pub tier: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Answer {
    pub results: Vec<Outcome>,
    pub os_copy_hint: &'static str,
    /// The drag pasteboard's change count now, for the page to send back as
    /// `pasteboard_change` next time. Absent off macOS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pasteboard_change: Option<i64>,
}

/// An OS index asked for one exact name before a deadline.
pub type IndexFn = dyn Fn(&str, Instant) -> Vec<PathBuf>;

/// What the drag pasteboard held when it was read.
#[derive(Debug, Clone)]
pub struct Pasteboard {
    pub change: i64,
    pub paths: Vec<PathBuf>,
}

/// Everything the resolver reads from the machine, so a test can hand it a
/// fake pasteboard, fake Explorer windows and a temporary tree on any OS.
pub struct Sources {
    pub pasteboard: Box<dyn Fn() -> Option<Pasteboard>>,
    /// The selection of each open Explorer window, one list per window.
    pub explorer: Box<dyn Fn() -> Vec<Vec<PathBuf>>>,
    /// The OS index, asked for one exact name before a deadline.
    pub index: Box<IndexFn>,
    pub roots: Vec<PathBuf>,
    pub home: Option<PathBuf>,
    /// Mounted volumes an index answer may come from besides home and roots.
    pub volumes: Vec<PathBuf>,
    /// Directories whose contents are copies that go away.
    pub temp: Vec<PathBuf>,
    pub budget: Duration,
}

impl Sources {
    /// This machine's sources, with the open stores' roots.
    pub fn system(roots: Vec<PathBuf>) -> Sources {
        let mut temp = vec![std::env::temp_dir()];
        if cfg!(unix) {
            temp.extend(
                [
                    "/tmp",
                    "/private/tmp",
                    "/var/folders",
                    "/private/var/folders",
                ]
                .map(PathBuf::from),
            );
        }
        let volumes = if cfg!(target_os = "macos") {
            vec![PathBuf::from("/Volumes")]
        } else if cfg!(windows) {
            Vec::new()
        } else {
            ["/media", "/run/media", "/mnt"].map(PathBuf::from).to_vec()
        };
        // Test-only: `SEMLITH_DROP_TIERS=walk` leaves the pasteboard, Explorer
        // and the OS index out, so a test of the walk and the temp rule does
        // not spend its budget waiting on a cold runner's Explorer or search
        // service. Unset, every tier runs.
        let walk_only = std::env::var("SEMLITH_DROP_TIERS").is_ok_and(|v| v == "walk");
        if walk_only {
            return Sources {
                pasteboard: Box::new(|| None),
                explorer: Box::new(Vec::new),
                index: Box::new(|_, _| Vec::new()),
                roots,
                home: crate::home::user_home().ok(),
                volumes,
                temp,
                budget: BUDGET,
            };
        }
        Sources {
            pasteboard: Box::new(|| {
                if cfg!(target_os = "macos") {
                    read_drag_pasteboard()
                } else {
                    None
                }
            }),
            explorer: Box::new(|| {
                if cfg!(windows) {
                    explorer_selections()
                } else {
                    Vec::new()
                }
            }),
            index: Box::new(os_index),
            roots,
            home: crate::home::user_home().ok(),
            volumes,
            temp,
            budget: BUDGET,
        }
    }
}

/// The shortcut that copies a selected item's path in this OS's file manager,
/// for the page's "paste its path" fallback.
pub fn os_copy_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌥⌘C"
    } else if cfg!(windows) {
        "Ctrl+Shift+C"
    } else {
        "Ctrl+L, Ctrl+C"
    }
}

/// Resolve every dropped item. `last_change` is the pasteboard change count
/// this daemon saw on its previous call, the baseline when the page sent none.
pub fn resolve(request: &Request, sources: &Sources, last_change: Option<i64>) -> Answer {
    let deadline = Instant::now() + sources.budget;
    let canon = |p: &PathBuf| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
    let temp: Vec<PathBuf> = sources.temp.iter().map(canon).collect();
    let roots: Vec<PathBuf> = sources.roots.iter().map(canon).collect();
    let home = sources.home.as_ref().map(canon);
    let mut allowed: Vec<PathBuf> = sources.volumes.iter().map(canon).collect();
    allowed.extend(roots.iter().cloned());
    allowed.extend(home.iter().cloned());

    let items = &request.items;
    let pasteboard = (sources.pasteboard)();
    let pasteboard_change = pasteboard.as_ref().map(|p| p.change);
    let mut answer = Answer {
        results: Vec::new(),
        os_copy_hint: os_copy_hint(),
        pasteboard_change,
    };
    if items.is_empty() {
        return answer;
    }

    // The exact tiers answer for the whole drop or not at all.
    let baseline = request.pasteboard_change.or(last_change);
    if let Some(board) = pasteboard.filter(|b| baseline != Some(b.change))
        && let Some(paths) = matches_all(items, &board.paths)
    {
        answer.results = conclude_exact(items, paths, "pasteboard", &temp);
        return answer;
    }
    for window in (sources.explorer)() {
        if let Some(paths) = matches_all(items, &window) {
            answer.results = conclude_exact(items, paths, "explorer", &temp);
            return answer;
        }
    }

    let mut found: Vec<Option<(&'static str, Vec<PathBuf>)>> = vec![None; items.len()];

    for (slot, item) in found.iter_mut().zip(items) {
        if Instant::now() >= deadline {
            break;
        }
        let hits: Vec<PathBuf> = (sources.index)(&item.name, deadline)
            .into_iter()
            .filter(|p| {
                let c = canon(p);
                allowed.iter().any(|a| c.starts_with(a))
            })
            .filter(|p| fits(item, p))
            .collect();
        if !hits.is_empty() {
            *slot = Some(("index", hits));
        }
    }

    let mut starts: Vec<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), usize::MAX)).collect();
    // One level of each parent: a folder beside one already indexed. Never the
    // whole parent, which is often the home directory itself.
    for root in &roots {
        if let Some(parent) = root.parent() {
            starts.push((parent.to_path_buf(), 1));
        }
    }
    walk_tier(items, &mut found, &starts, "roots", deadline);
    if let Some(home) = &home {
        walk_tier(
            items,
            &mut found,
            &[(home.clone(), usize::MAX)],
            "walk",
            deadline,
        );
    }

    answer.results = items
        .iter()
        .zip(found)
        .map(|(item, hit)| match hit {
            Some((tier, paths)) => conclude(item, paths, tier, &temp),
            None => Outcome {
                name: item.name.clone(),
                status: "none",
                path: None,
                candidates: None,
                tier: "walk",
                reason: None,
            },
        })
        .collect();
    answer
}

/// The paths of a pasteboard or a window, paired with the items, when every
/// item is there by name and kind and nothing else is. The fingerprint is
/// checked too: a stale pasteboard holding a same-named file is not this drop.
fn matches_all(items: &[Item], paths: &[PathBuf]) -> Option<Vec<PathBuf>> {
    if paths.len() != items.len() {
        return None;
    }
    let mut left: Vec<&PathBuf> = paths.iter().collect();
    let mut paired = Vec::with_capacity(items.len());
    for item in items {
        let at = left.iter().position(|p| fits(item, p))?;
        paired.push(left.remove(at).clone());
    }
    Some(paired)
}

fn conclude_exact(
    items: &[Item],
    paths: Vec<PathBuf>,
    tier: &'static str,
    temp: &[PathBuf],
) -> Vec<Outcome> {
    items
        .iter()
        .zip(paths)
        .map(|(item, path)| conclude(item, vec![path], tier, temp))
        .collect()
}

/// One item's candidates to an outcome: deduplicated by where they really
/// are, temporary copies set aside, then counted.
fn conclude(item: &Item, paths: Vec<PathBuf>, tier: &'static str, temp: &[PathBuf]) -> Outcome {
    let mut seen = HashSet::new();
    let (mut kept, mut temporary) = (Vec::new(), 0);
    for path in paths {
        let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !seen.insert(real.clone()) {
            continue;
        }
        if temp.iter().any(|t| real.starts_with(t)) {
            temporary += 1;
        } else {
            // As found, so a symlink stays the path the person dragged.
            kept.push(crate::plain(&path.display().to_string()));
        }
    }
    let mut outcome = Outcome {
        name: item.name.clone(),
        status: "none",
        path: None,
        candidates: None,
        tier,
        reason: None,
    };
    match kept.len() {
        0 if temporary > 0 => {
            outcome.status = "refused";
            outcome.reason = Some("extract first".into());
        }
        0 => {}
        1 => {
            outcome.status = "resolved";
            outcome.path = kept.pop();
        }
        _ => {
            kept.sort();
            outcome.status = "ambiguous";
            outcome.candidates = Some(kept);
        }
    }
    outcome
}

/// Walk `starts` for every item no earlier tier answered, and record what fits.
fn walk_tier(
    items: &[Item],
    found: &mut [Option<(&'static str, Vec<PathBuf>)>],
    starts: &[(PathBuf, usize)],
    tier: &'static str,
    deadline: Instant,
) {
    let names: HashSet<&str> = items
        .iter()
        .zip(found.iter())
        .filter(|(_, f)| f.is_none())
        .map(|(i, _)| i.name.as_str())
        .collect();
    if names.is_empty() || starts.is_empty() {
        return;
    }
    let by_name = walk(starts, &names, deadline);
    for (item, slot) in items.iter().zip(found.iter_mut()) {
        if slot.is_some() {
            continue;
        }
        let hits: Vec<PathBuf> = by_name
            .get(item.name.as_str())
            .into_iter()
            .flatten()
            .filter(|p| fits(item, p))
            .cloned()
            .collect();
        if !hits.is_empty() {
            *slot = Some((tier, hits));
        }
    }
}

/// Directories a person never drags from and that are vast: build output,
/// dependency trees, the macOS and Windows application-data folders.
const SKIPPED: &[&str] = &[
    "node_modules",
    "target",
    "Library",
    "AppData",
    "$RECYCLE.BIN",
];

/// Breadth first, so a shallow match is found before the deadline cuts a deep
/// one off. Symlinks are not followed. Each start carries how many levels
/// below it may be read.
///
/// ponytail: one thread and a 2 s cap; a home of ~55k directories takes ~11 s
/// here (research note), so a deep match in a big home is found only by the
/// index tier. A persistent directory cache is the upgrade if that bites.
fn walk<'a>(
    starts: &[(PathBuf, usize)],
    names: &HashSet<&'a str>,
    deadline: Instant,
) -> HashMap<&'a str, Vec<PathBuf>> {
    let mut out: HashMap<&str, Vec<PathBuf>> = HashMap::new();
    let mut queue: VecDeque<(PathBuf, usize)> = starts.iter().cloned().collect();
    let mut visited = HashSet::new();
    while let Some((dir, depth)) = queue.pop_front() {
        if Instant::now() >= deadline {
            break;
        }
        if depth == 0 || !visited.insert(dir.clone()) {
            continue;
        }
        let Ok(listing) = std::fs::read_dir(&dir) else {
            continue;
        };
        for (n, entry) in listing.flatten().enumerate() {
            // A directory of tens of thousands of entries would otherwise run
            // the whole listing past the deadline.
            if n % 128 == 127 && Instant::now() >= deadline {
                return out;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(&wanted) = names.get(name) {
                out.entry(wanted).or_default().push(entry.path());
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir && depth > 1 && !name.starts_with('.') && !SKIPPED.contains(&name) {
                queue.push_back((entry.path(), depth - 1));
            }
        }
    }
    out
}

/// Whether `path` is the item the page described: name, kind, and whatever
/// of the fingerprint the page sent.
fn fits(item: &Item, path: &Path) -> bool {
    if path.file_name().and_then(|n| n.to_str()) != Some(item.name.as_str()) {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    match item.kind {
        Kind::File => {
            meta.is_file()
                && item.size.is_none_or(|s| s == meta.len())
                && item.mtime.is_none_or(|m| same_ms(m, &meta))
        }
        Kind::Dir => {
            if !meta.is_dir() {
                return false;
            }
            if let Some(children) = &item.children {
                let Ok(listing) = std::fs::read_dir(path) else {
                    return false;
                };
                let here: HashSet<String> = listing
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                // A subset rather than equality: a browser may leave out what
                // the OS hides, and the page caps the list.
                if !children.iter().take(MAX_CHILDREN).all(|c| here.contains(c)) {
                    return false;
                }
            }
            item.child_files.iter().flatten().take(20).all(|child| {
                std::fs::metadata(path.join(&child.name))
                    .is_ok_and(|m| m.is_file() && m.len() == child.size && same_ms(child.mtime, &m))
            })
        }
    }
}

/// `File.lastModified` is the stat time floored to a millisecond; one either
/// side absorbs a rounding that went the other way.
fn same_ms(ms: i64, meta: &std::fs::Metadata) -> bool {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .is_some_and(|d| (d.as_millis() as i64 - ms).abs() <= 1)
}

/// Run a command until it exits or the deadline passes, and return its stdout.
/// Read on a thread, so a long answer cannot fill the pipe and stall it.
fn run_until(command: &mut Command, deadline: Instant) -> Option<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: the daemon has no console, and PowerShell would
        // otherwise flash one up on every drop.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut text);
        text
    });
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    reader.join().ok()
}

/// The drag pasteboard through JXA: 0.2 s, no prompt, and no AppKit linked
/// into the binary for three calls. The first line is the change count, every
/// other line a file URL's path (file-reference URLs resolved).
pub fn read_drag_pasteboard() -> Option<Pasteboard> {
    const SCRIPT: &str = r#"ObjC.import('AppKit');
function run() {
  var pb = $.NSPasteboard.pasteboardWithName($.NSPasteboardNameDrag);
  var out = [String(pb.changeCount)];
  var urls = pb.readObjectsForClassesOptions($([$.NSURL]), $({}));
  if (urls) for (var i = 0; i < urls.count; i++) {
    var u = urls.objectAtIndex(i);
    if (u.isFileURL) out.push(ObjC.unwrap(u.filePathURL.path));
  }
  return out.join('\n');
}"#;
    let text = run_until(
        Command::new("osascript").args(["-l", "JavaScript", "-e", SCRIPT]),
        Instant::now() + Duration::from_secs(1),
    )?;
    let mut lines = text.lines();
    let change = lines.next()?.trim().parse().ok()?;
    let paths = lines.filter(|l| !l.is_empty()).map(PathBuf::from).collect();
    Some(Pasteboard { change, paths })
}

/// Each open Explorer window's selection, windows separated by a blank line.
fn explorer_selections() -> Vec<Vec<PathBuf>> {
    const SCRIPT: &str = "$ErrorActionPreference='SilentlyContinue'; \
        foreach ($w in (New-Object -ComObject Shell.Application).Windows()) { \
        foreach ($i in $w.Document.SelectedItems()) { $i.Path }; '' }";
    let Some(text) = run_until(
        Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT]),
        Instant::now() + Duration::from_millis(1500),
    ) else {
        return Vec::new();
    };
    text.replace("\r\n", "\n")
        .split("\n\n")
        .map(|block| {
            block
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .filter(|w: &Vec<PathBuf>| !w.is_empty())
        .collect()
}

/// Ask whichever OS index this machine has for an exact name. Every answer is
/// filtered by basename afterwards, so a tool that matches loosely costs time,
/// not correctness.
fn os_index(name: &str, deadline: Instant) -> Vec<PathBuf> {
    let mut commands: Vec<Command> = Vec::new();
    if cfg!(target_os = "macos") {
        // `-name` rather than a query string, so nothing in a name is parsed as
        // Spotlight's query language.
        let mut c = Command::new("mdfind");
        c.args(["-name", name]);
        commands.push(c);
    } else if cfg!(windows) {
        // The name travels in the environment, never inside the script text.
        let mut c = Command::new("powershell");
        c.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference='SilentlyContinue'; \
             $n = $env:SEMLITH_DROP_NAME -replace \"'\",\"''\"; \
             $c = New-Object -ComObject ADODB.Connection; \
             $c.Open(\"Provider=Search.CollatorDSO;Extended Properties='Application=Windows';\"); \
             $r = $c.Execute(\"SELECT System.ItemPathDisplay FROM SYSTEMINDEX WHERE System.FileName = '$n'\"); \
             while (-not $r.EOF) { $r.Fields.Item(0).Value; $r.MoveNext() }",
        ])
        .env("SEMLITH_DROP_NAME", name);
        commands.push(c);
    } else {
        let exact = format!("\\{name}");
        for tool in ["plocate", "locate"] {
            let mut c = Command::new(tool);
            c.args(["-b", &exact]);
            commands.push(c);
        }
        let mut c = Command::new("localsearch");
        c.args(["search", "--disable-color", "--files", name]);
        commands.push(c);
        let mut c = Command::new("baloosearch");
        c.arg(name);
        commands.push(c);
    }

    for mut command in commands {
        if Instant::now() >= deadline {
            break;
        }
        let Some(text) = run_until(&mut command, deadline) else {
            continue;
        };
        let hits: Vec<PathBuf> = text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let line = line
                    .strip_prefix("file://")
                    .map(percent_decode)
                    .unwrap_or_else(|| line.to_string());
                let path = PathBuf::from(line);
                (path.is_absolute() && path.file_name().and_then(|n| n.to_str()) == Some(name))
                    .then_some(path)
            })
            .collect();
        if !hits.is_empty() {
            return hits;
        }
    }
    Vec::new()
}

/// `%20` and friends in a `file://` URI, as localsearch prints them.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = raw
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_uri_is_decoded() {
        assert_eq!(percent_decode("/home/a%20b/%C3%A9.txt"), "/home/a b/é.txt");
        assert_eq!(percent_decode("/x%2"), "/x%2");
    }
}
