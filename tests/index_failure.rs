//! What 0.19.0 promises about a run that meets a file it cannot read: the run
//! finishes, the file is named, and every other file is indexed.
//!
//! Most of this needs no embedding model, because a file that is skipped,
//! refused or failed is one whose text never reaches the embedder — which is
//! also why these run in CI rather than behind `--ignored`. The two that do
//! embed something say so.

use semlith::{FileOutcome, Semlith};
use std::fs;
use std::path::{Path, PathBuf};

/// One `file` event, flattened to what the assertions are about.
#[derive(Debug, Clone)]
struct Event {
    path: PathBuf,
    outcome: FileOutcome,
    why: Option<String>,
    scanned: usize,
    total: usize,
}

fn store_at(dir: &Path) -> Semlith {
    let mut s = Semlith::open(dir, None).unwrap();
    s.quiet = true;
    s
}

fn write(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, body).unwrap();
    path
}

/// Index `corpus` into a fresh store and return the report with every event.
fn run(store: &Path, corpus: &Path) -> (semlith::IndexReport, Vec<Event>) {
    let mut s = store_at(store);
    let mut events = Vec::new();
    let report = s
        .index_paths(&[corpus.to_path_buf()], |path, p| {
            events.push(Event {
                path: path.to_path_buf(),
                outcome: p.outcome,
                why: p.why.clone(),
                scanned: p.scanned,
                total: p.total,
            });
        })
        .unwrap();
    (report, events)
}

/// The last event for a path, which is its verdict. A file that was embedded
/// is announced as `indexing` first, so the first event for it is a marker
/// rather than an answer.
fn find<'a>(events: &'a [Event], name: &str) -> &'a Event {
    events
        .iter()
        .rev()
        .find(|e| e.path.ends_with(name))
        .unwrap_or_else(|| {
            panic!(
                "no event for {name}; there were {:?}",
                events
                    .iter()
                    .map(|e| e.path.display().to_string())
                    .collect::<Vec<_>>()
            )
        })
}

/// A tree of files none of which can be embedded, so the whole run is the
/// decisions this release is about and no model is needed to see them.
fn undecidable_tree(corpus: &Path) {
    write(corpus, "empty.scss", b"");
    write(corpus, "pkg/__init__.py", b"");
    write(corpus, "blob.bin", &[0u8, 159, 146, 150, 0, 1, 2, 3]);
    // The whitelist the reporter's Angular tree had: ignored directory, one
    // file pulled back out of it by name.
    write(
        corpus,
        ".gitignore",
        b"dist/*\n!dist/.gitkeep\n!dist/.env\n",
    );
    write(corpus, "dist/.gitkeep", b"");
    write(corpus, "dist/bundle.js", b"console.log(1)\n");
    // Whitelisted the same way the `.gitkeep` is, so the walk yields it and
    // the credential rule is what has to stop it rather than the hidden rule.
    write(corpus, "dist/.env", b"AWS_SECRET_ACCESS_KEY=abcdef\n");
}

/// The reason for every skip, and the closed set holding.
#[test]
fn every_skipped_file_says_why() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    undecidable_tree(corpus.path());
    // Over the cap, so the reason is the size and not the content.
    write(corpus.path(), "huge.txt", &vec![b'a'; 9 * 1024 * 1024]);

    let (report, events) = run(store.path(), corpus.path());

    for event in &events {
        if matches!(
            event.outcome,
            FileOutcome::Skipped | FileOutcome::Refused | FileOutcome::Failed
        ) {
            assert!(
                event.why.as_deref().is_some_and(|w| !w.trim().is_empty()),
                "{} came back as {} with no reason",
                event.path.display(),
                event.outcome.as_str()
            );
        }
    }

    assert_eq!(find(&events, "empty.scss").why.as_deref(), Some("empty"));
    assert_eq!(find(&events, "__init__.py").why.as_deref(), Some("empty"));
    assert_eq!(find(&events, "blob.bin").why.as_deref(), Some("binary"));
    assert_eq!(find(&events, "huge.txt").why.as_deref(), Some("over 8 MiB"));

    // Counted by kind as well as in total, which is what turns a number
    // nobody can act on into a line nobody has to investigate.
    // `empty.scss`, `__init__.py` and the whitelisted `dist/.gitkeep`.
    assert_eq!(report.skipped_reasons.get("empty"), Some(&3));
    assert_eq!(report.skipped_reasons.get("binary"), Some(&1));
    assert_eq!(report.skipped_reasons.get("too large"), Some(&1));
    assert_eq!(
        report.skipped_reasons.values().sum::<usize>(),
        report.skipped,
        "the per-reason counts must add up to the total"
    );
}

