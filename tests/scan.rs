//! The credential content scan: the table, what it refuses at index time, and
//! `semlith scan` over a store built before the rule existed.
//!
//! A refused file is one whose text never reaches the embedder, so most of
//! this needs no model. The two that assert a file was *indexed* do, and say
//! so.

use semlith::{FileOutcome, Semlith};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn store_at(dir: &Path) -> Semlith {
    let mut s = Semlith::open(dir, None).unwrap();
    s.quiet = true;
    s
}

fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, body).unwrap();
    path
}

/// Index `corpus` and return the report with every `(path, outcome, why)`.
fn run(
    store: &Path,
    corpus: &Path,
    secrets: bool,
) -> (
    semlith::IndexReport,
    Vec<(PathBuf, FileOutcome, Option<String>)>,
) {
    let mut s = store_at(store);
    s.boundary = semlith::Boundary {
        roots: None,
        allow_secrets: secrets,
    };
    let mut events = Vec::new();
    let report = s
        .index_paths(&[corpus.to_path_buf()], |path, p| {
            events.push((path.to_path_buf(), p.outcome, p.why.clone()))
        })
        .unwrap();
    (report, events)
}

/// A `.txt` per shape, named after nothing in the deny-list so the only rule
/// that can refuse it is the content scan.
fn credential_files(corpus: &Path) {
    for (i, shape) in semlith::filter::SHAPES.iter().enumerate() {
        write(
            corpus,
            &format!("config{i}.txt"),
            &format!(
                "# configuration\nvalue = \"{}\"\nother = 1\n",
                shape.example
            ),
        );
    }
    write(
        corpus,
        "generic.txt",
        "# configuration\napi_key = \"Xq7Lp2Rv9Wz4Kd8Nf1Hj\"\n",
    );
}

/// The table is only as good as the fact that every row means what it says.
#[test]
fn every_shape_matches_its_example_and_not_its_near_miss() {
    for shape in semlith::filter::SHAPES {
        let found = semlith::filter::scan_text(shape.example);
        assert!(
            found.is_some(),
            "{}: the pattern {:?} does not match its own example {:?}",
            shape.kind,
            shape.pattern,
            shape.example
        );
        assert!(
            !shape.example.is_empty() && !shape.near_miss.is_empty(),
            "{}: a row without both strings is a row nothing checks",
            shape.kind
        );
        assert!(
            semlith::filter::scan_text(shape.near_miss).is_none(),
            "{}: the near miss {:?} matched, so the pattern is too wide",
            shape.kind,
            shape.near_miss
        );
    }
}

/// The generic rule needs both halves. A key-like name with a placeholder is
/// not a credential, and a high-entropy blob with no name beside it is a test
/// fixture or a hash.
#[test]
fn the_generic_rule_needs_the_name_and_the_entropy_together() {
    assert!(semlith::filter::scan_text("api_key = \"Xq7Lp2Rv9Wz4Kd8Nf1Hj\"").is_some());
    // Entropy without a key-like name.
    assert!(semlith::filter::scan_text("digest = \"Xq7Lp2Rv9Wz4Kd8Nf1Hj\"").is_none());
    // A key-like name with no entropy behind it.
    assert!(semlith::filter::scan_text("password = \"aaaaaaaaaaaaaaaaaaaaaa\"").is_none());
    // Short enough to be a real password and not worth refusing a file for.
    assert!(semlith::filter::scan_text("password = \"hunter2\"").is_none());
}

/// The line number is the useful half of the finding.
#[test]
fn the_finding_names_the_line() {
    let text = "one\ntwo\nthree\nghp_aaaaBBBBccccDDDDeeeeFFFFgggg12345678\n";
    let found = semlith::filter::scan_text(text).expect("a GitHub token");
    assert_eq!(found.line, 4);
    assert!(found.kind.contains("GitHub"));
}

/// The rule the whole feature stands on: the scan's own output must never
/// contain the thing it found.
#[test]
fn no_reason_ever_quotes_the_credential() {
    for shape in semlith::filter::SHAPES {
        let found = semlith::filter::scan_text(shape.example).expect("its own example");
        let reason = found.reason();
        for window in shape
            .example
            .as_bytes()
            .windows(8)
            .map(|w| String::from_utf8_lossy(w).into_owned())
        {
            assert!(
                !reason.contains(&window),
                "{}: the reason {reason:?} quotes {window:?} out of the credential",
                shape.kind
            );
        }
    }
}

