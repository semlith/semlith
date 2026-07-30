//! What 0.8.0 promises about documents: a file a person would open in Word,
//! PowerPoint, Excel, LibreOffice, a browser or Jupyter is read as the text
//! that person would see, and a file semlith cannot read is skipped rather than
//! fatal.
//!
//! The fixtures under `tests/fixtures` are written by the real libraries —
//! python-docx, python-pptx, openpyxl, odfpy — by `tests/fixtures/generate.py`,
//! not hand-written XML. A parser that only ever meets XML written by its own
//! test proves nothing about the document a user has.
//!
//! Everything here except the end-to-end search runs without an embedding
//! model, because extraction is a pure function of bytes:
//!
//! ```sh
//! cargo test --test formats
//! cargo test --test formats -- --ignored   # the corpus round trip
//! ```

use semlith::chunk;
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn text_of(name: &str) -> String {
    let path = fixtures().join(name);
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("reading {name}: {e}"));
    chunk::extract(&path, &bytes).unwrap_or_else(|| panic!("{name} was skipped, not read"))
}

/// The release's central claim, one format at a time: the phrase a person can
/// see in the document is in the text semlith embeds, and the JSON, XML and
/// markup that carried it is not.
#[test]
fn every_document_format_yields_the_text_a_person_would_see() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "notes.docx",
            &["quokka thermostat calibration", "ferret ledger entry"],
            &["w:document", "w:t"],
        ),
        (
            "deck.pptx",
            &["opening remarks placeholder", "marmoset budget review"],
            &["a:t", "p:sld"],
        ),
        (
            "sheet.xlsx",
            &["pangolin invoice discrepancy", "Sheet: Q3 Notes"],
            &["sharedStrings", "worksheet"],
        ),
        ("notes.odt", &["tapir onboarding checklist"], &["text:p"]),
        ("deck.odp", &["okapi rollout plan"], &["draw:page"]),
        ("sheet.ods", &["civet expense summary"], &["table:table"]),
        (
            "analysis.ipynb",
            &[
                "capybara regression writeup",
                "def calibrate_axolotl(readings):",
                "narwhal fit converged",
                "quoll appendix notes",
            ],
            &["cell_type", "execution_count", "\\n"],
        ),
        (
            "page.html",
            &["The wombat migration corridor runs east of the ridge."],
            &["<p", "class=", "gecko_secret_token", "font-family"],
        ),
    ];

    for (name, wanted, unwanted) in cases {
        let text = text_of(name);
        for phrase in *wanted {
            assert!(
                text.contains(phrase),
                "{name} does not contain {phrase:?}:\n{text}"
            );
        }
        for syntax in *unwanted {
            assert!(
                !text.contains(syntax),
                "{name} still carries its syntax {syntax:?}:\n{text}"
            );
        }
    }
}

/// A slide, a sheet and a cell are divisions a line number cannot express, so
/// each one is named in the text — and a deck of twelve proves the ordering is
/// numeric, because sorted as names slide11 comes before slide2.
#[test]
fn slides_sheets_and_cells_are_named_and_in_order() {
    let deck = text_of("deck.pptx");
    let eleventh = deck.find("marmoset budget review").unwrap();
    let marker = deck.find("# Slide 11").unwrap();
    let twelfth = deck.find("# Slide 12").unwrap();
    assert!(
        marker < eleventh && eleventh < twelfth,
        "slide 11's text is not under its own marker:\n{deck}"
    );
    let order: Vec<usize> = (1..=12)
        .map(|n| deck.find(&format!("# Slide {n}\n")).unwrap())
        .collect();
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "slides came out in name order rather than slide order:\n{deck}"
    );

    // The second sheet of the workbook, not the first, and named.
    let sheet = text_of("sheet.xlsx");
    assert!(sheet.find("## Sheet: Summary") < sheet.find("## Sheet: Q3 Notes"));
    assert!(
        sheet.find("## Sheet: Q3 Notes").unwrap() < sheet.find("pangolin").unwrap(),
        "the phrase is not under the sheet that holds it:\n{sheet}"
    );
    // A row is a line of tab-separated cells, so a label keeps its number.
    assert!(
        sheet.lines().any(|l| l.contains('\t')),
        "no row came out as cells:\n{sheet}"
    );

    let notebook = text_of("analysis.ipynb");
    let cells: Vec<usize> = (1..=3)
        .map(|n| notebook.find(&format!("# Cell {n} (")).unwrap())
        .collect();
    assert!(cells.windows(2).all(|w| w[0] < w[1]), "cells out of order");
    assert!(
        notebook.contains("# Cell 2 (code)"),
        "a cell's kind is not recorded:\n{notebook}"
    );
}

