//! `semlith files --tree` / `semlith_files {tree: true}`: a directory view that
//! answers more than `ls`, `find` or `tree` can without opening a file (1.10).
//!
//! Each directory says how many files and chunks it holds and in which
//! languages; each file its length in lines, its symbol count and its first
//! definitions; and each directory ends with what is on disk but not in the
//! store, and why — so "not indexed" and "not there" read differently. No
//! engine of its own: the store's own rows, the not-indexed list, and one
//! bounded read of each directory shown.

use crate::fleet::Fleet;
use crate::store;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The most characters one tree answer takes.
pub const TREE_CHARS: usize = 8_000;

/// Entries one directory lists before the rest collapse into counts.
const ENTRIES: usize = 25;

/// How the entries of a directory are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Name,
    Size,
    Symbols,
    /// Newest `indexed_at` first: what changed lately.
    Recent,
}

impl Sort {
    pub fn parse(raw: &str) -> Result<Sort> {
        Ok(match raw {
            "" | "name" => Sort::Name,
            "size" => Sort::Size,
            "symbols" => Sort::Symbols,
            "recent" => Sort::Recent,
            other => anyhow::bail!("sort is name, size, symbols or recent, not {other:?}"),
        })
    }
}

/// Entries one level of the JSON tree lists before the rest are only counted.
/// Generous on purpose: the portal draws a scrolling list, not a text answer
/// with a character budget, and a folder past a thousand entries is rare
/// enough that a count is the honest answer for the remainder.
const LEVEL: usize = 1_000;

struct File {
    name: String,
    /// The path as the store holds it, for the one `stat` that decides
    /// whether the file changed since it was indexed.
    path: String,
    lines: i64,
    chunks: i64,
    indexed_at: i64,
    symbols: usize,
    firsts: Vec<String>,
    /// The size and modification time recorded at indexing, when there are.
    stamp: Option<(i64, i64)>,
}

impl File {
    /// Whether the file on disk differs from what was indexed. Asked only for
    /// a file an answer shows, so a view of one folder does not `stat` every
    /// file in the store.
    fn stale(&self) -> bool {
        self.stamp.is_some_and(|(bytes, at)| {
            std::fs::metadata(&self.path).is_ok_and(|m| {
                m.len() as i64 != bytes
                    || m.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .is_some_and(|d| d.as_secs() as i64 > at)
            })
        })
    }
}

#[derive(Default)]
struct Dir {
    files: Vec<File>,
    dirs: BTreeMap<String, Dir>,
}

impl Dir {
    fn insert(&mut self, dirs: &[String], file: File) {
        let mut node = self;
        for d in dirs {
            node = node.dirs.entry(d.clone()).or_default();
        }
        node.files.push(file);
    }

    /// The directory `dirs` names below this one, if the store holds any file
    /// under it.
    fn walk(&self, dirs: &[String]) -> Option<&Dir> {
        dirs.iter().try_fold(self, |node, d| node.dirs.get(d))
    }

    fn totals(&self) -> (usize, i64, BTreeMap<String, usize>) {
        let mut files = self.files.len();
        let mut chunks: i64 = self.files.iter().map(|f| f.chunks).sum();
        let mut langs: BTreeMap<String, usize> = BTreeMap::new();
        for f in &self.files {
            if let Some(lang) = crate::filter::language_of_path(&f.name) {
                *langs.entry(lang.name.to_string()).or_insert(0) += 1;
            }
        }
        for d in self.dirs.values() {
            let (f, c, l) = d.totals();
            files += f;
            chunks += c;
            for (k, v) in l {
                *langs.entry(k).or_insert(0) += v;
            }
        }
        (files, chunks, langs)
    }

    fn newest(&self) -> i64 {
        self.files
            .iter()
            .map(|f| f.indexed_at)
            .chain(self.dirs.values().map(Dir::newest))
            .max()
            .unwrap_or(0)
    }
}

