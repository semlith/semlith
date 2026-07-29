//! Narrowing a search to part of the corpus.
//!
//! A [`Filter`] is resolved to SQLite `GLOB` patterns and then to a set of
//! chunk ids by [`crate::store::filtered_chunk_ids`]. That one id set drives
//! both halves of the hybrid search — the allowlist handed to the vector index
//! and the predicate inside the FTS5 query — so the two can never disagree
//! about which chunks were eligible.

use anyhow::{Result, bail};

/// Extensions that make up each language name accepted by `--lang`.
///
/// Extension is the only signal. Reading file contents to tell Perl from
/// anything else is a research project, and a store is queried far more often
/// than it is built, so guessing at query time would cost on every search.
pub const LANGUAGES: &[(&str, &[&str])] = &[
    ("c", &["c", "h"]),
    ("cpp", &["cc", "cpp", "cxx", "hh", "hpp", "hxx"]),
    ("csharp", &["cs"]),
    ("css", &["css", "sass", "scss"]),
    ("go", &["go"]),
    ("haskell", &["hs"]),
    ("html", &["htm", "html"]),
    ("java", &["java"]),
    ("javascript", &["cjs", "js", "jsx", "mjs"]),
    ("json", &["json"]),
    ("kotlin", &["kt", "kts"]),
    ("lua", &["lua"]),
    ("markdown", &["markdown", "md"]),
    ("ocaml", &["ml", "mli"]),
    ("php", &["php"]),
    ("python", &["py", "pyi"]),
    ("ruby", &["rb"]),
    ("rust", &["rs"]),
    ("scala", &["sc", "scala"]),
    ("shell", &["bash", "sh", "zsh"]),
    ("sql", &["sql"]),
    ("swift", &["swift"]),
    ("toml", &["toml"]),
    ("typescript", &["cts", "mts", "ts", "tsx"]),
    ("yaml", &["yaml", "yml"]),
];

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
        let mut extensions: Vec<String> = exts
            .iter()
            .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
            .collect();
        for lang in langs {
            let want = lang.to_ascii_lowercase();
            let Some((_, exts)) = LANGUAGES.iter().find(|(name, _)| *name == want) else {
                bail!("unknown language {lang:?}; run `semlith languages` for the list");
            };
            extensions.extend(exts.iter().map(|e| e.to_string()));
        }
        if !extensions.is_empty() {
            groups.push(
                extensions
                    .iter()
                    .map(|e| anchor(&format!("*.{e}")))
                    .collect(),
            );
        }

        Ok(Self { groups })
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// AND-groups of OR-patterns, ready for the SQL builder.
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
    let pattern = pattern.to_lowercase();
    // ponytail: paths on Windows are stored with backslashes, so a pattern
    // written with forward slashes would match nothing. Translate rather than
    // asking the user to write a platform-specific glob.
    #[cfg(windows)]
    let pattern = pattern.replace('/', "\\");

    if is_absolute(&pattern) {
        pattern
    } else {
        format!("{}{pattern}", separator_prefix())
    }
}

fn is_absolute(pattern: &str) -> bool {
    if pattern.starts_with(std::path::MAIN_SEPARATOR) {
        return true;
    }
    // `c:\...` on Windows. Elsewhere a colon is an ordinary filename character.
    cfg!(windows) && pattern.as_bytes().get(1) == Some(&b':')
}

fn separator_prefix() -> String {
    format!("*{}", std::path::MAIN_SEPARATOR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| i.to_string()).collect()
    }

    /// The rule the README documents: a relative glob is anchored so it works
    /// from any directory; an absolute one is not, so it means exactly itself.
    #[test]
    fn a_relative_pattern_is_anchored_and_an_absolute_one_is_not() {
        let sep = std::path::MAIN_SEPARATOR;
        let f = Filter::new(&s(&["src/**"]), &[], &[]).unwrap();
        assert_eq!(f.groups(), [[format!("*{sep}src{sep}**")]]);

        let absolute = format!("{sep}home{sep}me{sep}proj{sep}src{sep}*");
        let f = Filter::new(&s(&[&absolute]), &[], &[]).unwrap();
        assert_eq!(f.groups(), [[absolute]]);
    }

    #[test]
    fn extensions_and_languages_share_one_group_and_union() {
        let sep = std::path::MAIN_SEPARATOR;
        let f = Filter::new(&[], &s(&["toml"]), &s(&["rust"])).unwrap();
        assert_eq!(f.groups().len(), 1, "one group means they union");
        assert_eq!(
            f.groups()[0],
            [format!("*{sep}*.toml"), format!("*{sep}*.rs")]
        );
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
        for (name, exts) in LANGUAGES {
            assert!(name > &previous, "language table is out of order at {name}");
            previous = name;
            assert_eq!(*name, name.to_ascii_lowercase());
            let mut sorted = exts.to_vec();
            sorted.sort_unstable();
            assert_eq!(&sorted, exts, "extensions for {name} are out of order");
        }
    }
}
