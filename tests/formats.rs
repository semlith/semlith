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

/// Malformed RTF, as literals rather than inline, because every one of them is
/// a backslash escape that a reader of this file has to be able to trust.
const RTF_OPEN: &[u8] = br"{\rtf1";
const RTF_MIDWORD: &[u8] = br"{\rtf1 text and then \u";
const RTF_MIDHEX: &[u8] = br"{\rtf1 text and then \'";
const RTF_MIDHEX2: &[u8] = br"{\rtf1 text and then \'e";
const RTF_GREEDY: &[u8] = br"{\rtf1\uc2147483647 \u233?? tail}";

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

/// 0.11.0's four new readers, each against the fixture its own library wrote.
///
/// The assertions are deliberately about what a person would see — the spine
/// order of a book, the chosen part of a mail, the decoded escape — rather than
/// about a substring being present somewhere. A reader that returns the right
/// words in the wrong order passes the first kind of test and fails a user.
#[test]
fn the_new_formats_yield_the_text_a_person_would_see() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "book.epub",
            &[
                "The numbat lighthouse inventory was copied out twice before anyone trusted it.",
                "By then the saiga ferry timetable had been pinned to the wall for a season.",
                "What remained was the lemur harbour survey, unfinished and still unread.",
            ],
            // The chapters are XHTML inside the archive; none of it survives.
            &["<html", "<body", "xmlns", "<?xml"],
        ),
        (
            "notes.rtf",
            &[
                "The binturong pier maintenance log was reopened after the winter inspection.",
                // The font and colour tables are groups the reader skips whole,
                // so neither their control words nor their contents appear.
                "urgent",
                "deferred",
            ],
            &["fonttbl", "colortbl", "\\par", "Times New Roman", "rtf1"],
        ),
        (
            "message.eml",
            &[
                "The serval dispatch confirmation arrived before the café closed.",
                "Two crates are still unaccounted for and the ferry leaves at six.",
            ],
            &[
                // The html alternative, its base64, the quoted-printable of the
                // plain part, and the headers that are nobody's search term.
                "This html alternative must never be the extracted text.",
                "Content-Transfer-Encoding",
                "=C3=A9",
                "X-Mailer",
                "Message-ID",
                "=?utf-8?",
            ],
        ),
        (
            "archive.mbox",
            &[
                "The aardwolf shipping manifest lists eleven pallets, not nine.",
                "The kinkajou warehouse audit closed with two open findings.",
                "Filed the vicuna freight receipt against the wrong quarter.",
            ],
            &[
                "Ignore this markup branch entirely.",
                "=?utf-8?",
                "X-Mailer",
            ],
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
                "{name} still carries {syntax:?}:\n{text}"
            );
        }
    }
}

/// A book is read in the order the book gives, which is the whole reason the
/// spine is consulted at all.
///
/// The fixture's chapter files are named so that filename order and spine order
/// disagree: sorted by name they are alpha, mike, zulu, and the book says zulu,
/// alpha, mike. A reader that listed the archive would pass every phrase
/// assertion above and still hand back the chapters shuffled.
#[test]
fn an_epub_is_read_in_spine_order_not_filename_order() {
    let book = text_of("book.epub");

    let first = book.find("numbat lighthouse inventory").unwrap();
    let second = book.find("saiga ferry timetable").unwrap();
    let third = book.find("lemur harbour survey").unwrap();
    assert!(
        first < second && second < third,
        "the chapters came out in filename order rather than spine order:\n{book}"
    );

    // Each chapter is marked with the file it came from, the way a slide and a
    // notebook cell are marked.
    assert!(
        book.contains("# zulu.xhtml"),
        "a chapter is not named:\n{book}"
    );
    assert!(
        book.find("# zulu.xhtml").unwrap() < first,
        "the marker is not above the chapter it names:\n{book}"
    );
}