/// One indexed file, placed: the store it came from, the root that holds it
/// and its path below that root.
struct Held {
    label: String,
    root: Option<PathBuf>,
    parts: Vec<String>,
    file: File,
}

/// Every file the chosen stores hold under the filter, and every refusal
/// they recorded, keyed by plain path. The one query path both the text
/// tree and the JSON levels are built from.
fn load(
    fleet: &Fleet,
    only: Option<&[String]>,
    filter: &crate::filter::Filter,
    roots: &[PathBuf],
) -> Result<(Vec<Held>, BTreeMap<String, store::Refused>)> {
    let mut held: Vec<Held> = Vec::new();
    let mut refusals: BTreeMap<String, store::Refused> = BTreeMap::new();
    for (label, s) in fleet.chosen_each(only)? {
        let db = s.db();
        for row in store::refusals(db)? {
            refusals.insert(crate::plain(&row.path), row);
        }
        let rows = store::file_rows(db, filter.groups(), store::FileSort::Path, false, i64::MAX)?;
        let paths: Vec<String> = rows.iter().map(|r| r.path.clone()).collect();
        let symbols = store::symbols_in_files(db, &paths)?;
        let stamps = store::file_stamps(db, &paths)?;
        for row in rows {
            let (root, parts) = split_root(&row.path, roots);
            let Some(name) = parts.last().cloned() else {
                continue;
            };
            let defs = symbols.get(&row.path).cloned().unwrap_or_default();
            let stamp = stamps.get(&row.path).copied();
            held.push(Held {
                label: label.to_string(),
                root: root.cloned(),
                parts,
                file: File {
                    name,
                    lines: row.lines,
                    chunks: row.chunks,
                    indexed_at: row.indexed_at,
                    symbols: defs
                        .iter()
                        .filter(|d| !crate::graph::NAVIGATIONAL_KINDS.contains(&d.3.as_str()))
                        .count(),
                    firsts: firsts(&defs),
                    stamp,
                    path: row.path,
                },
            });
        }
    }
    Ok((held, refusals))
}

/// The tree under the filter's paths, `depth` levels deep, as text.
pub fn render(
    fleet: &Fleet,
    only: Option<&[String]>,
    filter: &crate::filter::Filter,
    depth: usize,
    sort: Sort,
) -> Result<String> {
    let roots = fleet.roots();
    let (held, refusals) = load(fleet, only, filter, &roots)?;
    let mut top = Dir::default();
    let mut rels: Vec<Vec<String>> = Vec::new();
    let mut first_path: Option<String> = None;
    for h in held {
        first_path.get_or_insert_with(|| crate::plain(&h.file.path));
        top.insert(&h.parts[..h.parts.len() - 1], h.file);
        rels.push(h.parts);
    }
    if rels.is_empty() {
        return Ok("No file in the semlith store matches that.".to_string());
    }

    // The deepest directory every listed file shares is where the view
    // starts: `path: ["src/**"]` shows `src/`, not the repository around it.
    let mut base: Vec<String> = rels[0][..rels[0].len() - 1].to_vec();
    for r in &rels {
        let shared = base
            .iter()
            .zip(&r[..r.len() - 1])
            .take_while(|(a, b)| a == b)
            .count();
        base.truncate(shared);
    }
    let mut start = &top;
    for d in &base {
        start = &start.dirs[d];
    }
    // The root the first file sits under, which is where the on-disk half
    // of the view reads from.
    let root = first_path.and_then(|first| {
        roots
            .iter()
            .filter(|r| first.starts_with(&crate::plain(&r.to_string_lossy())))
            .max_by_key(|r| r.as_os_str().len())
            .cloned()
    });
    let disk_base = root.map(|r| base.iter().fold(r, |p, d| p.join(d)));
    let ignore = disk_base.as_deref().map(semlithignore_of);

    let mut out = String::new();
    let mut cut = false;
    let label = if base.is_empty() {
        "./".to_string()
    } else {
        format!("{}/", base.join("/"))
    };
    write_dir(
        &mut out,
        &label,
        start,
        disk_base.as_deref(),
        &refusals,
        ignore.as_ref().and_then(|i| i.as_ref()),
        0,
        depth.max(1),
        sort,
        &mut cut,
    );
    if cut {
        out.push_str(&format!(
            "more: the view stopped at {TREE_CHARS} characters; ask with depth: {} or a path under {label}",
            depth.saturating_sub(1).max(1)
        ));
    }
    Ok(out.trim_end().to_string())
}