/// Every shape, as a file, through a real index run.
#[test]
fn a_credential_in_an_ordinary_file_is_refused_at_index_time() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    credential_files(corpus.path());

    let (report, events) = run(store.path(), corpus.path(), false);

    let expected = semlith::filter::SHAPES.len() + 1;
    assert_eq!(
        report.refused.len(),
        expected,
        "every credential fixture should have been refused: {:?}",
        report.refused
    );
    assert_eq!(report.indexed, 0);
    for (path, outcome, why) in &events {
        assert_eq!(
            *outcome,
            FileOutcome::Refused,
            "{} was not refused",
            path.display()
        );
        let why = why.as_deref().unwrap();
        assert!(
            why.contains("line"),
            "the reason should name the line: {why}"
        );
    }
}

/// A cleaned twin of each fixture is ordinary text, and a release that refused
/// it would be one that stopped indexing code.
#[test]
fn the_cleaned_twins_are_not_refused() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    for (i, shape) in semlith::filter::SHAPES.iter().enumerate() {
        write(
            corpus.path(),
            &format!("clean{i}.txt"),
            &format!(
                "# configuration\nvalue = \"{}\"\nother = 1\n",
                shape.near_miss
            ),
        );
    }

    let (report, _) = run(store.path(), corpus.path(), false);
    assert!(
        report.refused.is_empty(),
        "a near miss is not a credential: {:?}",
        report.refused
    );
}

/// The scan reads what a document reader produced, not only what a text file
/// holds. A `.docx` is a zip of XML and the key is in its body.
#[test]
fn a_credential_inside_a_document_is_refused_on_the_readers_text() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write_docx(
        &corpus.path().join("handover.docx"),
        "Access key AKIAIOSFODNN7EXAMPLE, please rotate it.",
    );

    let (report, _) = run(store.path(), corpus.path(), false);

    assert_eq!(
        report.refused.len(),
        1,
        "the reader's text should have been scanned: {report:?}"
    );
    assert!(report.refused[0].1.contains("AWS"));
}

/// An image is not text and is never scanned, even when its bytes happen to
/// spell a credential's prefix.
#[test]
fn an_image_is_not_scanned() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    // Not a decodable image, which is enough: what is asserted is that the
    // content scan never sees it, and a scan that ran would refuse it.
    fs::write(
        corpus.path().join("shot.png"),
        b"\x89PNG\r\n\x1a\nAKIAIOSFODNN7EXAMPLE",
    )
    .unwrap();

    let (report, _) = run(store.path(), corpus.path(), false);
    assert!(
        report.refused.is_empty(),
        "an image must not be scanned as text: {:?}",
        report.refused
    );
}

/// `--include-secrets` is the user saying they meant it, and the count is
/// semlith saying what that cost.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn include_secrets_indexes_them_and_says_how_many() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    credential_files(corpus.path());

    let (report, _) = run(store.path(), corpus.path(), true);

    assert!(report.refused.is_empty(), "{:?}", report.refused);
    assert_eq!(report.secrets_indexed, semlith::filter::SHAPES.len() + 1);
    assert_eq!(report.indexed, report.secrets_indexed);
}

/// A file that was clean when it was indexed and is not any more leaves the
/// store on the next run, rather than being left behind because the new rule
/// only stops new files.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_file_that_gains_a_token_is_evicted_on_the_next_run() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "deploy.md",
        "Deployment runs on Tuesdays after the migration has finished.\n",
    );

    let (first, _) = run(store.path(), corpus.path(), false);
    assert_eq!(first.indexed, 1);
    assert!(
        !store_at(store.path())
            .search("deployment on tuesdays", 5)
            .unwrap()
            .is_empty()
    );

    write(
        corpus.path(),
        "deploy.md",
        "Deployment runs on Tuesdays.\ntoken = \"ghp_aaaaBBBBccccDDDDeeeeFFFFgggg12345678\"\n",
    );
    let (second, _) = run(store.path(), corpus.path(), false);

    assert_eq!(second.refused.len(), 1);
    assert!(
        second.refused[0].1.contains("removed from this store"),
        "the refusal should say the earlier contents went: {:?}",
        second.refused[0].1
    );
    assert!(
        store_at(store.path())
            .search("deployment on tuesdays", 5)
            .unwrap()
            .is_empty(),
        "the old contents are still searchable after the file was refused"
    );
}

