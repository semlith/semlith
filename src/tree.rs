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

struct File {
    name: String,
    lines: i64,
    chunks: i64,
    indexed_at: i64,
    symbols: usize,
    firsts: Vec<String>,
    stale: bool,
}

#[derive(Default)]
struct Dir {
    files: Vec<File>,
    dirs: BTreeMap<String, Dir>,
}

impl Dir {
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

/// The tree under the filter's paths, `depth` levels deep, as text.
pub fn render(
    fleet: &Fleet,
    only: Option<&[String]>,
    filter: &crate::filter::Filter,
    depth: usize,
    sort: Sort,
) -> Result<String> {
    let roots = fleet.roots();
    let mut top = Dir::default();
    let mut refusals: BTreeMap<String, store::Refused> = BTreeMap::new();
    let mut rels: Vec<Vec<String>> = Vec::new();
    let mut first_path: Option<String> = None;
    for (_, s) in fleet.chosen_each(only)? {
        let db = s.db();
        for row in store::refusals(db)? {
            refusals.insert(crate::plain(&row.path), row);
        }
        let rows = store::file_rows(db, filter.groups(), store::FileSort::Path, false, i64::MAX)?;
        let paths: Vec<String> = rows.iter().map(|r| r.path.clone()).collect();
        let symbols = store::symbols_in_files(db, &paths)?;
        let stamps = store::file_stamps(db, &paths)?;
        for row in rows {
            let rel = relative(&row.path, &roots);
            let parts: Vec<String> = rel.iter().map(|c| c.to_string()).collect();
            let Some((name, dirs)) = parts.split_last() else {
                continue;
            };
            let defs = symbols.get(&row.path).cloned().unwrap_or_default();
            let stale = stamps.get(&row.path).is_some_and(|(bytes, at)| {
                std::fs::metadata(&row.path).is_ok_and(|m| {
                    m.len() as i64 != *bytes
                        || m.modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .is_some_and(|d| d.as_secs() as i64 > *at)
                })
            });
            let mut node = &mut top;
            for d in dirs {
                node = node.dirs.entry(d.clone()).or_default();
            }
            node.files.push(File {
                name: name.clone(),
                lines: row.lines,
                chunks: row.chunks,
                indexed_at: row.indexed_at,
                symbols: defs
                    .iter()
                    .filter(|d| !crate::graph::NAVIGATIONAL_KINDS.contains(&d.3.as_str()))
                    .count(),
                firsts: firsts(&defs),
                stale,
            });
            first_path.get_or_insert_with(|| crate::plain(&row.path));
            rels.push(parts);
        }
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
    let mut mix: Vec<(String, usize)> = langs.into_iter().collect();
    mix.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mix: Vec<String> = mix
        .iter()
        .take(3)
        .map(|(l, n)| format!("{l} {n}"))
        .collect();
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
        if file.stale {
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
        let reason = if is_dir {
            format!("{reason} (folder)")
        } else {
            reason
        };
        *out.entry(reason).or_insert(0) += 1;
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

/// A file's path relative to the root that holds it, as its components.
fn relative(path: &str, roots: &[PathBuf]) -> Vec<String> {
    let plain = crate::plain(path);
    let mut best: Option<&str> = None;
    for root in roots {
        let r = crate::plain(&root.to_string_lossy());
        if let Some(rest) = plain.strip_prefix(r.as_str())
            && let Some(rest) = rest.strip_prefix(['/', '\\'])
            && best.is_none_or(|b| rest.len() < b.len())
        {
            best = Some(rest);
        }
    }
    let rel = best.unwrap_or(plain.trim_start_matches(['/', '\\']));
    rel.split(['/', '\\'])
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
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