#[allow(clippy::too_many_arguments)]
fn write_dir(
    out: &mut String,
    label: &str,
    dir: &Dir,
    disk: Option<&Path>,
    refusals: &BTreeMap<String, store::Refused>,
    ignore: Option<&ignore::gitignore::Gitignore>,
    level: usize,
    depth: usize,
    sort: Sort,
    cut: &mut bool,
) {
    if *cut {
        return;
    }
    let indent = "  ".repeat(level);
    let (files, chunks, langs) = dir.totals();
    let mix: Vec<String> = mix(langs).iter().map(|(l, n)| format!("{l} {n}")).collect();
    let header = format!(
        "{indent}{label} · {files} file{} · {chunks} chunks{}\n",
        if files == 1 { "" } else { "s" },
        if mix.is_empty() {
            String::new()
        } else {
            format!(" · {}", mix.join(", "))
        }
    );
    if !push(out, &header, cut) {
        return;
    }
    if level + 1 > depth {
        return;
    }
    let inner = "  ".repeat(level + 1);

    let (subdirs, files) = ordered(dir, sort);

    let mut shown = 0usize;
    for (name, sub) in &subdirs {
        if shown >= ENTRIES {
            break;
        }
        write_dir(
            out,
            &format!("{name}/"),
            sub,
            disk.map(|d| d.join(name.as_str())).as_deref(),
            refusals,
            ignore,
            level + 1,
            depth,
            sort,
            cut,
        );
        shown += 1;
    }
    let mut rest: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        if shown >= ENTRIES {
            let ext = Path::new(&file.name)
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_else(|| "no extension".to_string());
            *rest.entry(ext).or_insert(0) += 1;
            continue;
        }
        let mut line = format!(
            "{inner}{} · {} lines · {} symbol{}",
            file.name,
            file.lines,
            file.symbols,
            if file.symbols == 1 { "" } else { "s" }
        );
        if !file.firsts.is_empty() {
            line.push_str(&format!(" · {}", file.firsts.join(", ")));
            if file.symbols > file.firsts.len() {
                line.push_str(", …");
            }
        }
        if file.stale() {
            line.push_str(" · changed since indexed");
        }
        line.push('\n');
        if !push(out, &line, cut) {
            return;
        }
        shown += 1;
    }
    let hidden_dirs = subdirs.len().saturating_sub(ENTRIES);
    if !rest.is_empty() || hidden_dirs > 0 {
        let mut parts: Vec<String> = rest.iter().map(|(e, n)| format!("{n} {e}")).collect();
        if hidden_dirs > 0 {
            parts.insert(0, format!("{hidden_dirs} folders"));
        }
        if !push(
            out,
            &format!(
                "{inner}+ {} more: {}\n",
                rest.values().sum::<usize>() + hidden_dirs,
                parts.join(", ")
            ),
            cut,
        ) {
            return;
        }
    }
    if let Some(disk) = disk {
        let missing = not_indexed(disk, dir, refusals, ignore);
        if !missing.is_empty() {
            let total: usize = missing.values().sum();
            let why: Vec<String> = missing.iter().map(|(w, n)| format!("{n} {w}")).collect();
            push(
                out,
                &format!("{inner}+ {total} on disk not indexed: {}\n", why.join(", ")),
                cut,
            );
        }
    }
}