/// The two escapes that carry every non-ASCII character an RTF document holds.
///
/// `\'e9` is a byte in the document's codepage and `舒?` is a Unicode code
/// point followed by an ASCII fallback for readers that cannot show it. Getting
/// the first wrong turns every accented word into mojibake; getting the second
/// wrong prints the character and then a stray `?` after it.
#[test]
fn rtf_escapes_decode_to_the_characters_they_name() {
    let text = text_of("notes.rtf");

    assert!(
        text.contains("café"),
        "the \\'hh escape did not decode through the codepage:\n{text}"
    );
    assert!(
        text.contains("café wing — modestly"),
        "the \\u escape did not decode, or its ? fallback was kept:\n{text}"
    );
    assert!(
        !text.contains("—?"),
        "the \\u fallback character was indexed as well as the character:\n{text}"
    );
}

/// A message is its five headers and its body, and nothing else.
#[test]
fn a_message_keeps_the_headers_worth_searching_and_drops_the_rest() {
    let text = text_of("message.eml");

    // The Subject travelled folded across two lines and RFC 2047 encoded. Both
    // have to be undone before the word is searchable.
    assert!(
        text.contains(
            "Subject: Résumé of the quarterly walkthrough and the follow-up items we \
                       agreed on site"
        ),
        "the Subject was not unfolded and decoded:\n{text}"
    );
    for header in [
        "From: Ines Okonkwo",
        "To: Harbour Office",
        "Cc: Records",
        "Date: Tue, 11 Jun",
    ] {
        assert!(text.contains(header), "{header:?} is missing:\n{text}");
    }

    // Named, not decoded: the attachment is findable by its filename and its
    // bytes never reach a second format reader.
    assert!(
        text.contains("# Attachment: manifest.txt"),
        "the attachment was not named:\n{text}"
    );
    assert!(
        !text.contains("crate 42: missing"),
        "an attachment's contents were indexed:\n{text}"
    );

    // An mbox names each message, because a line number cannot say which of
    // three hundred messages a hit is in.
    let archive = text_of("archive.mbox");
    assert!(
        archive.contains("# Message 1: North route loading"),
        "a message is not named by its subject:\n{archive}"
    );
    assert!(
        archive.contains("# Message 2: Inventaire de l'entrepôt"),
        "an encoded-word subject was not decoded in the marker:\n{archive}"
    );
    let order: Vec<usize> = (1..=3)
        .map(|n| archive.find(&format!("# Message {n}: ")).unwrap())
        .collect();
    assert!(
        order.windows(2).all(|w| w[0] < w[1]),
        "the messages came out of file order:\n{archive}"
    );
}

