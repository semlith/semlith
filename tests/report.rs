//! The five reports, the four formats, and the one rule that matters: a file
//! saved from the portal and a file written by `semlith report` are the same
//! bytes, because they come from the same generator.
//!
//! ```sh
//! cargo test --test report -- --ignored
//! ```

use semlith::Semlith;
use semlith::fleet::Fleet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

fn cli(store: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store)
        .args(args)
        .output()
        .expect("the binary runs");
    assert!(
        out.status.success(),
        "semlith {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A stamp differs between two runs a second apart, so it is not what byte
/// equality is about. Everything else is.
fn without_the_stamp(text: &str) -> String {
    text.lines()
        .filter(|line| !line.contains("Generated 2") && !line.contains("\"generated\""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn corpus() -> (tempfile::TempDir, tempfile::TempDir) {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "a.rs",
        "fn start() { middle(); outside(); }\n",
    );
    write(corpus.path(), "b.rs", "fn middle() { finish(); }\n");
    write(corpus.path(), "c.rs", "fn finish() {}\n");
    write(
        corpus.path(),
        "notes.md",
        "# Notes\n\nThe lock is held by one writer.\n",
    );
    (corpus, store)
}

/// Every report generates in every format, and the CLI's bytes are the
/// generator's bytes.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn every_report_generates_in_every_format_and_the_cli_matches() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
        // A row, so the savings report has something to be over.
        semlith::store::record_retrieval(
            s.db(),
            &semlith::store::NewRetrieval {
                client: "claude-code",
                session: "s1",
                tool: "search",
                query: "who holds the lock",
                hits: 2,
                micros: 5_000,
                excerpt_tokens: 120,
                whole_file_tokens: 3_000,
                stale_hits: 0,
                tokenizer: semlith::ledger::CHARS4,
                query_id: "q1",
            },
        )
        .unwrap();
    }

    let dirs: Vec<PathBuf> = vec![store.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();
    for (kind, title) in semlith::report::KINDS {
        for format in semlith::report::FORMATS {
            let generated = semlith::report::generate(&fleet, kind, "Sonnet 5")
                .unwrap()
                .render(format)
                .unwrap();
            assert!(generated.contains(title), "{kind}/{format} lost its title");
            let printed = cli(store.path(), &["report", kind, "--format", format]);
            assert_eq!(
                without_the_stamp(&printed),
                without_the_stamp(&generated),
                "`semlith report {kind} --format {format}` is not what the generator produced"
            );
        }
    }
}

/// The savings report keeps every field the design names, and never sums the
/// three counterfactual lines.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_savings_report_states_coverage_tier_and_the_trust_strip() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
        for (query, hits, excerpt, whole) in [
            ("who holds the lock", 2, 120, 3_000),
            ("nothing at all", 0, 40, 0),
        ] {
            semlith::store::record_retrieval(
                s.db(),
                &semlith::store::NewRetrieval {
                    client: "claude-code",
                    session: "s1",
                    tool: "search",
                    query,
                    hits,
                    micros: 5_000,
                    excerpt_tokens: excerpt,
                    whole_file_tokens: whole,
                    stale_hits: 0,
                    tokenizer: semlith::ledger::CHARS4,
                    query_id: query,
                },
            )
            .unwrap();
        }
    }
    let dirs: Vec<PathBuf> = vec![store.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();
    let text = semlith::report::generate(&fleet, "savings", "Sonnet 5")
        .unwrap()
        .render("markdown")
        .unwrap();

    for wanted in [
        "Whole files not read",
        "Excerpts read instead",
        "Refunds",
        "Net",
        "Coverage",
        "Tier",
        "Zero-hit",
        "Refund rate",
        "p50",
        "p95",
        "Ledger chain",
        "Sonnet 5",
    ] {
        assert!(
            text.contains(wanted),
            "the savings report is missing {wanted:?}"
        );
    }
    assert!(
        text.contains("never \nsummed")
            || text.contains("never summed")
            || text.contains("never\n                   summed"),
        "the report must say the counterfactuals are not summed: {text}"
    );

    // A different model is a different price on the same tokens, and the
    // report says which model it priced at rather than leaving a bare dollar
    // figure on the page.
    let dearer = semlith::report::generate(&fleet, "savings", "Opus 5")
        .unwrap()
        .render("markdown")
        .unwrap();
    assert!(dearer.contains("Opus 5"), "{dearer}");
    assert_ne!(
        text.lines().find(|l| l.contains("Net cost avoided")),
        dearer.lines().find(|l| l.contains("Net cost avoided")),
        "the same tokens cost the same at two different prices"
    );
}

/// An unknown report names the five that exist rather than failing blankly.
#[test]
fn an_unknown_report_names_the_ones_there_are() {
    let store = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store.path())
        .args(["report", "team-rollup"])
        .output()
        .expect("the binary runs");
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("savings"), "{said}");
    assert!(said.contains("gaps"), "{said}");
}