/// The path for a store indexed before any of this existed.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn scan_finds_what_todays_rules_would_refuse_and_forget_evicts_it() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "notes.md",
        "Nothing secret here, only the release notes.\n",
    );
    // Indexed with the scan off, which is what a 0.18.0 store is.
    write(
        corpus.path(),
        "handover.md",
        "The key is ghp_aaaaBBBBccccDDDDeeeeFFFFgggg12345678 until Friday.\n",
    );
    let (report, _) = run(store.path(), corpus.path(), true);
    assert_eq!(report.indexed, 2);

    let mut s = store_at(store.path());
    let found = s.scan().unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].path.ends_with("handover.md"));
    assert!(found[0].why.contains("GitHub"));

    let gone = s.forget_findings(&found).unwrap();
    assert_eq!(gone, 1);
    assert!(
        s.scan().unwrap().is_empty(),
        "a second scan of a cleaned store finds nothing"
    );
}

/// The smallest `.docx` a reader will take: a zip holding one
/// `word/document.xml`.
fn write_docx(path: &Path, body: &str) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("word/document.xml", options).unwrap();
    write!(
        zip,
        "<?xml version=\"1.0\"?><w:document \
         xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:body><w:p><w:r><w:t>{body}</w:t></w:r></w:p></w:body></w:document>"
    )
    .unwrap();
    zip.finish().unwrap();
}

/// The false-positive audit, as a command rather than a paragraph in a record.
///
/// Point it at a real tree and it prints every file the content scan would
/// refuse, with the kind and the line, reading each file exactly as the index
/// loop does — through `chunk::extract`, so a document reader's text is
/// scanned the way it is in a run. It embeds nothing, so a corpus of any size
/// costs seconds.
///
/// ```sh
/// SEMLITH_SCAN_AUDIT=~/work/some-repo cargo test --test scan audit -- --ignored --nocapture
/// ```
///
/// Ignored because it needs a corpus, not because it is slow. 0.19.0 ran it
/// over two trees before the release proceeded, and the first run is what found
/// the `${SEMLITH_AGENT_KEY}` placeholder rule.
#[test]
#[ignore = "needs SEMLITH_SCAN_AUDIT pointing at a corpus"]
fn audit_a_corpus_for_false_positives() {
    let Ok(root) = std::env::var("SEMLITH_SCAN_AUDIT") else {
        panic!("set SEMLITH_SCAN_AUDIT to the tree to audit");
    };
    let root = PathBuf::from(root);
    let mut walked = 0usize;
    let mut refused = 0usize;
    let mut texts: Vec<String> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                // The directories a walk would never yield anyway.
                if !matches!(
                    name.as_str(),
                    ".git" | "target" | "node_modules" | ".semlith" | "dist" | "build"
                ) {
                    stack.push(path);
                }
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            if bytes.len() as u64 > 8 * 1024 * 1024 {
                continue;
            }
            let Ok(text) = semlith::chunk::extract(&path, &bytes) else {
                continue;
            };
            walked += 1;
            if let Some(found) = semlith::filter::scan_text(&text) {
                refused += 1;
                println!("{}: {}", path.display(), found.reason());
            }
            texts.push(text);
        }
    }
    println!("audited {walked} readable files under {}", root.display());
    println!("{refused} would be refused");

    // What the scan itself costs, which is the number the throughput claim
    // rests on. An end-to-end index run is almost entirely the embedder, so a
    // wall-clock comparison of two runs measures the model rather than this.
    let mut total = 0usize;
    let start = std::time::Instant::now();
    for text in &texts {
        total += text.len();
        std::hint::black_box(semlith::filter::scan_text(text));
    }
    let spent = start.elapsed();
    println!(
        "the scan itself: {:.1} ms over {:.1} MiB of text ({:.0} MiB/s)",
        spent.as_secs_f64() * 1000.0,
        total as f64 / (1024.0 * 1024.0),
        (total as f64 / (1024.0 * 1024.0)) / spent.as_secs_f64()
    );
}

/// The two shapes the false-positive audit found in real trees, as a gate.
///
/// Both were matches of the one rule with no prefix behind it, and both are
/// things a credential is never written as. Without these the rule refuses a
/// page documenting how not to write a key down, and a `token=` on one line
/// with a quote two lines later.
#[test]
fn a_placeholder_is_not_a_credential() {
    for value in [
        r#"Authorization = "Bearer ${SEMLITH_AGENT_KEY}""#,
        r#"api_key = "{{ openai_api_key_goes_here }}""#,
        r#"password = "<your-database-password-here>""#,
        r#"token: "SEMLITH_AGENT_KEY""#,
    ] {
        assert!(
            semlith::filter::scan_text(value).is_none(),
            "a placeholder was refused as a credential: {value}"
        );
    }
}