/// The defect the Windows logs opened with: the walk yields a `.gitkeep` the
/// user's own `.gitignore` whitelisted, and the indexer then refuses it for
/// being hidden.
#[test]
fn a_whitelisted_dotfile_is_walked_and_not_refused_as_hidden() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    undecidable_tree(corpus.path());

    let (_, events) = run(store.path(), corpus.path());

    let gitkeep = find(&events, ".gitkeep");
    assert_eq!(
        gitkeep.outcome,
        FileOutcome::Skipped,
        "a walked, whitelisted `.gitkeep` is empty, not hidden: {:?}",
        gitkeep.why
    );
    assert_eq!(gitkeep.why.as_deref(), Some("empty"));

    // The credential rules did not move with it.
    let env = find(&events, ".env");
    assert_eq!(env.outcome, FileOutcome::Refused);
    assert!(
        env.why.as_deref().unwrap().contains(".env"),
        "the refusal should name the pattern: {:?}",
        env.why
    );
}

/// The same `.gitkeep`, named rather than walked, is still refused: a caller
/// typing a path to a dotfile is a different statement from a walk yielding
/// one.
#[test]
fn a_named_dotfile_is_still_refused_as_hidden() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    undecidable_tree(corpus.path());
    let named = corpus.path().join("dist").join(".gitkeep");

    let mut s = store_at(store.path());
    let mut events = Vec::new();
    s.index_paths(&[named], |path, p| {
        events.push(Event {
            path: path.to_path_buf(),
            outcome: p.outcome,
            why: p.why.clone(),
            scanned: p.scanned,
            total: p.total,
        });
    })
    .unwrap();

    let gitkeep = find(&events, ".gitkeep");
    assert_eq!(gitkeep.outcome, FileOutcome::Refused);
    assert!(
        gitkeep.why.as_deref().unwrap().contains("hidden"),
        "a named dotfile is refused as hidden: {:?}",
        gitkeep.why
    );
}

/// The portal's progress bar reaching its own total, which it did not when a
/// branch returned without reporting.
///
/// One verdict per file. `indexing` is not a verdict — it is the marker that
/// says a slow embed has started, and a file that is being embedded carries it
/// and then, if the embed fails, its answer. Everything in this tree is
/// decided without embedding, so here one file is one event.
#[test]
fn every_walked_file_produces_exactly_one_verdict() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    undecidable_tree(corpus.path());

    let (_, events) = run(store.path(), corpus.path());

    let mut paths: Vec<PathBuf> = events
        .iter()
        .filter(|e| e.outcome != FileOutcome::Indexing)
        .map(|e| e.path.clone())
        .collect();
    let before = paths.len();
    paths.sort();
    paths.dedup();
    assert_eq!(before, paths.len(), "a file was given two verdicts");

    let last = events.last().expect("the walk found nothing");
    assert_eq!(
        last.scanned, last.total,
        "the run ended having scanned {} of {}",
        last.scanned, last.total
    );
    assert_eq!(paths.len(), last.total);
}

/// An entry the walk cannot read is a `failed` file on the same channel as
/// every other verdict, not a line on stderr the daemon never sees.
#[cfg(unix)]
#[test]
fn an_unreadable_directory_is_reported_rather_than_printed() {
    use std::os::unix::fs::PermissionsExt;

    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    undecidable_tree(corpus.path());
    let closed = corpus.path().join("closed");
    fs::create_dir_all(&closed).unwrap();
    write(&closed, "inside.txt", b"something");
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();

    let (report, events) = run(store.path(), corpus.path());

    // Restored before any assertion can fail, or the temporary directory
    // cannot be cleaned up.
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        report
            .failed
            .iter()
            .any(|(path, _)| path.contains("closed")),
        "the unreadable directory is missing from the report: {:?}",
        report.failed
    );
    let event = find(&events, "closed");
    assert_eq!(event.outcome, FileOutcome::Failed);
    assert!(event.why.is_some());
    let last = events.last().unwrap();
    assert_eq!(last.scanned, last.total, "the run still finished");
}