/// A directory's subdirectories and files in the order `sort` asks for. By
/// name that is byte order, which the text tree has always used; the JSON
/// levels fold case on top of it.
fn ordered(dir: &Dir, sort: Sort) -> (Vec<(&String, &Dir)>, Vec<&File>) {
    let mut subdirs: Vec<(&String, &Dir)> = dir.dirs.iter().collect();
    match sort {
        Sort::Recent => subdirs.sort_by_key(|(_, d)| std::cmp::Reverse(d.newest())),
        Sort::Size | Sort::Symbols => subdirs.sort_by_key(|(_, d)| std::cmp::Reverse(d.totals().0)),
        Sort::Name => {}
    }
    let mut files: Vec<&File> = dir.files.iter().collect();
    match sort {
        Sort::Name => files.sort_by(|a, b| a.name.cmp(&b.name)),
        Sort::Size => files.sort_by_key(|f| std::cmp::Reverse(f.lines)),
        Sort::Symbols => files.sort_by_key(|f| std::cmp::Reverse(f.symbols)),
        Sort::Recent => files.sort_by_key(|f| std::cmp::Reverse(f.indexed_at)),
    }
    (subdirs, files)
}

/// The three languages a directory holds most files in, most first.
fn mix(langs: BTreeMap<String, usize>) -> Vec<(String, usize)> {
    let mut mix: Vec<(String, usize)> = langs.into_iter().collect();
    mix.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    mix.truncate(3);
    mix
}

/// A root-relative directory from a request, as its components. `""` is the
/// root. Refused rather than normalised when it climbs or is absolute: the
/// answer reads the disk under the root, and a `..` would read beside it.
pub fn parse_dir(raw: &str) -> Result<Vec<String>> {
    let bytes = raw.as_bytes();
    if raw.starts_with(['/', '\\']) || (bytes.len() >= 2 && bytes[1] == b':') {
        anyhow::bail!("dir is relative to a root, not an absolute path: {raw:?}");
    }
    let parts: Vec<String> = raw
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .map(String::from)
        .collect();
    if parts.iter().any(|p| p == "..") {
        anyhow::bail!("dir stays under its root; {raw:?} climbs out with ..");
    }
    Ok(parts)
}

/// The direct children of `dir`, one level, for each chosen store root that
/// holds it: the portal's lazy file tree, which asks again as each folder
/// opens rather than taking the whole store in one answer.
pub fn level(
    fleet: &Fleet,
    only: Option<&[String]>,
    filter: &crate::filter::Filter,
    dir: &[String],
    sort: Sort,
) -> Result<serde_json::Value> {
    let roots = fleet.roots();
    let (held, refusals) = load(fleet, only, filter, &roots)?;
    // One tree per store and root. A file outside every registered root has
    // no directory to be listed under, so it is left to the text tree.
    let mut trees: Vec<((String, PathBuf), Dir)> = Vec::new();
    for h in held {
        let Some(root) = h.root else {
            continue;
        };
        let key = (h.label, root);
        let at = match trees.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                trees.push((key, Dir::default()));
                trees.len() - 1
            }
        };
        trees[at].1.insert(&h.parts[..h.parts.len() - 1], h.file);
    }
    let mut out: Vec<serde_json::Value> = Vec::new();
    for ((label, root), top) in &trees {
        let Some(node) = top.walk(dir) else {
            continue;
        };
        let disk = dir.iter().fold(root.clone(), |p, d| p.join(d));
        let ignore = semlithignore_of(&disk);
        let mut entry = one_level(node, Some(&disk), &refusals, ignore.as_ref(), sort);
        entry["store"] = serde_json::json!(label);
        entry["root"] = serde_json::json!(crate::plain(&root.to_string_lossy()));
        entry["dir"] = serde_json::json!(dir.join("/"));
        out.push(entry);
    }
    Ok(serde_json::json!({ "roots": out }))
}