/// The value may not cross a line. `split_once("/?token=")` and a quote two
/// lines later are not an assignment; they are code with a colon in it.
#[test]
fn a_generic_match_does_not_span_lines() {
    let code = "let (port, query) = rest.split_once(\"/?token=\").unwrap_or_else(|| {\n\
                    panic!(\"the URL does not carry a token in the documented shape: {url}\")\n\
                });\n";
    assert!(
        semlith::filter::scan_text(code).is_none(),
        "a multi-line match was treated as an assignment"
    );
}

/// A refusal that is about this caller rather than about the file must not
/// evict what another caller already indexed.
///
/// An agent confined to one root that asks to index a path outside it would
/// otherwise be able to delete rows the command line put there — a caller who
/// may not read a file deleting it.
#[test]
fn a_boundary_refusal_evicts_nothing() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(corpus.path(), "notes.md", "Release notes for the API.\n");
    let elsewhere = write(outside.path(), "plans.md", "The plans.\n");

    // Already in the store, put there by a caller with no boundary — written
    // rather than embedded, because what is under test is the refusal and not
    // the embedder.
    let s = store_at(store.path());
    let key = elsewhere.to_string_lossy().into_owned();
    semlith::store::read_only(s.db(), false).unwrap();
    let file = semlith::store::insert_file(s.db(), &key, "hash", 11, 0).unwrap();
    semlith::store::insert_chunk(s.db(), file, 0, 1, 1, "The plans.").unwrap();
    semlith::store::read_only(s.db(), true).unwrap();
    let before = semlith::store::all_paths(s.db()).unwrap();
    assert_eq!(before.len(), 1, "{before:?}");
    drop(s);

    // Now as an agent confined to the corpus, asking for the path outside it.
    let mut s = store_at(store.path());
    s.boundary = semlith::Boundary::within(vec![corpus.path().to_path_buf()]);
    let report = s.index_paths(&[elsewhere], |_, _| {}).unwrap();

    // Whether this is a boundary refusal at all depends on where the operating
    // system puts temporary directories: `within_boundary` admits anything
    // under the user's home, and on Windows the temp directory is under it. So
    // the refusal is asserted where it happens, and the half that matters —
    // the store is untouched — is asserted everywhere. The refusal itself is
    // pinned deterministically by the test below, which needs no files.
    if let Some((_, why)) = report.refused.first() {
        assert!(why.contains("outside"), "{why:?}");
        assert!(
            !why.contains("removed from this store"),
            "a boundary refusal claimed to have evicted something"
        );
    }
    let after = semlith::store::all_paths(s.db()).unwrap();
    assert_eq!(
        after, before,
        "a refusal about the caller deleted what another caller indexed"
    );
}

/// The flag that decides whether a refusal evicts, pinned without touching a
/// filesystem.
///
/// `Boundary::refuses` resolves nothing on disk — `canonical` falls back to the
/// path it was given — so a synthetic path outside any home exercises the
/// boundary branch identically on all three platforms.
#[test]
fn only_a_refusal_about_the_file_itself_is_marked_for_eviction() {
    let home = PathBuf::from(if cfg!(windows) {
        r"C:\Users\nobody"
    } else {
        "/home/nobody"
    });
    let root = home.join("work").join("api");
    let outside = PathBuf::from(if cfg!(windows) {
        r"C:\elsewhere\somebody-elses\notes.md"
    } else {
        "/elsewhere/somebody-elses/notes.md"
    });

    let agent = semlith::Boundary::within(vec![root.clone()]);
    let refusal = agent
        .refuses(&outside, false, Some(&home))
        .expect("a path outside the roots and outside the home is refused");
    assert!(refusal.why.contains("outside"), "{:?}", refusal.why);
    assert!(
        !refusal.credential,
        "a refusal about the caller must not evict what another caller indexed"
    );

    // The other kind, for contrast: this one is about the file, and does evict.
    let named = semlith::Boundary::default()
        .refuses(&root.join(".env"), true, Some(&home))
        .expect("a credential is refused by name");
    assert!(named.credential, "{:?}", named.why);
}
