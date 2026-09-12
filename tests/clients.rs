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
use std::path::{Path, PathBuf};
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

/// Since 0.9.0 a stanza carries no path at all: the server resolves its own
/// stores from the registry. This is the whole change, so it is asserted
/// directly rather than left to be noticed.
const NO_PATHS: &str = "--store";

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

/// A stanza that still carries a store path is a stanza that pins an agent to
/// one repository. The release's promise is that indexing a second repository
/// needs no edit to any client's configuration file, and that is only true if
/// none of them names a store.
#[test]
fn no_stanza_carries_a_store_path() {
    // The prose above the stanzas still explains `--store`, which is still a
    // supported flag. It is the pasteable blocks that must not carry one.
    for (language, body) in blocks(&section()) {
        if !names_semlith(&body) {
            continue;
        }
        assert!(
            !body.contains(NO_PATHS),
            "a {language} stanza still passes {NO_PATHS}; since 0.9.0 the \
             server resolves its own stores:\n{body}"
        );
        assert_eq!(
            args_of(&body),
            vec!["mcp".to_string()],
            "a {language} stanza does not run a bare `semlith mcp`:\n{body}"
        );
    }
}

/// Whatever the client's file format, the server it launches is semlith, and
/// the command line it launches it with has to be one semlith answers — now
/// with no store flag, against whatever the registry holds.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn every_stanza_launches_a_server_that_answers() {
    let home = two_registered_stores();

    let stanzas = blocks(&section());
    for (language, body) in &stanzas {
        assert!(
            names_semlith(body),
            "a {language} stanza does not name the semlith binary:\n{body}"
        );
    }
    // Every client has a stanza, and several have two — so the section can
    // never quietly shrink to one example that happens to still work.
    assert!(
        stanzas.len() >= CLIENTS.len(),
        "{} stanzas for {} clients",
        stanzas.len(),
        CLIENTS.len()
    );

    let named = answers(&home, &["mcp".to_string()]);
    // The point of the change: one stanza, every store. A server that opened
    // only one of them would answer every question about the other with
    // nothing, which reads to an agent as "the corpus does not discuss this".
    for store in ["api", "cli"] {
        assert!(
            named.contains(store),
            "`semlith mcp` with no flags did not open the registered store \
             {store}: {named}"
        );
    }
}

/// Drive a server started with `argv` through the opening a client performs,
/// then ask it what it opened. Returns the text of `semlith_stats`.
fn answers(home: &Path, argv: &[String]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(argv)
        .env("SEMLITH_HOME", home)
        .env("HOME", home)
        .env("SEMLITH_MODEL_CACHE", real_model_cache())
        .env_remove("SEMLITH_STORE")
        // Started somewhere with no `.semlith` in it, so the registry is the
        // only thing that can be answering.
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("{argv:?} did not start: {e}"));
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());

    let mut send = |id: u64, method: &str, params: serde_json::Value| -> serde_json::Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert!(!line.trim().is_empty(), "{argv:?} closed on {method}");
        serde_json::from_str(&line).unwrap()
    };

    let handshake = json!({ "protocolVersion": "2025-11-25", "capabilities": {} });
    let hello = send(1, "initialize", handshake);
    assert!(
        hello["result"]["protocolVersion"].is_string(),
        "{argv:?} did not complete a handshake: {hello}"
    );
    let listed = send(2, "tools/list", json!({}));
    assert!(
        listed["result"]["tools"]
            .as_array()
            .is_some_and(|t| !t.is_empty()),
        "{argv:?} listed no tools: {listed}"
    );

    let stats = send(3, "tools/call", json!({ "name": "semlith_stats" }));
    let said = stats["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    drop(stdin);
    let _ = child.wait();
    said
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

/// The arguments a stanza hands the binary: everything after the last mention
/// of it.
///
/// Read out of the stanza rather than compared against a constant: the README
/// is the thing under test, so the arguments have to come from it.
fn args_of(body: &str) -> Vec<String> {
    let tokens = tokens(body);
    let Some(binary) = tokens
        .iter()
        .rposition(|t| t == "semlith" || t.ends_with("/semlith"))
    else {
        return Vec::new();
    };
    // Stops at the subcommand: a client's own keys can follow the argument
    // list in the same block, and those are its configuration, not semlith's
    // command line.
    let Some(last) = tokens[binary + 1..]
        .iter()
        .rposition(|t| t == "mcp")
        .map(|i| binary + 1 + i)
    else {
        return Vec::new();
    };
    tokens[binary + 1..=last]
        .iter()
        // Braces, `=`, `--` and YAML dashes are the config formats' own
        // punctuation, and `args` is their word for "what follows". None of
        // them is an argument the binary ever sees.
        .filter(|t| t.chars().any(char::is_alphanumeric) && *t != "args")
        .cloned()
        .collect()
}

/// A stanza's words, with JSON, TOML, YAML and shell punctuation stripped, so
/// one reader serves every format the clients use.
fn tokens(body: &str) -> Vec<String> {
    body.replace(['"', ',', '[', ']', ':'], " ")
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// A store home with two registered stores in it, so "the server opened
/// everything the registry holds" is a decidable claim rather than a hope.
fn two_registered_stores() -> PathBuf {
    // Leaked deliberately: the servers under test outlive any guard scope here,
    // and a test process is about to end anyway.
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();

    for (name, text) in [
        ("api", "Rust ownership gives every value exactly one owner."),
        (
            "cli",
            "Sourdough rises because a starter of flour and water ferments.",
        ),
    ] {
        let corpus = dir.path().join(name);
        fs::create_dir_all(&corpus).unwrap();
        fs::write(corpus.join("notes.md"), text).unwrap();

        // No --store: this is the resolution under test making the store.
        let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg("index")
            .arg(".")
            .arg("--quiet")
            .current_dir(&corpus)
            .env("SEMLITH_HOME", &home)
            .env("HOME", &home)
            .env("SEMLITH_MODEL_CACHE", real_model_cache())
            .env_remove("SEMLITH_STORE")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "indexing {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    home
}

/// Where this machine already keeps the model weights, so a reset HOME does
/// not mean downloading 52 MB again.
fn real_model_cache() -> PathBuf {
    if let Ok(dir) = std::env::var("SEMLITH_MODEL_CACHE") {
        return PathBuf::from(dir);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("semlith")
        .join("models")
}