/// Every way one of the new formats can be unreadable, and the same answer to
/// all of them: skipped, not an error, not a panic, not a hang.
///
/// The RTF cases are the ones worth being careful about. Its reader walks a
/// character stream with a brace depth, and unbalanced braces, a file that ends
/// mid-control-word and a `\uc` claiming an enormous fallback are each a way to
/// write a loop that never ends.
#[test]
fn an_unreadable_new_format_is_skipped_rather_than_fatal() {
    let book = fs::read(fixtures().join("book.epub")).unwrap();

    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("truncated.epub", book[..400].to_vec()),
        ("plain.epub", b"this is not an archive at all".to_vec()),
        // A ZIP with no container.xml is a valid archive and not a book.
        (
            "notabook.epub",
            fs::read(fixtures().join("notes.docx")).unwrap(),
        ),
        (
            "notrtf.rtf",
            b"a plain sentence saved with the wrong extension".to_vec(),
        ),
        (
            "unbalanced.rtf",
            br"{\rtf1{{{{\fonttbl a paragraph".to_vec(),
        ),
        // A header block with no empty line after it has no body, and is far
        // likelier to be a truncated file than a message.
        (
            "noblank.eml",
            b"Subject: nothing follows this line".to_vec(),
        ),
        ("empty.eml", b"\n\n".to_vec()),
        // An mbox is defined by its separator; a file without one is not one.
        (
            "nosep.mbox",
            b"Subject: this never had a From line\n\nbody\n".to_vec(),
        ),
        ("binary.mbox", (0u8..=255).cycle().take(4096).collect()),
    ];

    for (name, bytes) in cases {
        let path = PathBuf::from(name);
        assert!(
            chunk::extract(&path, &bytes).is_none(),
            "{name} was read as text rather than skipped"
        );
    }

    // A zip bomb wearing a book's extension spends from the same decompression
    // budget every archive format shares.
    let bomb = fs::read(fixtures().join("bomb.docx")).unwrap();
    assert!(
        chunk::extract(&PathBuf::from("bomb.epub"), &bomb).is_none(),
        "the archive cap does not hold for EPUB"
    );

    // A document truncated partway through is a different case, and `None` is
    // the wrong answer to it: the text before the truncation is real text, and
    // a reader that threw it away would lose the readable nine tenths of a file
    // over its last line. What these have to do is terminate. Each one is a way
    // to write a scan that runs off the end of its input or loops on a
    // parameter the document handed it.
    let terminates: Vec<(&str, Vec<u8>)> = vec![
        ("midword.rtf", RTF_MIDWORD.to_vec()),
        ("midhex.rtf", RTF_MIDHEX.to_vec()),
        ("midhex2.rtf", RTF_MIDHEX2.to_vec()),
        // `\uc` claiming more fallback characters than the file holds.
        ("greedy.rtf", RTF_GREEDY.to_vec()),
        // A group that opens ten thousand times and closes none of them.
        ("deep.rtf", {
            let mut bytes = RTF_OPEN.to_vec();
            bytes.extend(std::iter::repeat_n(b'{', 10_000));
            bytes.extend(b" tail");
            bytes
        }),
        // A multipart that names a boundary and then stops at its first one.
        (
            "unclosed.eml",
            b"Content-Type: multipart/mixed; boundary=b\n\n--b\n".to_vec(),
        ),
    ];
    for (name, bytes) in terminates {
        // The assertion is that this returns at all. Whether it found any text
        // is the file's business rather than the reader's.
        let _ = chunk::extract(&PathBuf::from(name), &bytes);
    }
}

/// The portal's Files view says which reader parsed a file, because "that book
/// came out empty" and "that book was read as binary and skipped" look
/// identical in a file list and are different problems.
#[test]
fn the_new_formats_name_the_reader_that_parsed_them() {
    let cases = [
        ("book.epub", "epub"),
        ("notes.rtf", "rtf"),
        ("message.eml", "mail"),
        ("archive.mbox", "mail"),
    ];
    for (name, reader) in cases {
        assert_eq!(
            chunk::reader_of(&PathBuf::from(name)),
            reader,
            "{name} names the wrong reader"
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
        "book.epub",
        "notes.rtf",
        "message.eml",
        "archive.mbox",
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
        ("numbat lighthouse inventory", "book.epub"),
        ("binturong pier maintenance log", "notes.rtf"),
        ("serval dispatch confirmation", "message.eml"),
        ("aardwolf shipping manifest pallets", "archive.mbox"),
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

    // An edited document is re-read, and its old text stops being an answer —
    // the promise every other format already makes, made for documents.
    fs::write(
        corpus.path().join("analysis.ipynb"),
        r#"{"cells":[{"cell_type":"markdown","metadata":{},
            "source":["The dugong recalibration note replaced the earlier writeup.\n"]}],
            "metadata":{},"nbformat":4,"nbformat_minor":5}"#,
    )
    .unwrap();
    let report = s
        .index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    assert_eq!(report.indexed, 1, "only the changed file re-indexes");

    let hits = s.search("dugong recalibration note", 3).unwrap();
    assert!(
        hits.first()
            .is_some_and(|h| h.path.ends_with("analysis.ipynb")),
        "the edited notebook's new text is not searchable: {hits:#?}"
    );
    // The file still ranks for a loosely related question — it is the only
    // notebook in the corpus. What must be gone is the text itself.
    let hits = s.search("capybara regression writeup", 5).unwrap();
    assert!(
        !hits.iter().any(|h| h.text.contains("capybara")),
        "the notebook's replaced text is still in the store: {hits:#?}"
    );
}