// --------------------------------------------- the window and the scope

use semlith::report::{Block, Report, Window};

/// A store with one recorded retrieval and nothing indexed.
///
/// No embedding model is downloaded: a report reads the ledger, and the ledger
/// is written by `record_retrieval` under its own write permission. That is
/// what lets these run in the default set rather than behind `--ignored`.
fn ledger_store(dir: &Path, session: &str, days_ago: i64) {
    let s = Semlith::open(dir, None).unwrap();
    semlith::store::record_retrieval(
        s.db(),
        &semlith::store::NewRetrieval {
            client: "claude-code",
            session,
            tool: "search",
            query: session,
            hits: 2,
            micros: 5_000,
            excerpt_tokens: 120,
            whole_file_tokens: 3_000,
            stale_hits: 0,
            tokenizer: semlith::ledger::CHARS4,
            query_id: session,
        },
    )
    .unwrap();
    if days_ago > 0 {
        // Backdated in place because `NewRetrieval` has no `at` — the ledger
        // stamps its own rows, which is the point of it. This breaks the row's
        // hash, so nothing below asserts the chain verifies.
        semlith::store::read_only(s.db(), false).unwrap();
        s.db()
            .execute("UPDATE retrievals SET at = at - ?1", [days_ago * 86_400])
            .unwrap();
        semlith::store::read_only(s.db(), true).unwrap();
    }
}

fn labels(fleet: &Fleet) -> Vec<String> {
    fleet.each().map(|(label, _)| label.to_string()).collect()
}

