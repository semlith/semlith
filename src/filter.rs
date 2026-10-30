//! Narrowing a search to part of the corpus.
//!
//! A [`Filter`] is resolved to SQLite `GLOB` patterns and then to a set of
//! chunk ids by [`crate::store::filtered_chunk_ids`]. That one id set drives
//! both halves of the hybrid search — the allowlist handed to the vector index
//! and the predicate inside the FTS5 query — so the two can never disagree
//! about which chunks were eligible.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// One `--lang` name and everything that counts as it.
pub struct Language {
    pub name: &'static str,
    /// Lowercase, without the dot, sorted.
    pub extensions: &'static [&'static str],
    /// Whole filenames, lowercase, for the languages whose files carry no
    /// extension at all. Glob patterns, so `dockerfile.*` covers
    /// `Dockerfile.prod` alongside the bare name.
    pub filenames: &'static [&'static str],
}

/// What makes up each language name accepted by `--lang`.
///
/// The name is the signal, and for almost every language the name is the
/// extension. The two exceptions are the reason `filenames` exists: a
/// Dockerfile and a Makefile are identified by being called that, and a filter
/// that only knew extensions would answer `--lang dockerfile` with an empty
/// result on a repository full of them — which is indistinguishable from
/// "there are none", the worst answer available.
///
/// Contents are never read. Telling Perl from anything else by looking at it is
/// a research project, and a store is queried far more often than it is built,
/// so guessing at query time would cost on every search.
pub const LANGUAGES: &[Language] = &[
    lang("c", &["c", "h"], &[]),
    lang("clojure", &["clj", "cljc", "cljs", "edn"], &[]),
    lang("cpp", &["cc", "cpp", "cxx", "hh", "hpp", "hxx"], &[]),
    lang("csharp", &["cs"], &[]),
    lang("css", &["css", "sass", "scss"], &[]),
    lang("dart", &["dart"], &[]),
    lang(
        "dockerfile",
        &["dockerfile"],
        &["dockerfile", "dockerfile.*"],
    ),
    lang("elixir", &["ex", "exs"], &[]),
    lang("elm", &["elm"], &[]),
    lang("erlang", &["erl", "hrl"], &[]),
    lang("fortran", &["f", "f03", "f90", "f95"], &[]),
    lang("go", &["go"], &[]),
    lang("graphql", &["gql", "graphql"], &[]),
    lang("groovy", &["gradle", "groovy"], &[]),
    lang("haskell", &["hs"], &[]),
    lang("html", &["htm", "html"], &[]),
    lang("java", &["java"], &[]),
    lang("javascript", &["cjs", "js", "jsx", "mjs"], &[]),
    lang("json", &["json"], &[]),
    lang("julia", &["jl"], &[]),
    lang("kotlin", &["kt", "kts"], &[]),
    lang("lua", &["lua"], &[]),
    lang("makefile", &["mk"], &["gnumakefile", "makefile"]),
    lang("markdown", &["markdown", "md"], &[]),
    lang("nix", &["nix"], &[]),
    // `.m` is Objective-C and it is also MATLAB. Objective-C is the one with a
    // header file beside it, which is the only signal an extension-based filter
    // has, so it takes the extension.
    lang("objective-c", &["m", "mm"], &[]),
    lang("ocaml", &["ml", "mli"], &[]),
    lang("perl", &["pl", "pm", "t"], &[]),
    lang("php", &["php"], &[]),
    lang("powershell", &["ps1", "psd1", "psm1"], &[]),
    lang("proto", &["proto"], &[]),
    lang("python", &["py", "pyi"], &[]),
    lang("r", &["r"], &[]),
    lang("ruby", &["rb"], &[]),
    lang("rust", &["rs"], &[]),
    lang("scala", &["sc", "scala"], &[]),
    lang("shell", &["bash", "sh", "zsh"], &[]),
    lang("sql", &["sql"], &[]),
    lang("svelte", &["svelte"], &[]),
    lang("swift", &["swift"], &[]),
    lang("terraform", &["tf", "tfvars"], &[]),
    lang("toml", &["toml"], &[]),
    lang("typescript", &["cts", "mts", "ts", "tsx"], &[]),
    lang("vue", &["vue"], &[]),
    lang("yaml", &["yaml", "yml"], &[]),
    lang("zig", &["zig"], &[]),
];

/// One row of [`LANGUAGES`], so the table reads as a table.
const fn lang(
    name: &'static str,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
) -> Language {
    Language {
        name,
        extensions,
        filenames,
    }
}

/// The entries in [`LANGUAGES`] that are prose, markup or data rather than
/// implementation.
///
/// Named as the short list rather than tagging all 46 rows, because this is a
/// ranking hint and not a fact about the language: `prefer: docs` lifting a
/// README above the function it describes is the whole intent, and nothing
/// else in the crate asks the question.
const PROSE: &[&str] = &["css", "html", "json", "markdown", "toml", "yaml"];

/// The entry a path belongs to, by extension or by whole filename.
///
/// The one place that answers "what language is this file", so the search
/// filter, the `prefer` hint and the graph extractor cannot disagree about it.
/// Matching is case-insensitive because `Makefile`, `makefile` and `MAKEFILE`
/// are the same file to everyone except a string comparison.
pub fn language_of_path(path: &str) -> Option<&'static Language> {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit(['/', '\\']).next().unwrap_or(&lower);
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    LANGUAGES.iter().find(|entry| {
        entry.extensions.contains(&ext)
            || entry.filenames.iter().any(|f| match f.strip_suffix(".*") {
                Some(stem) => name.starts_with(stem),
                None => *f == name,
            })
    })
}

/// Whether a path holds implementation, for [`crate::Prefer`].
///
/// A path in no language at all — a `.txt` note, an extracted `.epub` — reads
/// as prose, which is what it is.
pub fn is_code(path: &str) -> bool {
    language_of_path(path).is_some_and(|entry| !PROSE.contains(&entry.name))
}

/// The entry for a `--lang` name, matched case-insensitively.
pub fn language(name: &str) -> Option<&'static Language> {
    let wanted = name.to_ascii_lowercase();
    LANGUAGES.iter().find(|entry| entry.name == wanted)
}

/// Which chunks a search is allowed to see.
///
/// Patterns are grouped: within a group they union, across groups they
/// intersect. So `--ext rs --ext toml` is "Rust or TOML", while
/// `--path 'src/**' --ext md` is "Markdown, under src".
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Filter {
    groups: Vec<Vec<String>>,
}

