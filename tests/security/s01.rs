//! S01: a file that aborts a parser costs one refused file, never the process.

#[allow(unused_imports)]
use super::*;

/// The three shapes that abort tree-sitter's block grammars at depth 300, and
/// one ordinary file.
fn nesting_corpus(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let d = 300;
    std::fs::write(dir.join("quotes.md"), "> ".repeat(d) + "x\n").unwrap();
    let list: String = (0..d).map(|i| "  ".repeat(i) + "- a\n").collect();
    std::fs::write(dir.join("list.md"), list).unwrap();
    let seq: String =
        (0..d).map(|i| "  ".repeat(i) + "-\n").collect::<String>() + &"  ".repeat(d) + "- x\n";
    std::fs::write(dir.join("seq.yaml"), seq).unwrap();
    std::fs::write(
        dir.join("notes.md"),
        "# Notes\n\nAn ordinary file beside them.\n",
    )
    .unwrap();
}

const HOSTILE: [&str; 3] = ["quotes.md", "list.md", "seq.yaml"];

fn crashed_rows(refused: &Value) -> Vec<(String, String)> {
    refused["stores"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["rows"].as_array().cloned().unwrap_or_default())
        .filter(|r| r["class"] == "crashed")
        .map(|r| {
            (
                r["path"].as_str().unwrap_or_default().to_string(),
                r["rule"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn begins(home: &Path, name: &str) -> usize {
    let mut n = 0;
    for entry in walk(&home.join(".semlith")) {
        if entry.file_name().is_some_and(|f| f == "parse-journal") {
            let text = std::fs::read_to_string(&entry).unwrap_or_default();
            n += text
                .lines()
                .filter(|l| l.contains("\"op\":\"begin\"") && l.contains(name))
                .count();
        }
    }
    n
}

/// S01: the three files that aborted 0.39.0 leave the CLI and the daemon
/// running, each refused with a parser-crash reason, and neither a catch-up
/// nor a restart parses them again. The nesting cap is switched off (debug
/// builds honour the switch) so the real C abort reaches the parse worker.
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s01_a_file_that_aborts_a_parser_is_refused_once_and_never_loops() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    nesting_corpus(&corpus);
    let cap_off = [("SEMLITH_NO_NESTING_CAP", "1")];

    let out = run(&home, &["index", corpus.to_str().unwrap()], &cap_off);
    assert!(
        out.status.success(),
        "semlith index did not survive the corpus: {:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run(&home, &["refused", "--json"], &cap_off);
    let refused: Value = serde_json::from_slice(&out.stdout).expect("refused --json");
    let rows = crashed_rows(&refused);
    for name in HOSTILE {
        let row = rows.iter().find(|(p, _)| p.ends_with(name));
        let (_, reason) =
            row.unwrap_or_else(|| panic!("{name} is not refused as crashed: {rows:?}"));
        assert!(reason.starts_with("parser crashed"), "{name}: {reason}");
    }
    let before: Vec<usize> = HOSTILE.iter().map(|n| begins(&home, n)).collect();

    // Catch-up on start, then a restart: the daemon stays up, the refusals
    // stand, and the journal shows no second parse of any of them.
    for round in 0..2 {
        let mut daemon = Daemon::start(&home, &cap_off);
        let listed = until(Duration::from_secs(120), || {
            crashed_rows(&daemon.api("/api/refused")).len() >= HOSTILE.len()
        });
        assert!(daemon.alive(), "round {round}: the daemon died on catch-up");
        assert!(
            listed,
            "round {round}: the portal does not list the refusals"
        );
        let (status, _) = daemon.send(
            "GET",
            "/api/about",
            &format!("Semlith-Token: {}\r\n", daemon.token),
            "",
        );
        assert_eq!(status, 200, "round {round}: the daemon does not answer");
        drop(daemon);
    }
    let after: Vec<usize> = HOSTILE.iter().map(|n| begins(&home, n)).collect();
    assert_eq!(before, after, "a refused file was handed to a parser again");

    // doctor names them.
    let out = run(&home, &["doctor"], &cap_off);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("crashed a parser and are refused"), "{said}");
}

/// S01, the shipped default: the nesting cap gives the same files no graph
/// and indexes their text, so nothing crashes at all.
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s01_deep_nesting_is_indexed_as_text_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    nesting_corpus(&corpus);
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(out.status.success(), "{:?}", out.status);
    let out = run(&home, &["files"], &[]);
    let listed = String::from_utf8_lossy(&out.stdout);
    for name in HOSTILE {
        assert!(listed.contains(name), "{name} was not indexed:\n{listed}");
    }
}