/// A caller that names neither gets the report 0.26.x wrote.
///
/// This is the whole of the minor-release promise: a script written against
/// the old route keeps working, byte for byte.
#[test]
fn a_report_with_no_window_and_no_scope_is_the_one_before_it() {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    ledger_store(one.path(), "recent", 0);
    ledger_store(two.path(), "older", 40);
    let dirs = vec![one.path().to_path_buf(), two.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();

    for (kind, _) in semlith::report::KINDS {
        let old = semlith::report::generate(&fleet, kind, "Sonnet 5").unwrap();
        let new =
            semlith::report::generate_over(&fleet, kind, "Sonnet 5", Window::Unset, &[]).unwrap();
        assert_eq!(
            without_the_stamp(&old.render("markdown").unwrap()),
            without_the_stamp(&new.render("markdown").unwrap()),
            "{kind} changed when nothing was asked for"
        );
        assert_eq!(old.stores, labels(&fleet), "{kind} narrowed on its own");
    }
}

/// Every window value says something different, and the two reports whose
/// rows carry a date actually lose rows to it.
#[test]
fn each_window_changes_the_report() {
    let dir = tempfile::tempdir().unwrap();
    ledger_store(dir.path(), "recent", 0);
    {
        // A second session, old enough that only the widest windows hold it.
        let s = Semlith::open(dir.path(), None).unwrap();
        semlith::store::record_retrieval(
            s.db(),
            &semlith::store::NewRetrieval {
                client: "cursor",
                session: "ancient",
                tool: "search",
                query: "ancient",
                hits: 1,
                micros: 1_000,
                excerpt_tokens: 10,
                whole_file_tokens: 100,
                stale_hits: 0,
                tokenizer: semlith::ledger::CHARS4,
                query_id: "ancient",
            },
        )
        .unwrap();
        semlith::store::read_only(s.db(), false).unwrap();
        s.db()
            .execute(
                "UPDATE retrievals SET at = at - ?1 WHERE query = 'ancient'",
                [40 * 86_400i64],
            )
            .unwrap();
        semlith::store::read_only(s.db(), true).unwrap();
    }
    let dirs = vec![dir.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();

    let audit = |window| {
        semlith::report::generate_over(&fleet, "access", "Sonnet 5", window, &[])
            .unwrap()
            .render("markdown")
            .unwrap()
    };
    // A day and a week hold the recent session and not the one forty days old;
    // a month and a quarter differ from each other only in what they say, and
    // `all` holds both.
    assert!(audit(Window::Days(1)).contains("recent"));
    assert!(!audit(Window::Days(1)).contains("ancient"));
    assert!(!audit(Window::Days(7)).contains("ancient"));
    assert!(audit(Window::Days(90)).contains("ancient"));
    assert!(audit(Window::All).contains("ancient"));

    // And every value is distinguishable in every report, including the three
    // that cannot narrow: a reader can tell from the document which was asked
    // for, rather than having to trust the caller.
    for (kind, _) in semlith::report::KINDS {
        let said: std::collections::BTreeSet<String> = semlith::report::WINDOWS
            .iter()
            .map(|(name, _)| {
                let window = semlith::report::window_of(Some(name)).unwrap();
                semlith::report::generate_over(&fleet, kind, "Sonnet 5", window, &[])
                    .unwrap()
                    .window
            })
            .collect();
        assert_eq!(
            said.len(),
            semlith::report::WINDOWS.len(),
            "{kind} says the same thing about two different windows: {said:?}"
        );
    }
}

/// The scope is the store filter the route used to read and discard.
#[test]
fn each_scope_changes_the_report() {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    ledger_store(one.path(), "first-session", 0);
    ledger_store(two.path(), "second-session", 0);
    let dirs = vec![one.path().to_path_buf(), two.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();
    let open = labels(&fleet);
    assert_eq!(open.len(), 2, "two stores did not open: {open:?}");

    let audit = |only: &[String]| {
        semlith::report::generate_over(&fleet, "access", "Sonnet 5", Window::Unset, only).unwrap()
    };
    let both = audit(&[]);
    assert_eq!(both.stores, open);
    let both = both.render("markdown").unwrap();
    assert!(both.contains("first-session") && both.contains("second-session"));

    for (i, label) in open.iter().enumerate() {
        let narrowed = audit(std::slice::from_ref(label));
        assert_eq!(narrowed.stores, vec![label.clone()]);
        let text = narrowed.render("markdown").unwrap();
        let (mine, theirs) = if i == 0 {
            ("first-session", "second-session")
        } else {
            ("second-session", "first-session")
        };
        assert!(text.contains(mine), "{label} lost its own session");
        assert!(
            !text.contains(theirs),
            "{label} answered for the other store"
        );
        // The narrowing is printed rather than implied.
        assert!(text.contains(label), "the report does not name its scope");
    }

    // A store nobody opened is refused rather than dropped: a report headed
    // with one store when two were asked for is a figure with the wrong
    // subject, and nothing in the output would say so.
    let e = semlith::report::generate_over(
        &fleet,
        "access",
        "Sonnet 5",
        Window::Unset,
        &["not-a-store".to_string()],
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("not-a-store"), "{e}");
    assert!(
        e.contains(&open[0]),
        "{e} does not say what would have worked"
    );
}

// ------------------------------------------------------- the fifth format

/// A report long enough to need more than one page.
fn long_report() -> Report {
    let rows: Vec<Vec<String>> = (0..240)
        .map(|i| {
            vec![
                format!("2026-09-{:02} 10:00:00", (i % 28) + 1),
                format!("/a/very/long/path/without/spaces/module-{i}/generated-source-file.rs"),
                "semlith".to_string(),
            ]
        })
        .collect();
    Report {
        kind: "change",
        title: "Change brief".into(),
        generated: "2026-09-20 10:00:00 +05:30".into(),
        stores: vec!["semlith".into()],
        window: "the last 7 days".into(),
        blocks: vec![
            Block::Text {
                text: "Files this machine re-read, newest first — re-read, not rewritten.".into(),
            },
            Block::Facts {
                facts: vec![(
                    "Files re-read".into(),
                    "240".into(),
                    "the last 7 days".into(),
                )],
            },
            Block::Table {
                title: "Re-read".into(),
                columns: vec!["when".into(), "file".into(), "store".into()],
                rows,
            },
        ],
    }
}

/// The number of pages the document's own page tree claims.
fn page_count(pdf: &[u8]) -> usize {
    let text = String::from_utf8_lossy(pdf);
    let at = text.find("/Count ").expect("a page tree with a count");
    text[at + "/Count ".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("a page count")
}

/// All five formats, one structure, and a PDF a person can actually read.
#[test]
fn all_five_formats_render_from_the_one_structure() {
    let report = long_report();
    for format in semlith::report::ALL_FORMATS {
        let bytes = report.render_bytes(format).unwrap();
        assert!(!bytes.is_empty(), "{format} rendered nothing");
        if format == semlith::report::PDF {
            continue;
        }
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Change brief"), "{format} lost the title");
        assert!(text.contains("module-239"), "{format} lost the last row");
        assert!(text.contains("the last 7 days"), "{format} lost the window");
    }
}

#[test]
fn the_pdf_is_valid_paginated_and_its_text_comes_back_out() {
    let pdf = long_report().render_bytes(semlith::report::PDF).unwrap();
    assert!(pdf.starts_with(b"%PDF-"), "no PDF header");
    assert!(
        String::from_utf8_lossy(&pdf).contains("%%EOF"),
        "no trailer"
    );

    let pages = page_count(&pdf);
    assert!(pages > 1, "240 rows fitted on {pages} page(s)");
    assert_eq!(
        String::from_utf8_lossy(&pdf)
            .matches("/Type /Page\n")
            .count(),
        pages,
        "the page tree's count is not the number of page objects"
    );

    // Read back with the extractor this binary already ships: a report semlith
    // writes has to be a report semlith can index.
    let text = pdf_extract::extract_text_from_mem(&pdf).expect("the PDF parses");
    for wanted in [
        "Change brief",
        "the last 7 days",
        "Files re-read",
        "module-0/",
        "module-239/",
        "Re-read",
    ] {
        assert!(
            text.contains(wanted),
            "the PDF does not say {wanted:?}:\n{}",
            &text[..text.len().min(1200)]
        );
    }

    // Nothing ran off the page. 105 is the widest line the geometry allows:
    // 504pt of content, and the smallest face on the page is Courier at 8pt,
    // which is 4.8pt a glyph. A line longer than that would have been drawn
    // past the right margin. The body's own 93-column bound is asserted where
    // the columns are computed, in `report::tests`.
    for line in text.lines() {
        assert!(
            line.len() <= 105,
            "{} characters on one line: {line}",
            line.len()
        );
    }
}

/// The builder's two content toggles change the document, not only the request.
///
/// The design draws three. Signing is not one of them here: it needs a key this
/// product has not decided anything about, so the page draws that switch
/// disabled and says what it is waiting for rather than offering a promise the
/// file would not keep.
#[test]
fn the_two_content_toggles_each_change_the_generated_document() {
    let dir = tempfile::tempdir().unwrap();
    // The session id and the query text are deliberately different strings.
    // `ledger_store` sets both from one argument, which would make this test
    // pass or fail on the session column rather than on the query one — and a
    // session id is an id, not something redaction promises to hide.
    {
        let s = Semlith::open(dir.path(), None).unwrap();
        semlith::store::record_retrieval(
            s.db(),
            &semlith::store::NewRetrieval {
                client: "claude-code",
                session: "sess-1",
                tool: "search",
                query: "what the ledger records",
                hits: 2,
                micros: 5_000,
                excerpt_tokens: 120,
                whole_file_tokens: 3_000,
                stale_hits: 0,
                tokenizer: semlith::ledger::CHARS4,
                query_id: "q1",
            },
        )
        .unwrap();
    }
    let dirs = vec![dir.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();

    let audit = |options| {
        semlith::report::generate_with(&fleet, "access", "Sonnet 5", Window::All, &[], options)
            .unwrap()
            .render("markdown")
            .unwrap()
    };

    let plain = audit(semlith::report::Options::default());
    let attached = audit(semlith::report::Options {
        excerpts: true,
        redact: false,
    });
    let hidden = audit(semlith::report::Options {
        excerpts: true,
        redact: true,
    });

    // Off is the 0.26.x document: the session totals and nothing under them.
    assert!(
        !plain.contains("## Retrievals"),
        "an access report with no toggles set grew a Retrievals table:\n{plain}"
    );
    assert!(
        attached.contains("## Retrievals"),
        "`Attach retrieved excerpts` did not attach anything:\n{attached}"
    );
    assert!(
        attached.contains("what the ledger records"),
        "the attachment does not carry the query it recorded:\n{attached}"
    );

    // Redaction is what it says: the query goes, everything around it stays.
    assert!(
        !hidden.contains("what the ledger records"),
        "`Hash the query text` left the query text in the document:\n{hidden}"
    );
    assert!(
        hidden.contains("hash:"),
        "the redacted document carries no digest where the query was:\n{hidden}"
    );
    for kept in ["claude-code", "search", "sess-1"] {
        assert!(
            hidden.contains(kept),
            "redaction dropped {kept:?}, which it promises to keep:\n{hidden}"
        );
    }

    // Wherever a query reaches paper, not only in the attachment. A toggle that
    // hid it in one table and printed it in another would be a promise broken
    // by the document that made it.
    let zero = tempfile::tempdir().unwrap();
    {
        let s = Semlith::open(zero.path(), None).unwrap();
        semlith::store::record_retrieval(
            s.db(),
            &semlith::store::NewRetrieval {
                client: "claude-code",
                session: "s",
                tool: "search",
                query: "a question nothing answers",
                hits: 0,
                micros: 1_000,
                excerpt_tokens: 0,
                whole_file_tokens: 0,
                stale_hits: 0,
                tokenizer: semlith::ledger::CHARS4,
                query_id: "zero",
            },
        )
        .unwrap();
    }
    let dirs = vec![zero.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();
    let gaps = |options| {
        semlith::report::generate_with(&fleet, "gaps", "Sonnet 5", Window::All, &[], options)
            .unwrap()
            .render("markdown")
            .unwrap()
    };
    assert!(
        gaps(semlith::report::Options::default()).contains("a question nothing answers"),
        "the gaps report stopped naming the questions that found nothing"
    );
    assert!(
        !gaps(semlith::report::Options {
            excerpts: false,
            redact: true,
        })
        .contains("a question nothing answers"),
        "redaction reaches the access report and not the gaps report, so a query text \
         survives in the second document a reader opens"
    );
}