impl Filter {
    /// Build a filter from the three user-facing kinds.
    ///
    /// Fails on a language name that is not in [`LANGUAGES`]. A silent miss
    /// there is indistinguishable from an empty corpus, which is the worst
    /// possible answer to give an agent.
    pub fn new(paths: &[String], exts: &[String], langs: &[String]) -> Result<Self> {
        let mut groups = Vec::new();

        if !paths.is_empty() {
            groups.push(paths.iter().flat_map(|p| as_folder_too(p)).collect());
        }

        // Extensions and languages are both extension sets, so they share one
        // group: `--ext rs --lang markdown` means "Rust or Markdown", the same
        // way two `--ext` flags do.
        //
        // A leading `!` is carried onto every pattern the value expands into,
        // so `--lang '!makefile'` excludes all three of the spellings that
        // `--lang makefile` would have included.
        let mut patterns: Vec<String> = Vec::new();
        for e in exts {
            let (bang, value) = negation(e);
            patterns.push(format!(
                "{bang}*.{}",
                value.trim_start_matches('.').to_ascii_lowercase()
            ));
        }
        for name in langs {
            let (bang, value) = negation(name);
            let Some(entry) = language(value) else {
                bail!("unknown language {value:?}; run `semlith languages` for the list");
            };
            patterns.extend(entry.extensions.iter().map(|e| format!("{bang}*.{e}")));
            // A filename pattern resolves into the same group as an extension
            // one, so `--lang dockerfile --ext md` unions exactly the way two
            // extensions do and nothing downstream has a second case to handle.
            patterns.extend(entry.filenames.iter().map(|f| format!("{bang}{f}")));
        }
        if !patterns.is_empty() {
            groups.push(patterns.iter().map(|p| anchor(p)).collect());
        }

        Ok(Self { groups })
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// AND-groups of OR-patterns, ready for the SQL builder.
    /// This filter, narrowed to one language as well.
    ///
    /// A new group rather than a merge, because groups intersect: whatever the
    /// caller already asked for stays, and the language is an additional
    /// requirement rather than an alternative to it.
    pub fn and_language(&self, language: &str) -> Result<Self> {
        let added = Self::new(&[], &[], std::slice::from_ref(&language.to_string()))?;
        let mut groups = self.groups.clone();
        groups.extend(added.groups);
        Ok(Self { groups })
    }

    pub fn groups(&self) -> &[Vec<String>] {
        &self.groups
    }
}

/// Turn a user's pattern into one matchable against the canonical absolute
/// path a store holds.
///
/// A relative pattern gains a `*/` prefix, so `src/**` finds
/// `/home/me/proj/src/lib.rs` no matter which directory the search runs from.
/// An absolute pattern is used verbatim, and therefore matches only what it
/// literally covers.
///
/// Lowercased, because the query side compares against `lower(files.path)`:
/// `README.MD` and `readme.md` are the same file to anyone typing `--ext md`.
/// Split a user's value into its exclusion marker and the value under it.
///
/// A leading `!` is the marker, the way Sourcebot and every `.gitignore`
/// reader spell it. It is punctuation rather than part of the value, so what
/// follows is anchored, lowercased and reported in errors exactly as the same
/// value without the `!` would be.
fn negation(value: &str) -> (&'static str, &str) {
    match value.strip_prefix('!') {
        Some(rest) => ("!", rest),
        None => ("", value),
    }
}

/// A path with no glob in it, anchored, and also as the folder of that name
/// (0.47.1): `src/oauth` is the files under `src/oauth`, the way a person and
/// `semlith_files` ("what is in this folder") mean it. A file and a folder
/// cannot share a path, so a pattern that names a file still matches only it.
fn as_folder_too(pattern: &str) -> Vec<String> {
    if pattern.contains(['*', '?', '[']) {
        return vec![anchor(pattern)];
    }
    let folder = format!("{}/**", pattern.trim_end_matches(['/', '\\']));
    vec![anchor(pattern), anchor(&folder)]
}

fn anchor(pattern: &str) -> String {
    // The marker stays in front of the anchored pattern rather than being
    // anchored into the middle of it: `!src/**` is the exclusion of `*/src/**`,
    // not a search for a directory named `!src`.
    if let ("!", rest) = negation(pattern) {
        return format!("!{}", anchor(rest));
    }
    // Always `/`, on every platform. The query side compares against a path
    // whose separators have been normalised to `/` (`store::GLOB_PATH`), so a
    // pattern written the way everyone writes one matches a Windows store as
    // well as a unix one. Translating the pattern to backslashes instead —
    // which is what this did — matched nothing whenever the store held a path
    // in any other form, and the store holds the verbatim form (#74).
    // ASCII, as SQLite's `lower()` is: Unicode lowercasing turned an `É` in a
    // `!` exclusion into an `é` the stored path never matched (audit S24).
    let pattern = pattern.to_ascii_lowercase().replace('\\', "/");
    if is_absolute(&pattern) {
        pattern
    } else {
        format!("*/{pattern}")
    }
}

fn is_absolute(pattern: &str) -> bool {
    if pattern.starts_with('/') {
        return true;
    }
    // `c:\...` on Windows. Elsewhere a colon is an ordinary filename character.
    cfg!(windows) && pattern.as_bytes().get(1) == Some(&b':')
}

#[cfg(test)]
mod tests {
    /// `prefer` needs to tell a README from the function it describes, and a
    /// file in no language at all is prose.
    #[test]
    fn code_is_told_from_prose_by_the_language_table() {
        for path in [
            "src/lib.rs",
            "a/b.py",
            "Makefile",
            "deploy/Dockerfile",
            "x.ts",
        ] {
            assert!(super::is_code(path), "{path} is code");
        }
        for path in [
            "README.md",
            "docs/architecture.md",
            "Cargo.toml",
            "page.html",
            "data.json",
            "notes.txt",
            "book.epub",
        ] {
            assert!(!super::is_code(path), "{path} is not code");
        }
    }

    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| i.to_string()).collect()
    }

    /// The rule the README documents: a relative glob is anchored so it works
    /// from any directory; an absolute one is not, so it means exactly itself.
    /// 0.47.1: a folder named without a glob is the files under it. Agents
    /// write `path: ["src/oauth"]` far more often than `src/oauth/**`, and
    /// `semlith_files` says it lists what is in a folder; as a bare GLOB it
    /// matched only a file of that exact name, which is to say nothing.
    #[test]
    fn a_bare_folder_matches_the_files_under_it() {
        let f = Filter::new(&s(&["src/oauth"]), &[], &[]).unwrap();
        assert_eq!(f.groups(), [["*/src/oauth", "*/src/oauth/**"]]);
        let f = Filter::new(&s(&["/home/me/proj/lib/"]), &[], &[]).unwrap();
        assert_eq!(f.groups(), [["/home/me/proj/lib/", "/home/me/proj/lib/**"]]);
        let f = Filter::new(&s(&["!vendor"]), &[], &[]).unwrap();
        assert_eq!(f.groups(), [["!*/vendor", "!*/vendor/**"]]);
        // A glob already says what it means.
        let f = Filter::new(&s(&["src/*.rs"]), &[], &[]).unwrap();
        assert_eq!(f.groups(), [["*/src/*.rs"]]);
    }

    #[test]
    fn a_relative_pattern_is_anchored_and_an_absolute_one_is_not() {
        // `/` on every platform: the query side normalises the path column to
        // `/` on Windows, so one pattern language works everywhere and
        // `--path 'src/**'` means the same thing on all three (#74).
        let f = Filter::new(&s(&["src/**"]), &[], &[]).unwrap();
        assert_eq!(f.groups(), [["*/src/**"]]);

        let f = Filter::new(&s(&["/home/me/proj/src/*"]), &[], &[]).unwrap();
        assert_eq!(f.groups(), [["/home/me/proj/src/*"]]);

        // A pattern somebody typed with backslashes still means the same thing.
        let f = Filter::new(&s(&[r"src\**"]), &[], &[]).unwrap();
        assert_eq!(f.groups(), [["*/src/**"]]);
    }

    #[test]
    fn extensions_and_languages_share_one_group_and_union() {
        let f = Filter::new(&[], &s(&["toml"]), &s(&["rust"])).unwrap();
        assert_eq!(f.groups().len(), 1, "one group means they union");
        assert_eq!(f.groups()[0], ["*/*.toml", "*/*.rs"]);
    }

    /// `--path 'src/**' --ext md` must mean "Markdown under src", not
    /// "Markdown, or anything under src".
    #[test]
    fn paths_and_extensions_are_separate_groups_and_intersect() {
        let f = Filter::new(&s(&["src/**"]), &s(&["md"]), &[]).unwrap();
        assert_eq!(f.groups().len(), 2);
    }

    #[test]
    fn a_leading_dot_on_an_extension_is_accepted() {
        let plain = Filter::new(&[], &s(&["rs"]), &[]).unwrap();
        let dotted = Filter::new(&[], &s(&[".rs"]), &[]).unwrap();
        assert_eq!(plain, dotted);
    }

    #[test]
    fn an_unknown_language_names_the_command_that_lists_them() {
        let err = Filter::new(&[], &[], &s(&["klingon"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("semlith languages"), "unhelpful error: {err}");
    }

    /// The two languages whose files carry no extension. Without the filename
    /// column `--lang dockerfile` resolves to `*.dockerfile` alone, which
    /// matches none of the Dockerfiles anyone actually has.
    #[test]
    fn a_language_with_no_extension_matches_by_filename() {
        let f = Filter::new(&[], &[], &s(&["dockerfile"])).unwrap();
        assert_eq!(
            f.groups()[0],
            ["*/*.dockerfile", "*/dockerfile", "*/dockerfile.*"],
            "a bare Dockerfile and a Dockerfile.prod are both the language"
        );

        let f = Filter::new(&[], &[], &s(&["makefile"])).unwrap();
        assert_eq!(f.groups()[0], ["*/*.mk", "*/gnumakefile", "*/makefile"]);

        // Filenames land in the same group extensions do, so the two union
        // rather than intersecting — one code path, so the vector allowlist and
        // the FTS5 predicate cannot come to different answers about them.
        let f = Filter::new(&[], &s(&["md"]), &s(&["dockerfile"])).unwrap();
        assert_eq!(f.groups().len(), 1);
        assert_eq!(f.groups()[0].len(), 4);
    }

    /// `--lang` is case-insensitive on the way in, the way `--ext` is.
    #[test]
    fn a_language_name_is_matched_however_it_is_typed() {
        let lower = Filter::new(&[], &[], &s(&["dockerfile"])).unwrap();
        let shouted = Filter::new(&[], &[], &s(&["DockerFile"])).unwrap();
        assert_eq!(lower, shouted);
    }

    #[test]
    fn no_flags_is_no_filter() {
        assert!(Filter::new(&[], &[], &[]).unwrap().is_empty());
    }

    /// A leading `!` excludes. The exclusion stays in the group its kind
    /// belongs to, because it applies after the inclusions of that kind and
    /// nowhere else: `--path 'src/**' --path '!src/vendor/**'` is one group.
    #[test]
    fn a_leading_bang_marks_a_pattern_as_an_exclusion() {
        let f = Filter::new(&s(&["src/**", "!src/vendor/**"]), &[], &[]).unwrap();
        assert_eq!(f.groups().len(), 1, "one kind is one group");
        assert_eq!(f.groups()[0], ["*/src/**", "!*/src/vendor/**"]);
    }

    /// The `!` survives anchoring rather than being anchored into the middle
    /// of the pattern, and the pattern under it is anchored exactly as an
    /// inclusion of the same shape would be.
    #[test]
    fn an_excluded_pattern_is_anchored_under_its_bang() {
        let f = Filter::new(&s(&["!/home/me/proj/vendor/*"]), &[], &[]).unwrap();
        assert_eq!(f.groups()[0], ["!/home/me/proj/vendor/*"]);

        let f = Filter::new(&s(&[r"!src\vendor\**"]), &[], &[]).unwrap();
        assert_eq!(f.groups()[0], ["!*/src/vendor/**"]);
    }

    /// An excluded extension and an excluded language expand the same way
    /// their included forms do, with every pattern they produce excluded.
    #[test]
    fn an_extension_and_a_language_both_negate() {
        let f = Filter::new(&[], &s(&["!md"]), &[]).unwrap();
        assert_eq!(f.groups()[0], ["!*/*.md"]);

        let f = Filter::new(&[], &[], &s(&["!makefile"])).unwrap();
        assert_eq!(f.groups()[0], ["!*/*.mk", "!*/gnumakefile", "!*/makefile"]);
    }

    /// An unknown language is still named without its `!`, so the error reads
    /// as the language the user typed rather than as punctuation.
    #[test]
    fn an_excluded_unknown_language_still_names_the_language() {
        let err = Filter::new(&[], &[], &s(&["!klingon"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("klingon"), "unhelpful error: {err}");
        assert!(!err.contains("!klingon"), "the bang is punctuation: {err}");
    }

    /// A filter holding nothing but exclusions is a filter: everything except.
    #[test]
    fn an_exclusion_on_its_own_is_not_an_empty_filter() {
        let f = Filter::new(&s(&["!vendor/**"]), &[], &[]).unwrap();
        assert!(!f.is_empty());
    }

    /// Every entry must be lowercase and its extensions sorted, since `--lang`
    /// lowercases what it is given and `semlith languages` prints the table
    /// verbatim.
    #[test]
    fn the_language_table_is_normalised_and_sorted() {
        let mut previous = "";
        for entry in LANGUAGES {
            let name = entry.name;
            assert!(name > previous, "language table is out of order at {name}");
            previous = name;
            assert_eq!(name, name.to_ascii_lowercase());
            for set in [entry.extensions, entry.filenames] {
                let mut sorted = set.to_vec();
                sorted.sort_unstable();
                assert_eq!(&sorted, &set, "{name}'s entries are out of order");
                assert!(
                    set.iter().all(|e| *e == e.to_ascii_lowercase()),
                    "{name} has an uppercase entry"
                );
            }
        }
    }
}

// ---------------------------------------------------------------- the deny-list

/// Directories whose contents are credentials, and nothing else.
///
/// Matched by whole path components wherever they appear, not only under the
/// home (audit S12): a `.aws` copied into a project, a `.ssh` inside a synced
/// folder and `~/.ssh` hold the same thing. A deny-list is a blunt instrument
/// and this one is deliberately short: every entry is a directory whose whole
/// purpose is to hold secrets, so refusing it costs a user nothing they meant
/// to index. `.git` is here because its `config` carries a remote's token as
/// readily as a `.netrc` does, and a repository's own ignore file whitelisting
/// `!.git/` must not be how it reaches a store.
pub const DENIED_DIRS: &[&str] = &[
    ".git",
    ".ssh",
    ".aws",
    ".gnupg",
    ".kube",
    ".config/gcloud",
    ".config/gh",
    ".azure",
    ".docker",
    "Library/Keychains",
    ".password-store",
    ".local/share/keyrings",
];

/// File names that are a credential wherever they are.
///
/// Matched against the file name alone, case-insensitively, with `*` matching
/// any run of characters. These are not about where a file lives: a `.env` in a
/// repository is the same kind of thing as a `.env` in a home directory, and an
/// agent that indexed one has put the contents of every environment variable a
/// service needs into a store it can then be asked to search.
pub const DENIED_NAMES: &[&str] = &[
    // Every shape of environment file, not only the two that are conventional.
    // `.env*` subsumes `.env` and `.env.*` and covers `.envrc`, `.env-local`
    // and `.env.vault`; `*.env` and `*.env.*` cover `dev.env`,
    // `production.env.local` and the files a Docker `env_file` points at.
    ".env*",
    "*.env",
    "*.env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.jks",
    "id_rsa*",
    "id_ed25519*",
    "*credentials*",
    "*secret*",
    "*.tfstate",
    "*.kdbx",
    // Per-user credential dotfiles. Until 0.19.0 the hidden-file rule caught
    // these on its own; now that a dotfile the user's own `.gitignore`
    // whitelisted is walked and indexed, each one has to be named or the
    // change would put an npm token into a store.
    ".npmrc",
    ".netrc",
    ".pypirc",
    ".pgpass",
    ".htpasswd",
    ".boto",
    ".s3cfg",
    "*.ppk",
];

/// Why a path was not indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// Under a directory whose contents are credentials.
    Directory(&'static str),
    /// Named like a credential.
    Name(&'static str),
    /// A dotfile, which the walker skips and an explicit path used to slip
    /// past.
    Hidden,
    /// The home directory could not be determined, so whether this path is
    /// under one of the credential directories is not knowable. The rule fails
    /// closed: on Windows, where nothing sets `HOME`, it used to be skipped
    /// entirely and `~/.kube/config` was indexed like any other file (#72).
    NoHome,
}

impl Denied {
    /// One line an agent can act on, naming the rule rather than restating the
    /// path.
    pub fn reason(&self) -> String {
        match self {
            Denied::Directory(".git") => "inside .git, git's own state, whose config can hold a \
                 remote's token — semlith does not index it"
                .to_string(),
            Denied::Directory(dir) => {
                format!("inside {dir}, which holds credentials — semlith does not index it")
            }
            Denied::Name(pattern) => {
                format!("matches {pattern}, which names a credential — semlith does not index it")
            }
            Denied::Hidden => "is a hidden file, which semlith skips when walking and does not \
                 index when named"
                .to_string(),
            Denied::NoHome => "cannot be checked against the credential directories, because \
                 semlith cannot tell where your home directory is — set SEMLITH_HOME, or HOME"
                .to_string(),
        }
    }
}

/// Whether this path is one semlith refuses to index, and why.
///
/// Applied to every walked entry and to every explicitly named path alike,
/// which is the point: the walker already skipped hidden files and the denied
/// directories under them, and an explicit `semlith_index ~/.ssh/id_rsa` went
/// straight past all of it.
pub fn denied(path: &Path) -> Option<Denied> {
    let home = crate::home::user_home().ok().map(|h| crate::canonical(&h));
    denied_against(&crate::canonical(path), home.as_deref())
}

/// [`denied`] against a home given rather than resolved, so the case that
/// matters — there is no home — is a test rather than an environment variable
/// three threads are fighting over.
///
/// Fails closed. An unknown home used to mean the directory rules were skipped
/// and everything under them sailed through, which on Windows — where nothing
/// sets `HOME` — was every run (#72).
/// Both paths are taken as already canonical, which is the 0.19.0 change. The
/// walk hands out canonical paths (`crate::walk`) and the home is resolved once
/// per run, so doing it again here was two `canonicalize` calls per file — two
/// opened handles per file on Windows, on a corpus where the answer for the
/// home could not have changed. [`denied`] canonicalises for the callers that
/// have a single path and no run around it.
pub(crate) fn denied_against(path: &Path, home: Option<&Path>) -> Option<Denied> {
    if let Some(dir) = denied_dir(path) {
        return Some(Denied::Directory(dir));
    }
    // The directory rules no longer need the home, but an unknown one still
    // fails closed: whatever else keys off it (the scan, the boundary) has
    // been told nothing it can check against.
    if home.is_none() {
        return Some(Denied::NoHome);
    }

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    for pattern in DENIED_NAMES {
        if glob_match(pattern, &name) {
            return Some(Denied::Name(pattern));
        }
    }
    if name.starts_with('.') && name.len() > 1 {
        return Some(Denied::Hidden);
    }
    None
}

/// The [`DENIED_DIRS`] entry this path lies inside, if any: its components
/// matched as a run anywhere in the path, ASCII case-insensitively because
/// the default file systems on macOS and Windows are.
pub fn denied_dir(path: &Path) -> Option<&'static str> {
    let parts = components(path);
    DENIED_DIRS.iter().copied().find(|dir| {
        let want: Vec<&str> = dir.split('/').collect();
        parts.windows(want.len()).any(|w| same(w, &want))
    })
}

/// Whether this directory is itself one of [`DENIED_DIRS`] — its last
/// components are an entry. The walk prunes it there, so nothing under it is
/// read, whatever an ignore file whitelists.
pub fn is_denied_dir(path: &Path) -> bool {
    let parts = components(path);
    DENIED_DIRS.iter().any(|dir| {
        let want: Vec<&str> = dir.split('/').collect();
        parts.len() >= want.len() && same(&parts[parts.len() - want.len()..], &want)
    })
}

fn components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn same(have: &[String], want: &[&str]) -> bool {
    have.iter()
        .zip(want)
        .all(|(h, w)| h.eq_ignore_ascii_case(w))
}

/// `*` against a name, with everything else literal.
///
/// Small enough to read, which matters more here than generality: this is the
/// predicate deciding whether a credential reaches a store, and a pattern
/// language with surprises in it is a rule nobody can check.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name;
    // The first segment has to be at the start, and the last at the end.
    if let Some(first) = parts.first()
        && !first.is_empty()
    {
        match rest.strip_prefix(first) {
            Some(tail) => rest = tail,
            None => return false,
        }
    }
    if let Some(last) = parts.last()
        && !last.is_empty()
    {
        match rest.strip_suffix(last) {
            Some(head) => rest = head,
            None => return false,
        }
    }
    for middle in &parts[1..parts.len().saturating_sub(1)] {
        if middle.is_empty() {
            continue;
        }
        match rest.find(middle) {
            Some(at) => rest = &rest[at + middle.len()..],
            None => return false,
        }
    }
    true
}

/// Whether a path is inside a boundary an agent may index.
///
/// The roots the target store is registered against, or the home directory. An
/// agent holding the key can index a repository it was pointed at; it cannot
/// index `/etc`, another user's home, or a directory nobody told semlith about.
/// The person typing `semlith index` on the command line is the owner of the
/// machine and is not held to this — which is the one asymmetry in the rule,
/// and it is deliberate.
pub fn within_boundary(path: &Path, roots: &[PathBuf]) -> bool {
    within_resolved(path, &resolve_boundary(roots))
}

/// The directories a boundary admits, each resolved once, for a pass that asks
/// about many paths.
///
/// A run's roots cannot move while it runs, and resolving every root again for
/// every file was a filesystem call per root per file on top of the one the
/// file itself needs — about 66,000 paths each time a 0.30.0 slice started.
pub fn resolve_boundary(roots: &[PathBuf]) -> Vec<String> {
    if roots.is_empty() {
        // The home directory is the fallback for a store that has no roots at
        // all — a `--store <dir>` the registry has never heard of — and for
        // nothing else. It used to apply to every store, which made the
        // boundary no boundary: every store on the machine shares a home, so
        // an agent pointed at one repository could index another one into it
        // and be allowed. That is how the `semlith` store came to hold 262
        // files belonging to another project, under a Privacy page that says the
        // roots are enforced.
        //
        // An unknown home makes this stricter rather than looser: nothing is
        // inside a boundary semlith cannot locate.
        return crate::home::user_home()
            .ok()
            .map(|home| comparable(&home))
            .into_iter()
            .collect();
    }
    roots.iter().map(|root| comparable(root)).collect()
}

/// [`within_boundary`] against roots [`resolve_boundary`] already resolved.
pub fn within_resolved(path: &Path, resolved: &[String]) -> bool {
    let real = comparable(path);
    resolved.iter().any(|root| under(&real, root))
}

/// A path in the one shape two of them can be compared in.
///
/// `crate::canonical` hands back what it was given when the path does not
/// resolve, and on Windows a path that resolves comes back verbatim —
/// `\\?\C:\work\api`. So a root that exists and a file that does not were
/// being compared in two different spellings of the same place, and the file
/// read as outside a boundary it was inside. That matters more since a store
/// is confined to its roots: a file missing for a moment would be judged to
/// belong to another store and its rows dropped.
fn comparable(path: &Path) -> String {
    crate::plain(&crate::canonical(path).to_string_lossy())
}

/// Whether one comparable path is the other or sits under it.
///
/// Segment-wise rather than a string prefix: `/work/api-old` starts with
/// `/work/api` and is not under it.
fn under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches(['/', '\\']);
    if root.is_empty() {
        return true;
    }
    match path.strip_prefix(root) {
        Some("") => true,
        Some(rest) => rest.starts_with('/') || rest.starts_with('\\'),
        None => false,
    }
}

#[cfg(test)]
mod deny_tests {
    use super::*;

    #[test]
    fn a_star_matches_a_run_of_anything() {
        assert!(glob_match(".env", ".env"));
        assert!(glob_match(".env.*", ".env.production"));
        assert!(!glob_match(".env.*", ".environment"));
        assert!(glob_match("*.pem", "server.pem"));
        assert!(!glob_match("*.pem", "pem"));
        assert!(glob_match("id_rsa*", "id_rsa"));
        assert!(glob_match("id_rsa*", "id_rsa.pub"));
        assert!(glob_match(
            "*credentials*",
            "service-account-credentials.json"
        ));
        assert!(glob_match("*secret*", "my-secrets.yaml"));
        assert!(!glob_match("*secret*", "secrat"));
        assert!(glob_match("*.tfstate", "terraform.tfstate"));
    }

    /// The rule that refuses `~/.ssh` and `~/.kube` has to hold when semlith
    /// cannot tell where home is, not be skipped. On Windows nothing sets
    /// `HOME`, so before 0.17.1 this was every run on that platform: the
    /// deny-list was documented on the Privacy page and did not run (#72).
    #[test]
    fn a_path_is_refused_when_the_home_directory_is_unknown() {
        let home = Path::new("/home/someone");
        assert_eq!(
            denied_against(&home.join(".kube").join("config"), Some(home)),
            Some(Denied::Directory(".kube")),
            "the rule does not hold with a home"
        );
        assert_eq!(
            denied_against(Path::new("/srv/corpus/notes.md"), None),
            Some(Denied::NoHome),
            "an unknown home let an ordinary path through, so it let ~/.kube through too"
        );
        assert_eq!(
            denied_against(&home.join(".kube").join("config"), None),
            Some(Denied::Directory(".kube")),
            "the credential itself must not slip past either, and is refused by its own rule"
        );
        let refusal = Denied::NoHome.reason();
        assert!(
            refusal.contains("home directory"),
            "the reason should say what semlith could not determine: {refusal}"
        );
    }

    /// Audit S12: a credential directory is one wherever it sits — copied
    /// into a project, inside a synced folder — and `.git` is one too. Matched
    /// by whole components, so `.github` and `.gitignore` are not.
    #[test]
    fn a_credential_directory_is_refused_wherever_it_sits() {
        let home = Some(Path::new("/home/x"));
        for (path, dir) in [
            ("/work/api/.git/config", ".git"),
            ("/work/api/.aws/credentials", ".aws"),
            ("/home/x/.config/gh/hosts.yml", ".config/gh"),
            ("/mnt/sync/backup/.SSH/id_ed25519.pub", ".ssh"),
            ("/work/vendor/.config/gcloud/creds.db", ".config/gcloud"),
        ] {
            assert_eq!(
                denied_against(Path::new(path), home),
                Some(Denied::Directory(dir)),
                "{path}"
            );
        }
        for path in [
            "/work/api/.github/workflows/ci.yml",
            "/work/api/.gitignore",
            "/work/api/src/git/config.rs",
            "/work/api/config/gh/readme.md",
        ] {
            assert_eq!(denied_dir(Path::new(path)), None, "{path}");
        }
        assert!(is_denied_dir(Path::new("/work/api/.git")));
        assert!(is_denied_dir(Path::new("/home/x/.config/gh")));
        assert!(!is_denied_dir(Path::new("/home/x/.config")));
        assert!(!is_denied_dir(Path::new("/work/api/.git/objects")));
    }

    /// Every name the widened list is supposed to cover, one row each.
    ///
    /// A table rather than prose because 0.19.0 walks and indexes a dotfile
    /// the user's own `.gitignore` whitelisted, and the hidden-file rule used
    /// to be what kept `.npmrc` and `.envrc` out of a store. A typo in one of
    /// these patterns is now the difference between a refused file and a
    /// credential in a searchable index, so it fails the build.
    #[test]
    fn every_credential_name_is_refused() {
        for name in [
            ".env",
            ".env.local",
            ".env.vault",
            ".envrc",
            ".env-local",
            // On purpose. The name says what the file holds, and a placeholder
            // today is a filled-in credential on somebody's branch tomorrow.
            ".env.example",
            "dev.env",
            "production.env.local",
            ".npmrc",
            ".netrc",
            ".pypirc",
            ".pgpass",
            ".htpasswd",
            ".boto",
            ".s3cfg",
            "server.ppk",
        ] {
            let path = PathBuf::from("/work/api").join(name);
            assert!(
                matches!(
                    denied_against(&path, Some(Path::new("/home/x"))),
                    Some(Denied::Name(_))
                ),
                "{name} must be refused by name, not by any other rule"
            );
        }
        // Not credentials, and a widened pattern that swallowed them would be
        // a release that stopped indexing ordinary code.
        for name in ["environment.ts", "env.rs", "envelope.md", "preventable.go"] {
            let path = PathBuf::from("/work/api").join(name);
            assert_eq!(
                denied_against(&path, Some(Path::new("/home/x"))),
                None,
                "{name} is not a credential"
            );
        }
    }

    /// The case the finding is about: a file an agent asked for by name, which
    /// the walker's rules never saw.
    #[test]
    fn a_credential_is_denied_wherever_it_is_named() {
        assert!(matches!(
            denied(Path::new("/work/api/.env")),
            Some(Denied::Name(".env*"))
        ));
        assert!(matches!(
            denied(Path::new("/work/api/service-account-credentials.json")),
            Some(Denied::Name("*credentials*"))
        ));
        assert!(matches!(
            denied(Path::new("/work/api/tls/server.pem")),
            Some(Denied::Name("*.pem"))
        ));
        // The hidden-file rule the walker applies, applied to an explicit path.
        assert!(matches!(
            denied(Path::new("/work/api/.hidden-notes")),
            Some(Denied::Hidden)
        ));
        // An ordinary file is an ordinary file.
        assert_eq!(denied(Path::new("/work/api/src/lib.rs")), None);
        assert_eq!(denied(Path::new("/work/api/README.md")), None);
        // `.` and `..` are not hidden files.
        assert_eq!(denied(Path::new("/work/api/.")), None);
    }

    #[test]
    fn a_refusal_names_the_rule_rather_than_the_path() {
        assert!(Denied::Name("*.pem").reason().contains("*.pem"));
        assert!(Denied::Directory(".ssh").reason().contains(".ssh"));
        assert!(Denied::Hidden.reason().contains("hidden"));
    }

    /// Two spellings of one place compare equal.
    ///
    /// `canonical` returns a path unchanged when it does not resolve, and on
    /// Windows returns the verbatim `\\?\C:\…` form when it does — so a root
    /// that exists and a file that does not were compared in two different
    /// spellings and never matched.
    #[test]
    fn a_verbatim_root_and_a_plain_path_are_the_same_place() {
        // Through `within_boundary`, because the stripping and the comparison
        // together are the thing under test. None of these paths exists, so
        // `canonical` hands each back unchanged and what reaches `under` is
        // exactly the two spellings a Windows machine produces.
        let plain_root = vec![PathBuf::from(r"C:\work\api")];
        let verbatim_root = vec![PathBuf::from(r"\\?\C:\work\api")];
        assert!(within_boundary(
            Path::new(r"\\?\C:\work\api\src\lib.rs"),
            &plain_root
        ));
        assert!(within_boundary(
            Path::new(r"C:\work\api\src\lib.rs"),
            &verbatim_root
        ));
        // A sibling whose name begins with the root's is not under it.
        assert!(!within_boundary(
            Path::new(r"\\?\C:\work\api-old\lib.rs"),
            &plain_root
        ));

        // The segment rule itself, on the form the comparison sees.
        assert!(under("/work/api/src/lib.rs", "/work/api"));
        assert!(under("/work/api", "/work/api"));
        assert!(under("/work/api/", "/work/api"));
        assert!(!under("/work/api-old/src/lib.rs", "/work/api"));
    }

    #[test]
    fn a_store_with_roots_is_confined_to_them() {
        let roots = vec![PathBuf::from("/work/api")];
        assert!(within_boundary(Path::new("/work/api/src/lib.rs"), &roots));

        // A machine with no home semlith can locate — a Windows runner, which
        // sets no HOME — is one where the fallback has nothing to fall back
        // to, and the rule under test is the roots one either way.
        let Ok(home) = crate::home::user_home() else {
            assert!(!within_boundary(
                Path::new("/somewhere/else/lib.mjs"),
                &roots
            ));
            return;
        };
        let elsewhere = home.join("elsewhere").join("lib.mjs");

        // The home directory is not a licence to index into a store that has
        // roots of its own. This is finding 1.1 of the 2026-09-17 drive: every
        // store on a machine shares a home, so treating it as inside every
        // boundary made the boundary no boundary at all.
        assert!(!within_boundary(&elsewhere, &roots));

        // With no roots recorded at all, the home directory is the boundary,
        // because otherwise a `--store` the registry never saw could index
        // nothing whatever.
        assert!(within_boundary(&elsewhere, &[]));
    }

    #[test]
    fn a_path_under_no_root_is_outside_the_boundary() {
        let roots = vec![PathBuf::from("/work/api")];
        assert!(within_boundary(Path::new("/work/api/src/lib.rs"), &roots));
        assert!(within_boundary(Path::new("/work/api"), &roots));
        assert!(!within_boundary(Path::new("/etc/hosts"), &roots));
        assert!(!within_boundary(Path::new("/work/other"), &roots));
    }

    /// Resolved once, asked many times: checking N paths against R roots costs
    /// N canonicalisations for the paths and R for the roots, not N × (R + 1).
    #[test]
    fn roots_are_resolved_once_however_many_paths_are_checked() {
        let roots = vec![
            PathBuf::from("/work/api"),
            PathBuf::from("/work/web"),
            PathBuf::from("/work/docs"),
        ];
        let before = crate::canonical_calls();
        let resolved = resolve_boundary(&roots);
        for i in 0..100 {
            assert!(within_resolved(
                Path::new(&format!("/work/docs/{i}.md")),
                &resolved
            ));
        }
        assert!(!within_resolved(Path::new("/etc/hosts"), &resolved));
        assert_eq!(crate::canonical_calls() - before, 3 + 101);

        // And one call on its own resolves each root once.
        let before = crate::canonical_calls();
        assert!(!within_boundary(Path::new("/etc/hosts"), &roots));
        assert_eq!(crate::canonical_calls() - before, 3 + 1);
    }
}

/// One credential shape the content scan looks for.
///
/// A row is a name, a pattern, a string that must match it and a string that
/// must not. The last two are not documentation: `tests/scan.rs` walks this
/// table and fails if a row has an example it does not match or a near miss it
/// does, so a pattern typo is a failing build rather than a credential in a
/// store.
pub struct Shape {
    /// What the match is called, in the line a user reads.
    pub kind: &'static str,
    /// Who issues it, as the redaction marker names it: `[REDACTED:aws …]`.
    pub provider: &'static str,
    /// What it is, in two or three words, for the same marker.
    pub label: &'static str,
    /// How many leading characters are the issuer's prefix rather than the
    /// secret — `ghp_`, `AKIA` — which is what a mask shows and what the
    /// dummy rules look past.
    pub prefix_len: usize,
    /// The regular expression. Anchored by the credential's own prefix
    /// wherever there is one, because a prefix is the issuer declaring what
    /// the string is — which is what makes these safe to refuse on sight.
    pub pattern: &'static str,
    /// A string of this shape that is a declared test dummy (see
    /// `keyscan::dummy_rule`): it matches the pattern and is let through.
    /// From 0.30.0 no row carries a live-looking literal; the tests that need
    /// one build it at run time with `keyscan::forge`.
    pub example: &'static str,
    /// A string that looks like it but is not: the right prefix and the wrong
    /// length, usually. What stops a pattern being widened by accident.
    pub near_miss: &'static str,
}

/// Every credential shape semlith refuses a file for.
///
/// Some examples are written as `concat!` of two halves. A table of credential
/// shapes is the one file that will hold fourteen credential-shaped literals,
/// and GitHub's own push protection refuses a branch that contains them — it
/// stopped 0.19.0's first push over the Slack and Twilio rows. Splitting the
/// literal keeps the source free of a contiguous match while the constant it
/// compiles to is exactly the string the tests assert against.
///
/// Prefix-declared, not entropy-guessed, with one exception at the bottom.
/// Until 0.30.0 there was no allow-list at all, and a documentation page that
/// quoted AWS's own `AKIAIOSFODNN7EXAMPLE` was refused like a leaked key —
/// eight of this repository's own files were. What lets a match through now is
/// a short set of declared rules about the value itself, in `keyscan.rs`, and
/// one live-looking match anywhere still refuses the whole file.
pub const SHAPES: &[Shape] = &[
    // Anthropic before OpenAI: `sk-ant-` is an `sk-` too, and the first match
    // is the one named.
    Shape {
        kind: "an Anthropic API key",
        provider: "anthropic",
        label: "api key",
        prefix_len: 7,
        pattern: r"(?-u:\b)sk-ant-[A-Za-z0-9_-]{24,}",
        example: "sk-ant-api03-EXAMPLEEXAMPLEEXAMPLEEXAMPLE",
        near_miss: "sk-ant-short",
    },
    // OpenRouter before OpenAI too: `sk-or-v1-` is an `sk-`. 64 hex after the
    // prefix (trufflehog's openrouter detector).
    Shape {
        kind: "an OpenRouter API key",
        provider: "openrouter",
        label: "api key",
        prefix_len: 9,
        pattern: r"(?-u:\b)sk-or-v1-[0-9a-f]{64}(?-u:\b)",
        example: concat!(
            "sk-",
            "or-v1-0000000000000000000000000000000000000000000000000000000000000000"
        ),
        near_miss: "sk-or-v1-0123abcd",
    },
    Shape {
        kind: "an OpenAI API key",
        provider: "openai",
        label: "api key",
        prefix_len: 3,
        pattern: r"(?-u:\b)sk-[A-Za-z0-9_-]{32,}",
        example: "sk-proj-EXAMPLEEXAMPLEEXAMPLEEXAMPLE1234",
        near_miss: "sk-tooshort",
    },
    // Base32 after the prefix, as AWS issues them and gitleaks' aws-access-token
    // rule reads them: a 0, 1, 8 or 9 is a hand-made fixture (0.51.0).
    Shape {
        kind: "an AWS access key id",
        provider: "aws",
        label: "access key id",
        prefix_len: 4,
        pattern: r"(?-u:\b)(?:AKIA|ASIA)[A-Z2-7]{16}(?-u:\b)",
        example: concat!("AKIAIOSFODNN7", "EXAMPLE"),
        near_miss: "AKIAIOSFODNN7EXAMPL",
    },
    Shape {
        kind: "a GitHub token",
        provider: "github",
        label: "token",
        prefix_len: 4,
        pattern: r"(?-u:\b)gh[pousr]_[A-Za-z0-9]{36}(?-u:\b)",
        example: "ghp_EXAMPLEaaaaBBBBccccDDDDeeeeFFFF12345",
        near_miss: "ghp_tooshortforatoken",
    },
    Shape {
        kind: "a GitHub fine-grained token",
        provider: "github",
        label: "fine-grained token",
        prefix_len: 11,
        pattern: r"(?-u:\b)github_pat_[A-Za-z0-9_]{40,}",
        example: "github_pat_11EXAMPLE0aaaaaaaaaaaa_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        near_miss: "github_pat_11AAAAAAA0",
    },
    Shape {
        kind: "a Slack token",
        provider: "slack",
        label: "token",
        prefix_len: 5,
        pattern: r"(?-u:\b)xox[abprs]-[0-9A-Za-z-]{12,}",
        example: concat!(
            "xox",
            "b-123456789012-1234567890123-EXAMPLEaaaaaaaaaaaaaaaaa"
        ),
        near_miss: "xoxb-123",
    },
    // Test keys as well as live ones. A `sk_test_` is not a credential in the
    // sense a live key is, and refusing it costs a documentation page — but
    // the alternative is a table with an exception in it, and an exception is
    // the thing that is wrong the day somebody pastes a live key into a file
    // called `test.md`. `--include-secrets` is the way past, as it is for a
    // `.env`.
    Shape {
        kind: "a Stripe key",
        provider: "stripe",
        label: "key",
        prefix_len: 8,
        pattern: r"(?-u:\b)[sr]k_(?:live|test)_[0-9A-Za-z]{16,}",
        example: "sk_live_EXAMPLEaaaaBBBB1234",
        near_miss: "sk_live_tooshort",
    },
    Shape {
        kind: "a Google API key",
        provider: "google",
        label: "api key",
        prefix_len: 4,
        pattern: r"(?-u:\b)AIza[0-9A-Za-z_-]{35}(?-u:\b)",
        example: concat!("AIza", "SyEXAMPLEaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        near_miss: "AIzaSyA0aaaaaaaaaaaa",
    },
    Shape {
        kind: "a Twilio API key",
        provider: "twilio",
        label: "api key",
        prefix_len: 2,
        pattern: r"(?-u:\b)SK[0-9a-fA-F]{32}(?-u:\b)",
        example: concat!("SK", "00000000000000000000000000000000"),
        near_miss: "SK0123456789abcdef",
    },
    Shape {
        kind: "a SendGrid API key",
        provider: "sendgrid",
        label: "api key",
        prefix_len: 3,
        pattern: r"(?-u:\b)SG\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}",
        example: "SG.EXAMPLEaaaaaaaaaaaaaa.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        near_miss: "SG.aaaa.bbbb",
    },
    Shape {
        kind: "an npm token",
        provider: "npm",
        label: "token",
        prefix_len: 4,
        pattern: r"(?-u:\b)npm_[A-Za-z0-9]{36}(?-u:\b)",
        example: "npm_EXAMPLEaaaaBBBBccccDDDDeeeeFFFF12345",
        near_miss: "npm_install",
    },
    Shape {
        kind: "a semlith agent key",
        provider: "semlith",
        label: "agent key",
        prefix_len: 4,
        pattern: r"(?-u:\b)sml_[0-9a-f]{64}(?-u:\b)",
        example: "sml_0000000000000000000000000000000000000000000000000000000000000000",
        near_miss: "sml_0123456789abcdef",
    },
    // 0.51.0's named formats. Each length and alphabet is the one gitleaks
    // 8.30.1's rule of the same name uses (config/gitleaks.toml at 5f6ec56),
    // or trufflehog's detector at 421026a where gitleaks has none.
    // gitleaks huggingface-access-token: 34 letters.
    Shape {
        kind: "a Hugging Face token",
        provider: "huggingface",
        label: "token",
        prefix_len: 3,
        pattern: r"(?-u:\b)hf_[A-Za-z]{34}(?-u:\b)",
        example: concat!("hf_", "EXAMPLEaaaaBBBBccccDDDDeeeeFFFFggg"),
        near_miss: "hf_hub_download",
    },
    // gitleaks gitlab-pat, gitlab-pat-routable, gitlab-oauth-app-secret,
    // gitlab-ptt.
    Shape {
        kind: "a GitLab token",
        provider: "gitlab",
        label: "token",
        prefix_len: 6,
        pattern: r"(?-u:\b)(?:glpat-[A-Za-z0-9_-]{20,}(?:\.[0-9a-z]{9})?|gloas-[A-Za-z0-9_-]{64}|glptt-[0-9a-f]{40}(?-u:\b))",
        example: concat!("glp", "at-EXAMPLEaaaaBBBBccccDDDDe"),
        near_miss: "glpat-short",
    },
    // gitleaks vault-service-token (hvs., 90 or more) and vault-batch-token
    // (hvb., 138 or more); recovery tokens (hvr.) share the alphabet.
    Shape {
        kind: "a HashiCorp Vault token",
        provider: "vault",
        label: "token",
        prefix_len: 4,
        pattern: r"(?-u:\b)hv[sbr]\.[A-Za-z0-9_-]{24,}",
        example: concat!("hvs", ".EXAMPLEaaaaBBBBccccDDDDeeee"),
        near_miss: "hvs.short",
    },
    // trufflehog groq: 52 alphanumerics.
    Shape {
        kind: "a Groq API key",
        provider: "groq",
        label: "api key",
        prefix_len: 4,
        pattern: r"(?-u:\b)gsk_[A-Za-z0-9]{52}(?-u:\b)",
        example: concat!(
            "gsk",
            "_EXAMPLEaaaaBBBBccccDDDDeeeeFFFFggggHHHHiiiiJJJJkkkkL"
        ),
        near_miss: "gsk_short",
    },
    // gitleaks perplexity-api-key: 48 alphanumerics.
    Shape {
        kind: "a Perplexity API key",
        provider: "perplexity",
        label: "api key",
        prefix_len: 5,
        pattern: r"(?-u:\b)pplx-[A-Za-z0-9]{48}(?-u:\b)",
        example: concat!("ppl", "x-EXAMPLEaaaaBBBBccccDDDDeeeeFFFFggggHHHHiiiiJJJJk"),
        near_miss: "pplx-short",
    },
    // trufflehog replicate: 37 of [A-Za-z0-9_-]. No closing `(?-u:\b)`: a body
    // that ends on `-` would fail it and the key would go unseen.
    Shape {
        kind: "a Replicate API token",
        provider: "replicate",
        label: "api token",
        prefix_len: 3,
        pattern: r"(?-u:\b)r8_[A-Za-z0-9_-]{37}",
        example: concat!("r8_", "EXAMPLEaaaaBBBBccccDDDDeeeeFFFFggggHH"),
        near_miss: "r8_short",
    },
    // trufflehog xai: 80 of [A-Za-z0-9_].
    Shape {
        kind: "an xAI API key",
        provider: "xai",
        label: "api key",
        prefix_len: 4,
        pattern: r"(?-u:\b)xai-[A-Za-z0-9_]{80}(?-u:\b)",
        example: concat!(
            "xai",
            "-EXAMPLEaaaaBBBBccccDDDDeeeeFFFFggggHHHHiiiiJJJJkkkkLLLLmmmmNNNNooooPPPPqqqqRRRRs"
        ),
        near_miss: "xai-short",
    },
    // trufflehog langsmith: 32 hex, underscore, 10 hex.
    Shape {
        kind: "a LangSmith API key",
        provider: "langsmith",
        label: "api key",
        prefix_len: 8,
        pattern: r"(?-u:\b)lsv2_(?:pt|sk)_[0-9a-f]{32}_[0-9a-f]{10}(?-u:\b)",
        example: concat!("lsv", "2_pt_00000000000000000000000000000000_0000000000"),
        near_miss: "lsv2_pt_0123",
    },
    // trufflehog pinecone: a 5 or 6 character key id, then 63.
    Shape {
        kind: "a Pinecone API key",
        provider: "pinecone",
        label: "api key",
        prefix_len: 5,
        pattern: r"(?-u:\b)pcsk_[A-Za-z0-9]{5,6}_[A-Za-z0-9]{63}(?-u:\b)",
        example: concat!(
            "pcs",
            "k_EXAMP_EXAMPLEaaaaBBBBccccDDDDeeeeFFFFggggHHHHiiiiJJJJkkkkLLLLmmmmNNNN"
        ),
        near_miss: "pcsk_short",
    },
    // gitleaks digitalocean-pat, -access-token, -refresh-token: 64 hex.
    Shape {
        kind: "a DigitalOcean token",
        provider: "digitalocean",
        label: "token",
        prefix_len: 7,
        pattern: r"(?-u:\b)do[por]_v1_[0-9a-f]{64}(?-u:\b)",
        example: concat!(
            "dop",
            "_v1_0000000000000000000000000000000000000000000000000000000000000000"
        ),
        near_miss: "dop_v1_0123",
    },
    // gitleaks databricks-api-token: 32 hex, an optional `-<digit>`.
    Shape {
        kind: "a Databricks token",
        provider: "databricks",
        label: "token",
        prefix_len: 4,
        pattern: r"(?-u:\b)dapi[0-9a-f]{32}(?:-\d)?(?-u:\b)",
        example: concat!("dap", "i00000000000000000000000000000000"),
        near_miss: "dapi0123",
    },
    // gitleaks pypi-upload-token: the fixed macaroon header, then 50 or more.
    Shape {
        kind: "a PyPI upload token",
        provider: "pypi",
        label: "upload token",
        prefix_len: 20,
        pattern: r"(?-u:\b)pypi-AgEIcHlwaS5vcmc[A-Za-z0-9_-]{50,}",
        example: concat!(
            "pyp",
            "i-AgEIcHlwaS5vcmcEXAMPLEaaaaBBBBccccDDDDeeeeFFFFggggHHHHiiiiJJJJkkkkLLLLm"
        ),
        near_miss: "pypi-AgEIcHlwaS5vcmcshort",
    },
    // gitleaks shopify-*-access-token and shopify-shared-secret: 32 hex.
    Shape {
        kind: "a Shopify token",
        provider: "shopify",
        label: "token",
        prefix_len: 6,
        pattern: r"(?-u:\b)shp(?:at|ca|pa|ss)_[a-fA-F0-9]{32}(?-u:\b)",
        example: concat!("shp", "at_00000000000000000000000000000000"),
        near_miss: "shpat_0123",
    },
    // gitleaks linear-api-key: 40 alphanumerics.
    Shape {
        kind: "a Linear API key",
        provider: "linear",
        label: "api key",
        prefix_len: 8,
        pattern: r"(?-u:\b)lin_api_[A-Za-z0-9]{40}(?-u:\b)",
        example: concat!("lin", "_api_EXAMPLEaaaaBBBBccccDDDDeeeeFFFFggggHHHHi"),
        near_miss: "lin_api_short",
    },
    // gitleaks doppler-api-token: 43 alphanumerics.
    Shape {
        kind: "a Doppler token",
        provider: "doppler",
        label: "token",
        prefix_len: 6,
        pattern: r"(?-u:\b)dp\.pt\.[A-Za-z0-9]{43}(?-u:\b)",
        example: concat!("dp.", "pt.EXAMPLEaaaaBBBBccccDDDDeeeeFFFFggggHHHHiiii"),
        near_miss: "dp.pt.short",
    },
    // gitleaks azure-ad-client-secret: three, a digit, `Q~`, 31 to 34. The
    // secret is the group; the characters around it only bound it.
    Shape {
        kind: "an Azure AD client secret",
        provider: "azure",
        label: "client secret",
        prefix_len: 0,
        pattern: r#"(?m)(?:^|[\\'"`\s>=:(,)])([a-zA-Z0-9_~.]{3}\dQ~[a-zA-Z0-9_~.-]{31,34})(?:$|[\\'"`\s<),])"#,
        example: concat!("sec", "ret: abc8Q~EXAMPLEaaaaBBBBccccDDDDeeeeFFFFg\n"),
        near_miss: "abc8Q~short",
    },
    // trufflehog slackwebhook and gitleaks slack-webhook-url.
    Shape {
        kind: "a Slack webhook URL",
        provider: "slack",
        label: "webhook",
        prefix_len: 0,
        pattern: r"https://hooks\.slack\.com/(?:services|workflows|triggers)/[A-Za-z0-9+/]{43,56}",
        example: concat!(
            "https://hooks.slack.com/",
            "services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXXXXXX"
        ),
        near_miss: "https://hooks.slack.com/services/",
    },
    // trufflehog discordwebhook: an 17-20 digit id and a 68 character token.
    Shape {
        kind: "a Discord webhook URL",
        provider: "discord",
        label: "webhook",
        prefix_len: 0,
        pattern: r"https://(?:(?:ptb|canary)\.)?discord(?:app)?\.com/api/webhooks/[0-9]{17,20}/[A-Za-z0-9_-]{60,}",
        example: concat!(
            "https://discord.com/api/webhooks/",
            "100000000000000000/EXAMPLEaaaaBBBBccccDDDDeeeeFFFFggggHHHHiiiiJJJJkkkkLLLLmmmmNNNNoooo"
        ),
        near_miss: "https://discord.com/api/webhooks/123/abc",
    },
    // trufflehog discordbottoken: 24.6.27-38 of base64url. No prefix, so the
    // scan also checks the first part decodes to a user id (17 to 20 digits),
    // which is what a bot token starts with; anything else is not one. Base64
    // of a number opens on one of nine pairs, which the pattern spells so the
    // regex engine can skip to them.
    Shape {
        kind: "a Discord bot token",
        provider: "discord",
        label: "bot token",
        prefix_len: 0,
        pattern: r"(?-u:\b)(?:M[TDjz]|N[DTjz]|O[DT])[A-Za-z0-9_-]{22,24}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,38}",
        example: concat!(
            "MTA",
            "wMDAwMDAwMDAwMDAwMDAw.EXAMPL.EXAMPLEaaaaBBBBccccDDDDeeeeF"
        ),
        near_miss: "MTAwMDAwMDAwMDAwMDAwMDAw.short",
    },
    // `BLOCK` for PGP's armour, `-----BEGIN PGP PRIVATE KEY BLOCK-----`.
    Shape {
        kind: "a private key block",
        provider: "pem",
        label: "private key",
        prefix_len: 0,
        pattern: r"-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY(?: BLOCK)?-----",
        example: "-----BEGIN RSA PRIVATE KEY-----",
        near_miss: "-----BEGIN CERTIFICATE-----",
    },
    // A PEM block base64-encoded, as a Kubernetes Secret or an environment
    // variable holds one: `LS0tLS1CRUdJTi` is `-----BEGIN`. The scan decodes
    // it and keeps only a private key; an encoded certificate is not one.
    Shape {
        kind: "a base64-encoded private key",
        provider: "pem",
        label: "encoded private key",
        prefix_len: 0,
        pattern: r"(?-u:\b)LS0tLS1CRUdJTi[A-Za-z0-9+/]{20,}={0,2}",
        example: concat!(
            "LS0",
            "tLS1CRUdJTiBSU0EgUFJJVkFURSBLRVktLS0tLQotLS0tLUVORCBSU0EgUFJJVkFURSBLRVktLS0tLQo="
        ),
        near_miss: "LS0tLS1CRUdJTiBDRVJUSUZJQ0FURS0tLS0tCk1JSUJzekNDQVZtZ0F3SUJBZ0lVCi0tLS0tRU5EIENFUlRJRklDQVRFLS0tLS0K",
    },
    // PuTTY's own format, through its base64 private lines, its line breaks
    // real or escaped as a string escapes them.
    Shape {
        kind: "a PuTTY private key",
        provider: "pem",
        label: "putty key",
        prefix_len: 0,
        pattern: r"PuTTY-User-Key-File-[23]: [^\r\n\\]*(?:(?:\r?\n|\\r\\n|\\n)[^\r\n\\]*){0,8}?(?:\r?\n|\\r\\n|\\n)Private-Lines: \d+(?:(?:\r?\n|\\r\\n|\\n)[A-Za-z0-9+/=]+)+",
        example: concat!(
            "PuT",
            "TY-User-Key-File-3: ssh-rsa\nEncryption: none\nPrivate-Lines: 1\nAAAA"
        ),
        near_miss: "PuTTY-User-Key-File-3: ssh-rsa",
    },
    Shape {
        kind: "a JSON web token",
        provider: "jwt",
        label: "token",
        prefix_len: 3,
        pattern: r"(?-u:\b)eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.(?:[A-Za-z0-9_-]{10,})?",
        example: "eyJFAKEaaaaaaaaaa.eyJFAKEbbbbbbbbbb.FAKEcccccccccccc",
        near_miss: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0",
    },
    // The two below are judged on their secret part only, which is the
    // capture group and all a match or a redaction covers. Last, so a token
    // with a prefix of its own inside them is named by its own row.
    // gitleaks has no rule for these; the scheme list is the one the contract
    // names. The password is judged as the generic rule judges a value.
    Shape {
        kind: "a connection string with a password",
        provider: "connection",
        label: "password",
        prefix_len: 0,
        pattern: r#"(?-u:\b)(?:postgres(?:ql)?|mysql|mariadb|mongodb(?:\+srv)?|rediss?|amqps?|mssql|sqlserver)://[^\s:/@'"`]*:([^\s@'"`/]+)@"#,
        example: concat!("pos", "tgres://app:EXAMPLE0password0EXAMPLE@localhost/app"),
        near_miss: "postgres://app@localhost/app",
    },
    // gitleaks curl-auth-header covers curl only; this is any header line.
    Shape {
        kind: "a token in an Authorization header",
        provider: "header",
        label: "authorization",
        prefix_len: 0,
        pattern: r#"(?i-u:\bauthorization)["']?[ \t]*[:=][ \t]*["']?(?i-u:bearer|token|basic)[ \t]+([A-Za-z0-9._~+/=-]{12,})"#,
        example: concat!("Aut", "horization: Bearer EXAMPLE0token0EXAMPLE"),
        near_miss: "Authorization: Bearer ${TOKEN}",
    },
];

/// The generic rule: a key-like name assigned a long, high-entropy literal.
///
/// The value may not cross a line, and the name and the value may not be
/// separated by one. Without that, `split_once("/?token=")` and a quote two
/// lines later are one match whose "value" is the code in between — which is
/// what the 0.19.0 false-positive audit found in this repository's own
/// `tests/daemon.rs`. A credential is written on one line.
///
/// The one shape with no issuer prefix, so it needs two things at once. The
/// name alone catches `password = "hunter2"`, which is not a credential worth
/// refusing a file for; the entropy alone catches every base64 fixture and
/// every lockfile hash in the corpus. Both together is the rule.
///
/// From 0.51.0 the pattern only finds candidates: group 1 is the name and
/// group 2 the value, and `keyscan` reads the name as words and the value by
/// its shape. The substring list here is wider than the words that count, so
/// `tokens` and `author` reach the word test and fail it there. A name may be
/// quoted (`"api_key": "…"`) and the operator may be `:=` or `=>`.
pub(crate) const ASSIGNMENT: &str = r#"(?i-u:([a-z0-9_.-]*(?:key|secret|token|passw|passphrase|pwd|auth|cred|pat\b|bearer)[a-z0-9_.-]*))["']?[ \t]*(?::=|=>|[:=])[ \t]*["']([^"'\r\n]{12,})["']"#;

/// The same rule for a value written without quotes, as a `.env`, a shell
/// `export`, a YAML file or a compose list writes one (0.30.0, 2.2).
///
/// Anchored to a whole line, a trailing `# comment` aside, so
/// `let token = compute_token(input);` is not a match. From 0.51.0 it does
/// not run on code at all (`keyscan::unquoted_applies`), and the value may
/// hold any character but whitespace and quotes, as a password does.
pub(crate) const ENV_ASSIGNMENT: &str = r#"(?m)^[ \t]*(?i-u:(?:export[ \t]+|-[ \t]+)?([a-z0-9_.-]*(?:key|secret|token|passw|passphrase|pwd|auth|cred|pat\b|bearer)[a-z0-9_.-]*))[ \t]*[:=][ \t]*([^\s"'`]{12,})(?:[ \t]+#[^\r\n]*)?[ \t]*\r?$"#;

/// How much entropy a generic literal needs before it reads as a credential.
///
/// Shannon bits per character. A random 24-character token sits near 4.5; a
/// placeholder like `changeme_changeme_changeme` sits near 2.5. Measured
/// against both corpora in the 0.19.0 false-positive audit.
pub(crate) const MIN_ENTROPY: f64 = 3.2;

/// What the scan found, and where.
///
/// The line, never the text. No character of a matched credential is written
/// anywhere semlith writes: not here, not in an event, not in the CLI's
/// output, not in the store, not in a log. A scan that quotes the secret it
/// found has moved the secret rather than refused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub kind: String,
    pub line: u32,
}

impl Found {
    /// The line a user reads, naming the kind and the place.
    pub fn reason(&self) -> String {
        format!(
            "holds what looks like {} at line {} — semlith does not index it",
            self.kind, self.line
        )
    }
}

/// The compiled table, built once: one regex per row, and the two
/// assignment rules.
pub(crate) fn shapes() -> &'static (Vec<regex::Regex>, regex::Regex, regex::Regex) {
    static COMPILED: std::sync::OnceLock<(Vec<regex::Regex>, regex::Regex, regex::Regex)> =
        std::sync::OnceLock::new();
    COMPILED.get_or_init(|| {
        let each = SHAPES
            .iter()
            .map(|s| regex::Regex::new(s.pattern).expect("the credential shapes compile"))
            .collect();
        let assignment = regex::Regex::new(ASSIGNMENT).expect("the assignment rule compiles");
        let env = regex::Regex::new(ENV_ASSIGNMENT).expect("the assignment rule compiles");
        (each, assignment, env)
    })
}

