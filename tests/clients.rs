//! The README's client setup stanzas, executed.
//!
//! None of the clients they are written for can be installed in CI, so the
//! check available is the one that matters most anyway: that what the
//! documentation prints is a command line this binary answers on. A flag
//! renamed in the code and not in the README fails here rather than on somebody
//! else's first attempt.
//!
//! ```sh
//! cargo test --test clients -- --ignored
//! ```

use serde_json::json;
use std::fs;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

const README: &str = include_str!("../README.md");

/// The heading whose stanzas this file is about, and where they stop.
const SECTION: &str = "### Setting it up in your client";

/// Every client the release promises a stanza for.
const CLIENTS: [&str; 12] = [
    "Claude Code",
    "Claude Desktop",
    "OpenAI Codex",
    "GitHub Copilot in VS Code",
    "GitHub Copilot CLI",
    "Cursor",
    "Windsurf",
    "Zed",
    "Gemini CLI",
    "JetBrains",
    "Cline",
    "Goose",
];

/// What the README writes where a real store path goes.
const PLACEHOLDER: &str = "/path/to/.semlith";

#[test]
fn every_promised_client_has_a_stanza() {
    let section = section();
    for client in CLIENTS {
        assert!(
            section.contains(client),
            "the setup section says nothing about {client}"
        );
    }
    assert!(
        !blocks(&section).is_empty(),
        "the setup section has no code blocks at all"
    );
}

/// A stanza with a syntax error is a stanza nobody can paste. JSON is the
/// format most of these are in, and a stray comma is the commonest way to break
/// one.
#[test]
fn every_json_stanza_parses() {
    for (language, body) in blocks(&section()) {
        if language == "json" {
            serde_json::from_str::<serde_json::Value>(&body)
                .unwrap_or_else(|e| panic!("a json stanza does not parse: {e}\n{body}"));
        }
    }
}

/// Whatever the client's file format, the server it launches is semlith, and
/// the command line it launches it with has to be one semlith answers.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn every_stanza_launches_a_server_that_answers() {
    let store = a_real_store();

    let mut argvs: Vec<Vec<String>> = Vec::new();
    let stanzas = blocks(&section());
    for (language, body) in &stanzas {
        assert!(
            names_semlith(body),
            "a {language} stanza does not name the semlith binary:\n{body}"
        );
        let argv = argv_of(body).unwrap_or_else(|| {
            panic!("no `--store … mcp` command line in this {language} stanza:\n{body}")
        });
        if !argvs.contains(&argv) {
            argvs.push(argv);
        }
    }
    // Every client has a stanza, and several have two — so the section can
    // never quietly shrink to one example that happens to still work.
    assert!(
        stanzas.len() >= CLIENTS.len(),
        "{} stanzas for {} clients",
        stanzas.len(),
        CLIENTS.len()
    );
    assert!(!argvs.is_empty(), "no command line to run");

    for argv in &argvs {
        let real: Vec<String> = argv
            .iter()
            .map(|a| a.replace(PLACEHOLDER, store.to_str().unwrap()))
            .collect();
        answers(&real);
    }
}

/// Drive a server started with `argv` through the opening a client performs.
fn answers(argv: &[String]) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("{argv:?} did not start: {e}"));
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());

    let mut send = |id: u64, method: &str| -> serde_json::Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": { "protocolVersion": "2025-11-25", "capabilities": {} },
        });
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert!(!line.trim().is_empty(), "{argv:?} closed on {method}");
        serde_json::from_str(&line).unwrap()
    };

    let hello = send(1, "initialize");
    assert!(
        hello["result"]["protocolVersion"].is_string(),
        "{argv:?} did not complete a handshake: {hello}"
    );
    let listed = send(2, "tools/list");
    assert!(
        listed["result"]["tools"]
            .as_array()
            .is_some_and(|t| !t.is_empty()),
        "{argv:?} listed no tools: {listed}"
    );

    drop(stdin);
    let _ = child.wait();
}

/// The README's client setup section, up to the next top-level heading.
fn section() -> String {
    let start = README
        .find(SECTION)
        .unwrap_or_else(|| panic!("the README has no {SECTION:?} heading"));
    let rest = &README[start..];
    let end = rest[SECTION.len()..]
        .find("\n## ")
        .map(|i| i + SECTION.len())
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// `(language, body)` for every fenced code block in `text`.
fn blocks(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(language) = line.strip_prefix("```") else {
            continue;
        };
        let mut body = String::new();
        for line in lines.by_ref() {
            if line.starts_with("```") {
                break;
            }
            body.push_str(line);
            body.push('\n');
        }
        out.push((language.trim().to_string(), body));
    }
    out
}

/// Whether the block launches semlith rather than something else — a bare
/// `semlith` on the `PATH` or an absolute path ending in it.
fn names_semlith(body: &str) -> bool {
    tokens(body)
        .iter()
        .any(|t| t == "semlith" || t.ends_with("/semlith"))
}

/// The `--store … mcp` command line a stanza hands to the binary.
///
/// Read out of the stanza rather than compared against a constant: the README
/// is the thing under test, so the arguments have to come from it.
fn argv_of(body: &str) -> Option<Vec<String>> {
    let tokens = tokens(body);
    let first = tokens.iter().position(|t| t == "--store")?;
    let last = tokens.iter().rposition(|t| t == "mcp")?;
    if last <= first {
        return None;
    }
    Some(tokens[first..=last].to_vec())
}

/// A stanza's words, with JSON, TOML, YAML and shell punctuation stripped, so
/// one reader serves every format the clients use.
fn tokens(body: &str) -> Vec<String> {
    body.replace(['"', ',', '[', ']', ':'], " ")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// A store with something in it, so `tools/list` is answered by a server that
/// really opened one.
fn a_real_store() -> PathBuf {
    // Leaked deliberately: the servers under test outlive any guard scope here,
    // and a test process is about to end anyway.
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let corpus = dir.path().join("corpus");
    fs::create_dir_all(&corpus).unwrap();
    fs::write(
        corpus.join("ownership.md"),
        "Rust ownership gives every value exactly one owner.",
    )
    .unwrap();

    let store = corpus.join(".semlith");
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(&store)
        .arg("index")
        .arg(&corpus)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "indexing failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    store
}
