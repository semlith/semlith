//! The client setup stanzas, read out of `docs/clients.md` at compile time.
//!
//! The portal's Agents view shows a copy-ready stanza per client, and there is
//! exactly one way for that to stay true as flags change: it has to be the
//! same text `tests/clients.rs` executes. So `docs/clients.md` is the source,
//! embedded with `include_str!` and parsed with the same rules the test uses — a heading
//! per client, then its fenced blocks.
//!
//! A stanza retyped into Rust would be a second copy that looks right for
//! exactly as long as nobody edits one of them.

use std::sync::OnceLock;

const CLIENTS_DOC: &str = include_str!("../docs/clients.md");

/// The heading the stanzas live under.
const SECTION: &str = "### Setting it up in your client";

/// The heading the HTTP stanzas live under.
const HTTP_SECTION: &str = "### Connecting over HTTP";

/// What a stanza names where the agent key goes.
///
/// The variable rather than the key from 0.14.0: a configuration file that
/// names it keeps working across every rotation, and one that carries the key
/// itself goes stale the moment somebody rotates. The portal substitutes the
/// literal value into this only when the reader has pressed Reveal.
const KEY_PLACEHOLDER: &str = "${SEMLITH_AGENT_KEY}";

/// One pasteable block.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Stanza {
    /// The fence's language: `json`, `toml`, `sh`, `yaml`.
    pub format: String,
    pub text: String,
}

/// One client, its note, and every stanza `docs/clients.md` gives for it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Client {
    pub name: String,
    /// Where the file lives and anything the client needs told about it, as
    /// one line of plain text.
    pub note: String,
    /// `terminal`, `editor` or `desktop`, read from `docs/clients.md`'s own grouping
    /// so the portal's tabs and the documentation cannot disagree about which
    /// kind of thing a client is.
    pub group: String,
    pub stanzas: Vec<Stanza>,
}

/// The HTTP stanzas, with `key` substituted for `docs/clients.md`'s placeholder.
///
/// One template for every client rather than one per client: the endpoint and
/// the header are the same wherever they are pasted, and a per-client copy
/// would be twelve places for one URL to go stale.
pub fn http_stanzas(key: &str) -> Vec<Stanza> {
    static PARSED: OnceLock<Vec<Stanza>> = OnceLock::new();
    PARSED
        .get_or_init(|| parse_http(CLIENTS_DOC))
        .iter()
        .map(|stanza| Stanza {
            format: stanza.format.clone(),
            text: stanza.text.replace(KEY_PLACEHOLDER, key),
        })
        .collect()
}

fn parse_http(readme: &str) -> Vec<Stanza> {
    let Some(start) = readme.find(HTTP_SECTION) else {
        return Vec::new();
    };
    let rest = &readme[start + HTTP_SECTION.len()..];
    let section = match rest.find("\n## ") {
        Some(end) => &rest[..end],
        None => rest,
    };
    let mut out = Vec::new();
    let mut lines = section.lines();
    while let Some(line) = lines.next() {
        let Some(format) = line.strip_prefix("```") else {
            continue;
        };
        let mut text = String::new();
        for body in lines.by_ref() {
            if body.starts_with("```") {
                break;
            }
            text.push_str(body);
            text.push('\n');
        }
        out.push(Stanza {
            format: format.trim().to_string(),
            text,
        });
    }
    out
}

/// Every documented client, parsed once.
pub fn clients() -> &'static [Client] {
    static PARSED: OnceLock<Vec<Client>> = OnceLock::new();
    PARSED.get_or_init(|| parse(CLIENTS_DOC))
}

fn parse(readme: &str) -> Vec<Client> {
    let Some(start) = readme.find(SECTION) else {
        return Vec::new();
    };
    let rest = &readme[start + SECTION.len()..];
    // The section ends at the next heading of its own level or above. Stopping
    // only at `## ` swept in the `### Connecting over HTTP` section below it,
    // and its example stanzas were then read as the last client's own.
    let end = ["\n## ", "\n### "]
        .iter()
        .filter_map(|marker| rest.find(marker))
        .min();
    let section = match end {
        Some(end) => &rest[..end],
        None => rest,
    };

    let mut out: Vec<Client> = Vec::new();
    let mut group = String::from("terminal");
    let mut lines = section.lines().peekable();
    while let Some(line) = lines.next() {
        // `docs/clients.md` groups its clients under `#### Terminal`, `#### Editors`
        // and `#### Desktop apps`. Read from there rather than from a table in
        // this file, so the grouping has one source.
        if let Some(heading) = line.strip_prefix("#### ") {
            group = match heading.trim() {
                "Editors" => "editor",
                "Desktop apps" => "desktop",
                _ => "terminal",
            }
            .to_string();
            continue;
        }
        // A client's block starts at its bold name at the start of a line.
        let Some(after) = line.strip_prefix("**") else {
            continue;
        };
        let Some((name, tail)) = after.split_once("**") else {
            continue;
        };

        // The note runs to the first blank line, and is joined into one line:
        // the portal shows it under the client's name, not as a paragraph.
        let mut note = tail.trim_start_matches([' ', '—', '-']).trim().to_string();
        for next in lines.by_ref() {
            if next.trim().is_empty() {
                break;
            }
            if next.starts_with("```") || next.starts_with("**") {
                break;
            }
            note.push(' ');
            note.push_str(next.trim());
        }

        let mut stanzas = Vec::new();
        // Then every fenced block until the next client or the end.
        while let Some(peeked) = lines.peek() {
            // The next client, or the heading of the next group — either way
            // this client's blocks have ended.
            if peeked.starts_with("**") || peeked.starts_with("#### ") {
                break;
            }
            let line = lines.next().expect("just peeked");
            let Some(format) = line.strip_prefix("```") else {
                continue;
            };
            let mut text = String::new();
            for body in lines.by_ref() {
                if body.starts_with("```") {
                    break;
                }
                text.push_str(body);
                text.push('\n');
            }
            stanzas.push(Stanza {
                format: format.trim().to_string(),
                text,
            });
        }

        if !stanzas.is_empty() {
            out.push(Client {
                name: name.trim().to_string(),
                note: squeeze(&note),
                group: group.clone(),
                stanzas,
            });
        }
    }
    out
}