/// One directory's subdirectories, files and not-indexed neighbours as JSON,
/// folders first, at most [`LEVEL`] entries with the rest counted in `more`.
fn one_level(
    dir: &Dir,
    disk: Option<&Path>,
    refusals: &BTreeMap<String, store::Refused>,
    ignore: Option<&ignore::gitignore::Gitignore>,
    sort: Sort,
) -> serde_json::Value {
    let (mut subdirs, mut files) = ordered(dir, sort);
    let mut gone = disk
        .map(|d| missing(d, dir, refusals, ignore))
        .unwrap_or_default();
    // The explorer order: case folded, so `README.md` does not sort ahead of
    // `app.js`. Stable, so names equal but for case keep byte order.
    if sort == Sort::Name {
        subdirs.sort_by_key(|(n, _)| n.to_lowercase());
        files.sort_by_key(|f| f.name.to_lowercase());
    }
    gone.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let total = subdirs.len() + files.len() + gone.len();
    let mut budget = LEVEL;
    let mut take = |n: usize| {
        let t = n.min(budget);
        budget -= t;
        t
    };
    let (nd, nf, ng) = (take(subdirs.len()), take(files.len()), take(gone.len()));
    let dirs: Vec<serde_json::Value> = subdirs[..nd]
        .iter()
        .map(|(name, d)| {
            let (files, chunks, langs) = d.totals();
            let langs: Vec<String> = mix(langs).into_iter().map(|(l, _)| l).collect();
            serde_json::json!({ "name": name, "files": files, "chunks": chunks, "langs": langs })
        })
        .collect();
    let files: Vec<serde_json::Value> = files[..nf]
        .iter()
        .map(|f| {
            serde_json::json!({
                "name": f.name,
                "lines": f.lines,
                "symbols": f.symbols,
                "chunks": f.chunks,
                "stale": f.stale(),
                "lang": crate::filter::language_of_path(&f.name).map(|l| l.name),
            })
        })
        .collect();
    let not_indexed: Vec<serde_json::Value> = gone[..ng]
        .iter()
        .map(|(name, why, is_dir)| serde_json::json!({ "name": name, "why": why, "dir": is_dir }))
        .collect();
    serde_json::json!({
        "dirs": dirs,
        "files": files,
        "not_indexed": not_indexed,
        "more": total - nd - nf - ng,
    })
}

/// Append `text` if it fits under the cap; say whether it did.
fn push(out: &mut String, text: &str, cut: &mut bool) -> bool {
    if out.len() + text.len() + 200 > TREE_CHARS {
        *cut = true;
        return false;
    }
    out.push_str(text);
    true
}

/// What sits directly in `disk` that the store does not hold, by reason.
///
/// One `read_dir` of a directory the view already shows, so its cost is
/// bounded by the depth and the cap. Hidden entries are left out, as `ls`
/// leaves them out.
fn not_indexed(
    disk: &Path,
    dir: &Dir,
    refusals: &BTreeMap<String, store::Refused>,
    ignore: Option<&ignore::gitignore::Gitignore>,
) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    for (_, reason, is_dir) in missing(disk, dir, refusals, ignore) {
        let reason = if is_dir {
            format!("{reason} (folder)")
        } else {
            reason
        };
        *out.entry(reason).or_insert(0) += 1;
    }
    out
}