/// The cost this release took out of the Windows path: the home directory is
/// resolved once for the run rather than twice for every file.
#[test]
fn the_home_is_canonicalised_once_per_run_not_twice_per_file() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    undecidable_tree(corpus.path());
    for i in 0..40 {
        write(corpus.path(), &format!("generated/{i}.scss"), b"");
    }

    let before = semlith::canonical_calls();
    let (_, events) = run(store.path(), corpus.path());
    let spent = semlith::canonical_calls() - before;
    let files = events.len() as u64;

    // The walk canonicalises each entry it yields, which is where the store's
    // keys come from and is the one call per file that has to stay. 0.18.0
    // added two more on top of it, on every file, for an answer that cannot
    // change during a run.
    assert!(
        spent < files * 2,
        "{spent} canonicalize calls for {files} files — the per-file home \
         resolution is back"
    );
}

/// The whole fixture tree, including the halves that need a model: one
/// truncated PNG that fails, one real file that indexes anyway.
#[test]
#[ignore = "downloads an embedding model and CLIP on first run"]
fn one_corrupt_image_does_not_end_the_run() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    undecidable_tree(corpus.path());
    // A dotfile the tree's own ignore file asks for. A plain dotfile is still
    // outside the walk — 0.19.0 did not change which files the walker yields,
    // only what happens to the ones it does — so the whitelist is what makes
    // this the case the release is about.
    fs::write(
        corpus.path().join(".gitignore"),
        b"dist/*\n!dist/.gitkeep\n!dist/.env\n!.eslintrc.json\n",
    )
    .unwrap();
    write(
        corpus.path(),
        ".eslintrc.json",
        br#"{"extends": ["eslint:recommended"], "rules": {"eqeqeq": "error"}}"#,
    );
    // A real PNG with its pixel data cut off. The signature and the IHDR are
    // intact, so the dimension reader — which only reads the header — accepts
    // it and the file goes to the decoder, which is where it fails. A file
    // whose header is malformed is a different case: that one is `skipped` as
    // not a decodable image, and never reaches a decoder at all.
    let whole = {
        let img = image::RgbImage::from_pixel(64, 64, image::Rgb([10, 120, 200]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    };
    let truncated = &whole[..whole.len() / 2];
    write(corpus.path(), "broken.png", truncated);

    let (report, events) = run(store.path(), corpus.path());

    assert_eq!(
        report.failed.len(),
        1,
        "exactly the one corrupt image should have failed: {:?}",
        report.failed
    );
    assert!(report.failed[0].0.ends_with("broken.png"));
    assert!(
        !report.failed[0].1.trim().is_empty(),
        "the decoder's own message is the point of the line"
    );

    let broken = find(&events, "broken.png");
    assert_eq!(broken.outcome, FileOutcome::Failed);

    // The run carried on, and the whitelisted dotfile beside it is in the
    // store with chunks.
    assert!(
        report.indexed >= 1 && report.chunks > 0,
        "the run should have indexed the files that were fine: {report:?}"
    );
    let eslintrc = find(&events, ".eslintrc.json");
    assert_eq!(
        eslintrc.outcome,
        FileOutcome::Indexing,
        "the whitelisted dotfile beside the failure should have been embedded"
    );

    let last = events.last().unwrap();
    assert_eq!(last.scanned, last.total);
}

/// The same tree with a valid image, so the test above proves the decoder
/// failed rather than that the image path is broken.
#[test]
#[ignore = "downloads an embedding model and CLIP on first run"]
fn the_same_tree_with_a_valid_image_indexes_it() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    undecidable_tree(corpus.path());
    let img = image::RgbImage::from_pixel(64, 64, image::Rgb([200, 30, 30]));
    let path = corpus.path().join("broken.png");
    img.save(&path).unwrap();

    let (report, events) = run(store.path(), corpus.path());

    assert!(
        report.failed.is_empty(),
        "nothing should have failed: {:?}",
        report.failed
    );
    assert_eq!(report.images, 1);
    assert_eq!(find(&events, "broken.png").outcome, FileOutcome::Indexing);
}
