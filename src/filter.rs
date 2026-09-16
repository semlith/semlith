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
            groups.push(paths.iter().map(|p| anchor(p)).collect());
        }

        // Extensions and languages are both extension sets, so they share one
        // group: `--ext rs --lang markdown` means "Rust or Markdown", the same
        // way two `--ext` flags do.
        let mut patterns: Vec<String> = exts
            .iter()
            .map(|e| format!("*.{}", e.trim_start_matches('.').to_ascii_lowercase()))
            .collect();
        for name in langs {
            let Some(entry) = language(name) else {
                bail!("unknown language {name:?}; run `semlith languages` for the list");
            };
            patterns.extend(entry.extensions.iter().map(|e| format!("*.{e}")));
            // A filename pattern resolves into the same group as an extension
            // one, so `--lang dockerfile --ext md` unions exactly the way two
            // extensions do and nothing downstream has a second case to handle.
            patterns.extend(entry.filenames.iter().map(|f| f.to_string()));
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
fn anchor(pattern: &str) -> String {
    // Always `/`, on every platform. The query side compares against a path
    // whose separators have been normalised to `/` (`store::GLOB_PATH`), so a
    // pattern written the way everyone writes one matches a Windows store as
    // well as a unix one. Translating the pattern to backslashes instead —
    // which is what this did — matched nothing whenever the store held a path
    // in any other form, and the store holds the verbatim form (#74).
    let pattern = pattern.to_lowercase().replace('\\', "/");
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

/// Directories under the home whose contents are credentials, and nothing else.
///
/// Relative to the home directory, because that is where each of them lives and
/// where an agent asked to "index my config" would find them. A deny-list is a
/// blunt instrument and this one is deliberately short: every entry is a
/// directory whose whole purpose is to hold secrets, so refusing it costs a user
/// nothing they meant to index.
pub const DENIED_DIRS: &[&str] = &[
    ".ssh",
    ".aws",
    ".gnupg",
    ".kube",
    ".config/gcloud",
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
            Denied::Directory(dir) => {
                format!("under ~/{dir}, which holds credentials — semlith does not index it")
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
    let Some(home) = home else {
        return Some(Denied::NoHome);
    };
    let real = path;
    for dir in DENIED_DIRS {
        if real.starts_with(home.join(dir)) {
            return Some(Denied::Directory(dir));
        }
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
    let real = crate::canonical(path);
    if roots
        .iter()
        .any(|root| real.starts_with(crate::canonical(root)))
    {
        return true;
    }
    // An unknown home makes this stricter rather than looser: nothing is inside
    // a boundary semlith cannot locate.
    crate::home::user_home().is_ok_and(|home| real.starts_with(crate::canonical(&home)))
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
            Some(Denied::NoHome),
            "the credential itself must not slip past either"
        );
        let refusal = Denied::NoHome.reason();
        assert!(
            refusal.contains("home directory"),
            "the reason should say what semlith could not determine: {refusal}"
        );
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

    #[test]
    fn the_boundary_is_the_roots_and_the_home() {
        let roots = vec![PathBuf::from("/work/api")];
        assert!(within_boundary(Path::new("/work/api/src/lib.rs"), &roots));
        assert!(within_boundary(Path::new("/work/api"), &roots));
        assert!(!within_boundary(Path::new("/etc/hosts"), &roots));
        assert!(!within_boundary(Path::new("/work/other"), &roots));
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
    /// The regular expression. Anchored by the credential's own prefix
    /// wherever there is one, because a prefix is the issuer declaring what
    /// the string is — which is what makes these safe to refuse on sight.
    pub pattern: &'static str,
    /// A string of this shape. Fake, and refused all the same.
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
/// compiles to is exactly the string the tests assert against. Any row a
/// scanner flags gets the same treatment; the rest stay whole, because an
/// unnecessary split is a row that reads worse for nothing.
///
/// Prefix-declared, not entropy-guessed, with one exception at the bottom. A
/// documentation page that quotes AWS's own `AKIAIOSFODNN7EXAMPLE` is refused
/// like any other match and `--include-secrets` indexes it: an allow-list of
/// known-fake values is a second table to keep right, and a credential that
/// gets indexed because it resembled an example is the failure that matters.
pub const SHAPES: &[Shape] = &[
    // Anthropic before OpenAI: `sk-ant-` is an `sk-` too, and the first match
    // is the one named.
    Shape {
        kind: "an Anthropic API key",
        pattern: r"\bsk-ant-[A-Za-z0-9_-]{24,}",
        example: "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHH",
        near_miss: "sk-ant-short",
    },
    Shape {
        kind: "an OpenAI API key",
        pattern: r"\bsk-[A-Za-z0-9_-]{32,}",
        example: "sk-proj-AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIII",
        near_miss: "sk-tooshort",
    },
    Shape {
        kind: "an AWS access key id",
        pattern: r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        example: "AKIAIOSFODNN7EXAMPLE",
        near_miss: "AKIAIOSFODNN7EXAMPL",
    },
    Shape {
        kind: "a GitHub token",
        pattern: r"\bgh[pousr]_[A-Za-z0-9]{36}\b",
        example: "ghp_aaaaBBBBccccDDDDeeeeFFFFgggg12345678",
        near_miss: "ghp_tooshortforatoken",
    },
    Shape {
        kind: "a GitHub fine-grained token",
        pattern: r"\bgithub_pat_[A-Za-z0-9_]{40,}",
        example: "github_pat_11AAAAAAA0aaaaaaaaaaaa_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        near_miss: "github_pat_11AAAAAAA0",
    },
    Shape {
        kind: "a Slack token",
        pattern: r"\bxox[abprs]-[0-9A-Za-z-]{12,}",
        example: concat!("xox", "b-123456789012-1234567890123-aaaaaaaaaaaaaaaaaaaaaaaa"),
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
        pattern: r"\b[sr]k_(?:live|test)_[0-9A-Za-z]{16,}",
        example: "sk_live_aaaaBBBBccccDDDD1234",
        near_miss: "sk_live_tooshort",
    },
    Shape {
        kind: "a Google API key",
        pattern: r"\bAIza[0-9A-Za-z_-]{35}\b",
        example: "AIzaSyA0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        near_miss: "AIzaSyA0aaaaaaaaaaaa",
    },
    Shape {
        kind: "a Twilio API key",
        pattern: r"\bSK[0-9a-fA-F]{32}\b",
        example: concat!("SK", "0123456789abcdef0123456789abcdef"),
        near_miss: "SK0123456789abcdef",
    },
    Shape {
        kind: "a SendGrid API key",
        pattern: r"\bSG\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}",
        example: "SG.aaaaaaaaaaaaaaaaaaaaaa.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        near_miss: "SG.aaaa.bbbb",
    },
    Shape {
        kind: "an npm token",
        pattern: r"\bnpm_[A-Za-z0-9]{36}\b",
        example: "npm_aaaaBBBBccccDDDDeeeeFFFFgggg12345678",
        near_miss: "npm_install",
    },
    Shape {
        kind: "a semlith agent key",
        pattern: r"\bsml_[0-9a-f]{64}\b",
        example: "sml_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        near_miss: "sml_0123456789abcdef",
    },
    Shape {
        kind: "a private key block",
        pattern: r"-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----",
        example: "-----BEGIN RSA PRIVATE KEY-----",
        near_miss: "-----BEGIN CERTIFICATE-----",
    },
    Shape {
        kind: "a JSON web token",
        pattern: r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
        example: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        near_miss: "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0",
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
const ASSIGNMENT: &str = r#"(?i)[a-z0-9_-]*(?:api[_-]?key|secret|token|password|passwd|auth)[a-z0-9_-]*[ \t]*[:=][ \t]*["']([^"'\r\n]{20,})["']"#;

/// How much entropy a generic literal needs before it reads as a credential.
///
/// Shannon bits per character. A random 24-character token sits near 4.5; a
/// placeholder like `changeme_changeme_changeme` sits near 2.5. Measured
/// against both corpora in the 0.19.0 false-positive audit.
const MIN_ENTROPY: f64 = 3.2;

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

/// The compiled table, built once.
fn shapes() -> &'static (regex::RegexSet, Vec<regex::Regex>, regex::Regex) {
    static COMPILED: std::sync::OnceLock<(regex::RegexSet, Vec<regex::Regex>, regex::Regex)> =
        std::sync::OnceLock::new();
    COMPILED.get_or_init(|| {
        let set = regex::RegexSet::new(SHAPES.iter().map(|s| s.pattern))
            .expect("the credential shapes compile");
        let each = SHAPES
            .iter()
            .map(|s| regex::Regex::new(s.pattern).expect("the credential shapes compile"))
            .collect();
        let assignment = regex::Regex::new(ASSIGNMENT).expect("the assignment rule compiles");
        (set, each, assignment)
    })
}

/// The first credential this text looks like it holds, if any.
///
/// One `RegexSet` pass decides whether anything matched at all, which is the
/// answer for every file in a corpus but a handful; only then is the matching
/// shape run again to find where. Called on the text a reader produced, before
/// it is chunked, stored or embedded — a file semlith refuses is a file whose
/// contents never reach the store in the first place.
pub fn scan_text(text: &str) -> Option<Found> {
    let (set, each, assignment) = shapes();
    let mut best: Option<(usize, &'static str)> = None;
    for index in set.matches(text).iter() {
        if let Some(m) = each[index].find(text) {
            let at = m.start();
            if best.is_none_or(|(prev, _)| at < prev) {
                best = Some((at, SHAPES[index].kind));
            }
        }
    }
    if let Some(caps) = assignment.captures(text)
        && let Some(value) = caps.get(1)
        && !is_placeholder(value.as_str())
        && entropy(value.as_str()) >= MIN_ENTROPY
    {
        let at = caps.get(0).expect("the whole match").start();
        if best.is_none_or(|(prev, _)| at < prev) {
            best = Some((at, "a secret assigned to a key-like name"));
        }
    }
    let (at, kind) = best?;
    Some(Found {
        kind: kind.to_string(),
        line: line_of(text, at),
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
fn is_placeholder(value: &str) -> bool {
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
fn entropy(value: &str) -> f64 {
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
fn line_of(text: &str, at: usize) -> u32 {
    text[..at].bytes().filter(|b| *b == b'\n').count() as u32 + 1
}