/// The first live-looking credential this text holds, if any.
///
/// Called on the text a reader produced, before it is chunked, stored or
/// embedded — a file semlith refuses is a file whose contents never reach the
/// store in the first place. A match that is a declared test dummy does not
/// count; see `keyscan::scan` for every match with its verdict.
pub fn scan_text(text: &str) -> Option<Found> {
    crate::keyscan::scan("", text)
        .into_iter()
        .find(|m| m.dummy.is_none())
        .map(|m| Found {
            kind: m.kind,
            line: m.line,
        })
}

/// Whether a value is a placeholder standing in for a credential rather than
/// one.
///
/// The 0.19.0 false-positive audit found this on the first corpus it ran over:
/// semlith's own `docs/clients.md` carries `Authorization = "Bearer
/// ${SEMLITH_AGENT_KEY}"`, which is a key-like name assigned twenty characters
/// of respectable entropy and is the exact opposite of a leaked credential —
/// it is the documentation for how not to write one down.
///
/// A rule about shape, not a list of known-fake values. A template reference,
/// an angle-bracket placeholder and a bare `SCREAMING_SNAKE` variable name are
/// all things a credential is never spelled as: a real key mixes case or digits
/// in a way a variable name does not. The prefixed shapes in [`SHAPES`] get no
/// such exemption — an `AKIA…` is an AWS key wherever it is written.
pub(crate) fn is_placeholder(value: &str) -> bool {
    if value.contains("${") || value.contains("{{") || value.contains("<") || value.contains("%(") {
        return true;
    }
    // A variable name rather than a value: upper case, digits, underscores and
    // nothing else, optionally with a word in front of it.
    value.split_whitespace().last().is_some_and(|last| {
        !last.is_empty()
            && last
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    })
}

/// Shannon entropy in bits per character.
pub(crate) fn entropy(value: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    for c in value.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let total = value.chars().count() as f64;
    if total == 0.0 {
        return 0.0;
    }
    -counts
        .values()
        .map(|n| {
            let p = *n as f64 / total;
            p * p.log2()
        })
        .sum::<f64>()
}

/// Which line a byte offset falls on, counting from one.
pub(crate) fn line_of(text: &str, at: usize) -> u32 {
    text[..at].bytes().filter(|b| *b == b'\n').count() as u32 + 1
}