/// The locator has to keep meaning something. An HTML page's text moves through
/// extraction and chunking, and the line range that comes out the far end must
/// still be the line of the file on disk.
#[test]
fn an_html_hit_points_at_the_line_of_the_file_on_disk() {
    let source = fs::read_to_string(fixtures().join("page.html")).unwrap();
    let sentence = "The wombat migration corridor runs east of the ridge.";
    let source_line = source
        .lines()
        .position(|l| l.contains(sentence))
        .expect("the fixture no longer holds the sentence")
        + 1;

    let chunks = chunk::chunk_text(&text_of("page.html"));
    let hit = chunks
        .iter()
        .find(|c| c.text.contains(sentence))
        .expect("the sentence did not survive chunking");

    assert!(
        (hit.start_line..=hit.end_line).contains(&(source_line as u32)),
        "the sentence is on line {source_line} of page.html, and the chunk claims \
         lines {}-{}",
        hit.start_line,
        hit.end_line
    );

    // The entity forms a browser would render, rendered.
    let text = text_of("page.html");
    assert!(text.contains("dawn & dusk"), "{text}");
    assert!(text.contains("cafés"), "{text}");
    assert!(
        !text.contains("a comment with"),
        "an HTML comment was indexed:\n{text}"
    );
}

/// Every way a document can be unreadable, and the same answer to all of them:
/// skipped, not an error, not a panic, not an unbounded allocation.
#[test]
fn an_unreadable_document_is_skipped_rather_than_fatal() {
    let good = fs::read(fixtures().join("notes.docx")).unwrap();

    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "corrupt.docx",
            b"PK\x03\x04 and then nothing that follows".to_vec(),
        ),
        ("truncated.docx", good[..500].to_vec()),
        ("empty.xlsx", Vec::new()),
        ("random.pptx", (0u8..=255).cycle().take(4096).collect()),
        ("broken.ipynb", b"{\"cells\": not json}".to_vec()),
        ("plain.odt", b"this is not an archive at all".to_vec()),
        // What a password-protected Office document is on disk: a container
        // that is not a ZIP, so it never opens.
        ("encrypted.docx", {
            let mut v = vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
            v.extend(std::iter::repeat_n(0u8, 512));
            v
        }),
    ];

    for (name, bytes) in cases {
        let path = PathBuf::from(name);
        assert!(
            chunk::extract(&path, &bytes).is_none(),
            "{name} was read as text rather than skipped"
        );
    }

    // And the two on disk: an archive whose entries are encrypted, and one
    // whose single entry expands far past the decompression cap.
    for name in ["protected.docx", "bomb.docx"] {
        let path = fixtures().join(name);
        let bytes = fs::read(&path).unwrap();
        assert!(
            chunk::extract(&path, &bytes).is_none(),
            "{name} was read; the cap or the encryption check did not hold"
        );
    }
}

/// The round trip the release exists for: a directory of documents, one index
/// run, and a question answered with the file that holds the answer.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_mixed_corpus_indexes_and_answers() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();

    let documents = [
        "notes.docx",
        "deck.pptx",
        "sheet.xlsx",
        "notes.odt",
        "deck.odp",
        "sheet.ods",
        "analysis.ipynb",
        "page.html",
    ];
    for name in documents {
        fs::copy(fixtures().join(name), corpus.path().join(name)).unwrap();
    }
    // Two files that cannot be read, among the ones that can.
    for name in ["protected.docx", "bomb.docx"] {
        fs::copy(fixtures().join(name), corpus.path().join(name)).unwrap();
    }

    let mut s = semlith::Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let report = s
        .index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();

    assert_eq!(
        report.indexed,
        documents.len(),
        "not every document was indexed: {report:?}"
    );
    assert_eq!(
        report.skipped, 2,
        "the unreadable files were not skipped cleanly: {report:?}"
    );

    let questions = [
        ("quokka thermostat calibration", "notes.docx"),
        ("marmoset budget review", "deck.pptx"),
        ("pangolin invoice discrepancy", "sheet.xlsx"),
        ("tapir onboarding checklist", "notes.odt"),
        ("okapi rollout plan", "deck.odp"),
        ("civet expense summary", "sheet.ods"),
        ("capybara regression writeup", "analysis.ipynb"),
        ("wombat migration corridor", "page.html"),
    ];
    for (question, expected) in questions {
        let hits = s.search(question, 3).unwrap();
        let top = hits.first().unwrap_or_else(|| {
            panic!("{question:?} found nothing at all");
        });
        assert!(
            top.path.ends_with(expected),
            "{question:?} ranked {} first, not {expected}",
            top.path
        );
        assert!(top.start_line >= 1, "a hit with no locator: {top:?}");
    }

    // A changed document is re-read, and the old text stops being an answer.
    fs::copy(
        fixtures().join("page.html"),
        corpus.path().join("notes.odt"),
    )
    .unwrap();
    let report = s
        .index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    assert_eq!(report.indexed, 1, "only the changed file re-indexes");
    let hits = s.search("tapir onboarding checklist", 5).unwrap();
    assert!(
        !hits.iter().any(|h| h.path.ends_with("notes.odt")),
        "the replaced document's old text is still searchable: {hits:#?}"
    );
}