/// Each entry [`not_indexed`] counts, as `(name, reason, is a folder)`.
fn missing(
    disk: &Path,
    dir: &Dir,
    refusals: &BTreeMap<String, store::Refused>,
    ignore: Option<&ignore::gitignore::Gitignore>,
) -> Vec<(String, String, bool)> {
    let mut out: Vec<(String, String, bool)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(disk) else {
        return out;
    };
    let walked: std::collections::HashSet<PathBuf> = {
        let mut builder = ignore::WalkBuilder::new(disk);
        builder
            .max_depth(Some(1))
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .require_git(false)
            .add_custom_ignore_filename(crate::IGNORE_FILE);
        builder
            .build()
            .filter_map(|e| e.ok())
            .map(|e| crate::canonical(e.path()))
            .collect()
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let is_dir = path.is_dir();
        let held = if is_dir {
            dir.dirs.contains_key(&name)
        } else {
            dir.files.iter().any(|f| f.name == name)
        };
        if held {
            continue;
        }
        let key = crate::plain(&crate::canonical(&path).to_string_lossy());
        let reason = match refusals.get(&key) {
            Some(row) => match row.class.as_str() {
                store::class::CONTENT => "refused as a secret".to_string(),
                store::class::CREDENTIAL => "credential file".to_string(),
                store::class::POLICY if is_dir => "generated folder".to_string(),
                store::class::POLICY => "over the size cap".to_string(),
                store::class::UNINDEXABLE => row
                    .rule
                    .split(':')
                    .next()
                    .unwrap_or("unindexable")
                    .to_string(),
                _ => row.rule.clone(),
            },
            None if is_dir && crate::is_generated_dir(&path) => "generated folder".to_string(),
            None if !walked.contains(&crate::canonical(&path)) => {
                if ignore.is_some_and(|i| i.matched_path_or_any_parents(&path, is_dir).is_ignore())
                {
                    crate::IGNORE_FILE.to_string()
                } else {
                    ".gitignore".to_string()
                }
            }
            None if is_dir => continue,
            None => "not indexed yet".to_string(),
        };
        out.push((name, reason, is_dir));
    }
    out
}

/// The `.semlithignore` matcher for a tree, from the nearest file above it.
fn semlithignore_of(dir: &Path) -> Option<ignore::gitignore::Gitignore> {
    let mut at = Some(dir);
    while let Some(d) = at {
        let file = d.join(crate::IGNORE_FILE);
        if file.is_file() {
            let (matcher, _) = ignore::gitignore::Gitignore::new(&file);
            return Some(matcher);
        }
        at = d.parent();
    }
    None
}

/// The deepest root that holds a file, and the file's path relative to it as
/// its components. With no root holding it, the whole path is the relative
/// part, so the text tree still has somewhere to put it.
fn split_root<'a>(path: &str, roots: &'a [PathBuf]) -> (Option<&'a PathBuf>, Vec<String>) {
    let plain = crate::plain(path);
    let mut best: Option<(&PathBuf, &str)> = None;
    for root in roots {
        let r = crate::plain(&root.to_string_lossy());
        if let Some(rest) = plain.strip_prefix(r.as_str())
            && let Some(rest) = rest.strip_prefix(['/', '\\'])
            && best.is_none_or(|(_, b)| rest.len() < b.len())
        {
            best = Some((root, rest));
        }
    }
    let rel = best.map_or(plain.trim_start_matches(['/', '\\']), |(_, rest)| rest);
    let parts = rel
        .split(['/', '\\'])
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    (best.map(|(root, _)| root), parts)
}