/// Markdown emphasis and backticks are noise in a portal panel that is not
/// rendering markdown.
fn squeeze(note: &str) -> String {
    note.replace("**", "")
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// If this ever comes back empty, the Agents view silently shows nothing
    /// and `docs/clients.md` has been restructured under it.
    #[test]
    fn every_documented_client_is_parsed_with_at_least_one_stanza() {
        let parsed = clients();
        assert!(
            parsed.len() >= 27,
            "only {} clients parsed out of the CLIENTS_DOC",
            parsed.len()
        );
        for client in parsed {
            assert!(!client.name.is_empty());
            assert!(!client.stanzas.is_empty(), "{} has no stanza", client.name);
            for stanza in &client.stanzas {
                assert!(
                    !stanza.text.trim().is_empty(),
                    "{} has an empty stanza",
                    client.name
                );
            }
        }
    }

    /// The release's promise, asserted against the text the portal hands out:
    /// no client stanza carries a store path.
    #[test]
    fn no_parsed_stanza_carries_a_store_path() {
        for client in clients() {
            for stanza in &client.stanzas {
                assert!(
                    !stanza.text.contains("--store"),
                    "{} still passes --store:\n{}",
                    client.name,
                    stanza.text
                );
            }
        }
    }

    /// Every client is in one of the three groups the portal renders as tabs,
    /// and the counts sum to the number of clients `docs/clients.md` documents.
    #[test]
    fn every_client_carries_a_group_read_from_the_readme() {
        let parsed = clients();
        let mut counts = std::collections::BTreeMap::new();
        for client in parsed {
            assert!(
                ["terminal", "editor", "desktop"].contains(&client.group.as_str()),
                "{} is in group {:?}",
                client.name,
                client.group
            );
            *counts.entry(client.group.as_str()).or_insert(0usize) += 1;
        }
        assert_eq!(
            counts.values().sum::<usize>(),
            parsed.len(),
            "the groups do not cover every client"
        );
        assert_eq!(counts.len(), 3, "a group is empty: {counts:?}");
    }

    /// The HTTP stanzas carry the live key rather than the placeholder, and
    /// name the endpoint the daemon actually serves.
    #[test]
    fn the_http_stanzas_carry_the_key_they_are_given() {
        let stanzas = http_stanzas("sml_abc123");
        assert!(!stanzas.is_empty(), "no HTTP stanza was parsed");
        for stanza in &stanzas {
            assert!(
                stanza.text.contains("127.0.0.1:7365/mcp"),
                "an HTTP stanza does not name the endpoint:\n{}",
                stanza.text
            );
            assert!(
                stanza.text.contains("Bearer sml_abc123"),
                "an HTTP stanza does not carry the key it was given:\n{}",
                stanza.text
            );
            assert!(
                !stanza.text.contains(KEY_PLACEHOLDER),
                "an HTTP stanza still carries the placeholder:\n{}",
                stanza.text
            );
        }
    }

    #[test]
    fn claude_code_is_the_first_client_and_has_both_of_its_forms() {
        let first = &clients()[0];
        assert_eq!(first.name, "Claude Code");
        let formats: Vec<&str> = first.stanzas.iter().map(|s| s.format.as_str()).collect();
        // The endpoint first, as a committed file and as a command, then the
        // subprocess form for anyone who wants no key at all.
        assert_eq!(formats, vec!["json", "sh", "sh"]);
        assert!(
            first.stanzas[0].text.contains("http://127.0.0.1:7365/mcp"),
            "the first stanza is not the endpoint:\n{}",
            first.stanzas[0].text
        );
        assert_eq!(
            first.stanzas[2].text.trim(),
            "claude mcp add semlith -- semlith mcp"
        );
    }
}