/// A file's first few definitions by name: types before functions, in the
/// order they appear, methods and headings left out.
fn firsts(defs: &[store::SymbolSpan]) -> Vec<String> {
    const TYPES: [&str; 6] = ["struct", "enum", "trait", "class", "interface", "type"];
    let mut ranked: Vec<&store::SymbolSpan> = defs
        .iter()
        .filter(|d| !crate::graph::NAVIGATIONAL_KINDS.contains(&d.3.as_str()) && d.3 != "module")
        .filter(|d| d.3 != "method")
        .collect();
    ranked.sort_by_key(|d| (!TYPES.contains(&d.3.as_str()), d.0));
    let mut out: Vec<String> = Vec::new();
    for d in ranked {
        if !out.contains(&d.2) {
            out.push(d.2.clone());
        }
        if out.len() == 3 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, lines: i64, chunks: i64) -> File {
        File {
            name: name.to_string(),
            path: String::new(),
            lines,
            chunks,
            indexed_at: 0,
            symbols: 0,
            firsts: Vec::new(),
            stamp: None,
        }
    }

    fn parts(p: &str) -> Vec<String> {
        p.split('/').map(String::from).collect()
    }

    fn sample() -> Dir {
        let mut top = Dir::default();
        top.insert(&parts("src"), file("main.rs", 10, 2));
        top.insert(&parts("src/portal"), file("app.js", 300, 30));
        top.insert(&parts("src/portal"), file("Zeta.css", 20, 3));
        top.insert(&parts("src/portal/fonts"), file("a.css", 5, 7));
        top.insert(&parts("src/portal/Assets"), file("b.css", 1, 1));
        top.insert(&parts("src/portal/Assets/deep"), file("c.js", 1, 4));
        top
    }

    #[test]
    fn one_level_lists_folders_first_in_explorer_order_with_recursive_totals() {
        let top = sample();
        let node = top.walk(&parts("src/portal")).expect("src/portal is held");
        let level = one_level(node, None, &BTreeMap::new(), None, Sort::Name);

        let names = |key: &str| -> Vec<String> {
            level[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["name"].as_str().unwrap().to_string())
                .collect()
        };
        // Case folded: `Assets` before `fonts`, `app.js` before `Zeta.css`,
        // where byte order would put the capitals first.
        assert_eq!(names("dirs"), ["Assets", "fonts"]);
        assert_eq!(names("files"), ["app.js", "Zeta.css"]);

        // Totals are recursive: `Assets` holds one file itself and one below.
        let assets = &level["dirs"][0];
        assert_eq!(assets["files"], 2);
        assert_eq!(assets["chunks"], 5);
        assert_eq!(assets["langs"], serde_json::json!(["css", "javascript"]));
        assert_eq!(level["files"][0]["lang"], "javascript");
        assert_eq!(level["files"][0]["stale"], false);
        assert_eq!(level["not_indexed"], serde_json::json!([]));
        assert_eq!(level["more"], 0);
    }

    #[test]
    fn size_order_follows_the_text_tree() {
        let top = sample();
        let node = top.walk(&parts("src/portal")).unwrap();
        let level = one_level(node, None, &BTreeMap::new(), None, Sort::Size);
        assert_eq!(level["dirs"][0]["name"], "Assets");
        assert_eq!(level["files"][0]["name"], "app.js");
        assert_eq!(level["files"][1]["name"], "Zeta.css");
    }

    #[test]
    fn a_directory_the_store_does_not_hold_walks_to_nothing() {
        let top = sample();
        assert!(top.walk(&parts("src/missing")).is_none());
        assert!(top.walk(&parts("src/main.rs")).is_none());
        assert!(top.walk(&[]).is_some());
    }

    #[test]
    fn a_level_past_the_cap_counts_the_rest() {
        let mut top = Dir::default();
        for i in 0..LEVEL + 5 {
            top.insert(&[], file(&format!("f{i}.rs"), 1, 1));
        }
        let level = one_level(&top, None, &BTreeMap::new(), None, Sort::Name);
        assert_eq!(level["files"].as_array().unwrap().len(), LEVEL);
        assert_eq!(level["more"], 5);
    }

    #[test]
    fn parse_dir_keeps_to_the_root() {
        assert!(parse_dir("").unwrap().is_empty());
        assert_eq!(parse_dir("src/portal/").unwrap(), ["src", "portal"]);
        assert_eq!(parse_dir(r"src\portal").unwrap(), ["src", "portal"]);
        assert_eq!(parse_dir("./src").unwrap(), ["src"]);
        for bad in [
            "..",
            "src/../..",
            r"src\..\x",
            "/etc",
            r"\etc",
            "C:/x",
            r"C:\x",
        ] {
            assert!(parse_dir(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn split_root_picks_the_deepest_root() {
        let roots = vec![PathBuf::from("/a"), PathBuf::from("/a/b")];
        let (root, rel) = split_root("/a/b/c/d.rs", &roots);
        assert_eq!(root, Some(&PathBuf::from("/a/b")));
        assert_eq!(rel, ["c", "d.rs"]);
        let (root, rel) = split_root("/z/d.rs", &roots);
        assert_eq!(root, None);
        assert_eq!(rel, ["z", "d.rs"]);
    }
}
