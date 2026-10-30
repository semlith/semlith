//! The content scan's second half: every secret-shaped match in a text, whether
//! each one is a live-looking value or a provable test dummy, how likely it is to
//! be real, what it looks like masked, and what redacting it leaves behind.
//!
//! The patterns themselves stay in [`crate::filter::SHAPES`], the one table the
//! scan has always read. What this module adds is the verdict on each match:
//!
//! - A **dummy** is a match a declared rule says cannot be a live key — a value
//!   from a short list of published documentation examples, a body that says
//!   `EXAMPLE` or `FAKE` or is one character repeated, a private-key header with
//!   no key under it. Declared, never guessed: a rule that let through anything
//!   that "looked random enough" would index the one real key that did not.
//! - Anything else is **live-looking**, and one live-looking match anywhere
//!   refuses the whole file, because the secret beside it — an AWS secret
//!   access key next to its `AKIA…` id — may have no prefix to find.
//! - Every match carries a **confidence** from 0 to 100 that it is a real,
//!   working secret, built from signals that are each shown beside it. It is an
//!   estimate for a person deciding whether to accept a file, never a gate: the
//!   refusal is decided by the dummy rules alone.
//!
//! No character of a match is ever written anywhere semlith writes. A match
//! leaves this module as its kind, its line, a mask (`ghp_…Xa9Q`) and a salted
//! fingerprint.

use crate::filter::{self, SHAPES};
use serde::Serialize;

/// One reason the confidence moved, shown beside it so a person can see why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Signal {
    /// `format`, `checksum`, `randomness`, `companion`, `location`, `expiry`
    /// or `dummy`.
    pub name: &'static str,
    /// `up` when it makes a real secret more likely, `down` when less.
    pub effect: &'static str,
    pub detail: String,
}

/// One secret-shaped match in a text.
#[derive(Debug, Clone, Serialize)]
pub struct Match {
    /// What it looks like, as the refusal line names it.
    pub kind: String,
    pub provider: &'static str,
    pub label: &'static str,
    pub line: u32,
    /// The issuer's prefix and the last four characters, never more.
    pub masked: String,
    /// The declared rule that makes it a test dummy, when one does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dummy: Option<&'static str>,
    /// 0–100: how likely this is a real, working secret. An estimate.
    pub confidence: u8,
    pub signals: Vec<Signal>,
    /// Where the match sits in the text. Never serialised, and never the
    /// value itself: the value is read out of the text only to fingerprint it
    /// or to redact it.
    #[serde(skip)]
    pub range: std::ops::Range<usize>,
}

impl Match {
    /// The marker redaction leaves in the text in place of the value.
    pub fn marker(&self) -> String {
        format!("[REDACTED:{} {}]", self.provider, self.label)
    }
}

/// Values published as documentation examples, which are dummies wherever
/// they are written. Split so this file does not hold a contiguous match.
const DOC_EXAMPLES: [&str; 4] = [
    concat!("AKIAIOSFODNN7", "EXAMPLE"),
    concat!("wJalrXUtnFEMI/K7MDENG/", "bPxRfiCYEXAMPLEKEY"),
    // The signature of RFC 7515's example JWS (A.1) and of jwt.io's sample
    // token, copied into a great many tests and docs.
    concat!("dBjftJeZ4CVP-mB92K27uhb", "UJU1p1r_wW1gFWFOEjXk"),
    concat!("SflKxwRJSMeKKF2QT4fwpM", "eJf36POk6yJV_adQssw5c"),
];

/// Words that mark a value as standing in for a key rather than being one.
const MARKERS: [&str; 11] = [
    "EXAMPLE",
    "TEST",
    "FAKE",
    "DUMMY",
    "PLACEHOLDER",
    "REDACTED",
    "MASKED",
    "SAMPLE",
    "INVALID",
    "NOTREAL",
    "TESTING",
];

/// Whole segments of a generic value that say it stands in for one (0.51.0).
/// A segment is what lies between `-`, `_`, `.` and whitespace, never `/` or
/// `+`: those are base64, and a key path or body holding `/test/` by chance
/// is still a key (one of the audit's drafts lost exactly that).
const TEST_WORDS: [&str; 13] = [
    "test",
    "fake",
    "mock",
    "dummy",
    "sample",
    "example",
    "placeholder",
    "changeme",
    "your",
    "invalid",
    "notreal",
    "redacted",
    "xxx",
];

/// What a line says to mark its match as allowed, for every tool a repository
/// may already be tagged for (0.51.0, rule (e)).
const ALLOW_TAGS: [&str; 4] = [
    "semlith:allow",
    "gitleaks:allow",
    "pragma: allowlist secret",
    "trufflehog:ignore",
];

/// Every secret-shaped match in `text`, in the order they appear.
///
/// `path` is used for the location signal and to decide whether the unquoted
/// form of the generic rule applies (it does not in code); it may be empty,
/// which reads as a file of unknown kind and applies every rule.
pub fn scan(path: &str, text: &str) -> Vec<Match> {
    let (each, assignment, env) = filter::shapes();
    let mut found: Vec<Match> = Vec::new();
    // Every range taken so far, by start. They never overlap, so the one that
    // starts last before a candidate ends is the only one that can overlap it:
    // a lookup, where a scan of every earlier match made a file planted with
    // matches quadratic (audit S18).
    let mut taken: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    let overlaps = |taken: &std::collections::BTreeMap<usize, usize>,
                    at: &std::ops::Range<usize>| {
        taken
            .range(..at.end)
            .next_back()
            .is_some_and(|(_, end)| *end > at.start)
    };
    for (index, regex) in each.iter().enumerate() {
        let shape = &SHAPES[index];
        // The one row with no literal for the regex engine to skip to; its
        // `Q~` is required, and a text without one is most texts.
        if shape.provider == "azure" && !text.contains("Q~") {
            continue;
        }
        // A row with a group names only its secret part: the password of a
        // connection string, the token of a header.
        let ranges: Vec<std::ops::Range<usize>> = if regex.captures_len() > 1 {
            regex
                .captures_iter(text)
                .filter_map(|c| c.get(1))
                .map(|m| m.range())
                .collect()
        } else {
            regex.find_iter(text).map(|m| m.range()).collect()
        };
        for mut range in ranges {
            // Table order decides a match two rows both claim: `sk-ant-` is an
            // `sk-` too, and the Anthropic row comes first.
            if overlaps(&taken, &range) {
                continue;
            }
            if shape.provider == "pem" && shape.label == "private key" {
                range = pem_block(text, range);
            }
            let Some(m) = judge(path, text, index, range) else {
                continue;
            };
            taken.insert(m.range.start, m.range.end);
            found.push(m);
        }
    }
    let unquoted = unquoted_applies(path);
    for (rule, quoted) in [(assignment, true), (env, false)] {
        if !quoted && !unquoted {
            continue;
        }
        for caps in rule.captures_iter(text) {
            let (Some(name), Some(value)) = (caps.get(1), caps.get(2)) else {
                continue;
            };
            let Some(named) = credential_name(name.as_str()) else {
                continue;
            };
            let raw = value.as_str();
            // Twenty characters, as before 0.51.0; twelve for a password.
            if (!named.password && raw.chars().count() < 20)
                || filter::is_placeholder(raw)
                || filter::entropy(raw) < filter::MIN_ENTROPY
            {
                continue;
            }
            let Some(shape) = value_shape(raw, named.password) else {
                continue;
            };
            let range = value.range();
            if overlaps(&taken, &range) {
                continue;
            }
            taken.insert(range.start, range.end);
            found.push(judge_assignment(path, text, range, &named, shape));
        }
    }
    found.sort_by_key(|m| m.range.start);
    // Line numbers in one pass over the text, in match order, rather than a
    // count from the top of the file for every match.
    let (mut line, mut at) = (1u32, 0usize);
    for m in &mut found {
        line += text.as_bytes()[at..m.range.start]
            .iter()
            .filter(|b| **b == b'\n')
            .count() as u32;
        at = m.range.start;
        m.line = line;
    }
    found
}

/// The start of a window of at most `bytes` ending at `at`, on a character
/// boundary. The context checks look a few lines around a match, and on a
/// file that is one enormous line "a few lines" was the whole file, read
/// again for every match.
fn window_start(text: &str, at: usize, bytes: usize) -> usize {
    let mut start = at.saturating_sub(bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    start
}

/// How far around a match its context is read.
const CONTEXT_BYTES: usize = 16 * 1024;

/// Whether a set of matches refuses its file: any one that is not a dummy.
pub fn refuses(matches: &[Match]) -> bool {
    matches.iter().any(|m| m.dummy.is_none())
}

/// `text` with every match in `matches` replaced by its marker, line numbers
/// kept: a match that spans lines leaves as many line breaks as it had.
pub fn redact(text: &str, matches: &[Match]) -> String {
    let mut ranges: Vec<&Match> = matches.iter().collect();
    ranges.sort_by_key(|m| m.range.start);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for m in ranges {
        if m.range.start < at {
            continue;
        }
        out.push_str(&text[at..m.range.start]);
        out.push_str(&m.marker());
        for _ in text[m.range.clone()].matches('\n') {
            out.push('\n');
        }
        at = m.range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// A match's fingerprint under a store's salt: what an acceptance remembers
/// instead of the value. Keyed blake3, so the store never holds anything a
/// value could be recovered or confirmed from without the salt beside it.
pub fn fingerprint(salt: &[u8; 32], text: &str, m: &Match) -> String {
    print(salt, &text[m.range.clone()])
}

fn print(salt: &[u8; 32], value: &str) -> String {
    blake3::keyed_hash(salt, value.as_bytes()).to_hex()[..32].to_string()
}

/// How bad it would be to index one not-indexed row, for the review list
/// (0.35.0): `{risk, band, likely, tone, kind, why, evidence, suggest}`.
///
/// Read off the verdict this module already gave each match — its dummy rule
/// and its confidence — and nothing else: there is no second classifier. The
/// matches are taken as stored (masked, serialised), which is the one shape
/// both the not-indexed table and the scan plan hold, so the two lists the
/// portal draws cannot score one file two ways. `evidence` is the match's
/// mask and line, never its value.
pub fn assess(
    class: &str,
    path: &str,
    matches: &[serde_json::Value],
    confidence: Option<u8>,
) -> serde_json::Value {
    let text = |m: &serde_json::Value, key: &str| {
        m.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let live: Vec<&serde_json::Value> = matches
        .iter()
        .filter(|m| m.get("dummy").is_none_or(serde_json::Value::is_null))
        .collect();
    let first = live.first().copied().or_else(|| matches.first());
    let kind_of = |m: &serde_json::Value| match text(m, "provider").as_str() {
        "pem" => "Private key",
        _ if text(m, "label") == "webhook" => "Webhook URL",
        "aws" | "google" | "azure" => "Cloud key",
        "anthropic" | "openai" | "twilio" | "sendgrid" | "stripe" | "openrouter"
        | "huggingface" | "groq" | "perplexity" | "replicate" | "xai" | "langsmith"
        | "pinecone" | "linear" => "API key",
        "github" | "slack" | "npm" | "semlith" | "gitlab" | "vault" | "digitalocean"
        | "databricks" | "pypi" | "shopify" | "doppler" | "discord" => "Access token",
        "jwt" | "header" => "Token",
        "connection" => "Connection string",
        _ if text(m, "kind").contains("password") => "Password",
        _ => "Secret",
    };
    let evidence = first.map(|m| {
        format!(
            "line {} · {} · {}",
            m.get("line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            text(m, "kind"),
            text(m, "masked")
        )
    });
    let is_dir = std::path::Path::new(path).is_dir();

    let (risk, likely, kind, why, suggest): (u8, String, String, String, &str) = match class {
        crate::store::class::CREDENTIAL => (
            95,
            "credential file".into(),
            "Credential file".into(),
            "A credential file by its name; semlith never indexes one, whatever is in it.".into(),
            "out",
        ),
        crate::store::class::POLICY => (
            0,
            "no secret found".into(),
            if is_dir { "Generated" } else { "Large file" }.into(),
            if is_dir {
                "A generated or vendored folder: indexing it adds chunks nobody asked about."
            } else {
                "Over the size cap; nothing secret was found, it is only large."
            }
            .into(),
            if is_dir { "out" } else { "in" },
        ),
        _ if live.is_empty() => {
            let documented = matches
                .iter()
                .any(|m| text(m, "dummy").contains("documentation"));
            let kind = first.map_or("Secret", kind_of).to_string();
            if matches.is_empty() {
                (
                    0,
                    "no secret found".into(),
                    kind,
                    "Nothing secret-shaped was found in it.".into(),
                    "in",
                )
            } else {
                (
                    5,
                    if documented {
                        "documented example key"
                    } else {
                        "likely a test value"
                    }
                    .into(),
                    kind,
                    format!(
                        "Every match is a declared test dummy: {}.",
                        first.map(|m| text(m, "dummy")).unwrap_or_default()
                    ),
                    "in",
                )
            }
        }
        _ => {
            let top = live
                .iter()
                .filter_map(|m| m.get("confidence").and_then(serde_json::Value::as_u64))
                .max()
                .map(|c| c.min(100) as u8)
                .or(confidence)
                .unwrap_or(50);
            let m = first.expect("a live match");
            let kind = kind_of(m).to_string();
            let noun = if kind == "Password" {
                "password"
            } else {
                "key"
            };
            let likely = if top >= 70 {
                format!("likely a real {noun}")
            } else if top >= 30 {
                format!("may be a real {noun}")
            } else {
                "likely a test value".to_string()
            };
            // The strongest signal each way, in its own words, so a person
            // sees why it is 35 and not 80: what lowered it, then what raised
            // it. Strength is the order the scan weighs them in.
            let signals = m
                .get("signals")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let strongest = |effect: &str, order: &[&str]| {
                order.iter().find_map(|name| {
                    signals.iter().find_map(|s| {
                        (s.get("effect").and_then(serde_json::Value::as_str) == Some(effect)
                            && s.get("name").and_then(serde_json::Value::as_str) == Some(name))
                        .then(|| text(s, "detail"))
                    })
                })
            };
            let reasons: Vec<String> = [
                strongest(
                    "down",
                    &[
                        "dummy",
                        "checksum",
                        "expiry",
                        "location",
                        "format",
                        "randomness",
                    ],
                ),
                strongest(
                    "up",
                    &["checksum", "companion", "format", "randomness", "location"],
                ),
            ]
            .into_iter()
            .flatten()
            .collect();
            let why = format!(
                "Looks like {} at line {}{}: {top} % likely real.",
                text(m, "kind"),
                m.get("line")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                if reasons.is_empty() {
                    String::new()
                } else {
                    format!(": {}", reasons.join("; "))
                }
            );
            let suggest = if top >= 70 {
                "out"
            } else if top >= 30 {
                "redact"
            } else {
                "in"
            };
            (top, likely, kind, why, suggest)
        }
    };
    let band = if risk >= 70 {
        "high"
    } else if risk >= 30 {
        "medium"
    } else {
        "low"
    };
    let tone = match (class, band) {
        (crate::store::class::POLICY, _) => "grey",
        (_, "high") => "red",
        (_, "medium") => "amber",
        _ if risk == 0 => "grey",
        _ => "green",
    };
    serde_json::json!({
        "risk": risk,
        "band": band,
        "likely": likely,
        "tone": tone,
        "kind": kind,
        "why": why,
        "evidence": evidence,
        "suggest": suggest,
    })
}

/// What a pass does with one scanned file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Index this text: the file's own, or with accepted matches redacted.
    Index { text: String, accepted: bool },
    /// Refuse the file, for this reason.
    Refuse(String),
}

/// The one rule every surface applies to a scanned file (2.1, 2.6).
///
/// A file whose every match is a dummy is indexed. A file with a live-looking
/// match is refused, unless a person accepted it and every live match is one
/// they saw — the fingerprints say so — in which case it is indexed in the
/// mode they chose, with redaction applied again to the text as it is now. A
/// new match since the acceptance refuses it again, whatever the acceptance
/// said: an edit never lets a new secret through.
///
/// An acceptance never loosens (0.51.0). A file accepted redacted keeps every
/// value its acceptance holds a fingerprint of redacted, even one later rules
/// call a dummy or no longer find at all: the person chose redaction for what
/// they saw, and a rule change is not their choice.
pub fn decide(
    db: &rusqlite::Connection,
    path: &str,
    text: &str,
    found: &[Match],
) -> anyhow::Result<Decision> {
    let accepted = crate::store::acceptance(db, path)?;
    if accepted.as_ref().is_some_and(|a| a.mode == "refused") {
        return Ok(Decision::Refuse(
            "refused by a person, although every match is a test dummy".to_string(),
        ));
    }
    let live: Vec<&Match> = found.iter().filter(|m| m.dummy.is_none()).collect();
    let acceptance = accepted.filter(|a| a.class == crate::store::class::CONTENT);
    if live.is_empty() && acceptance.as_ref().is_none_or(|a| a.mode != "redacted") {
        return Ok(Decision::Index {
            text: text.to_string(),
            accepted: false,
        });
    }
    let Some(acceptance) = acceptance else {
        let first = live[0];
        return Ok(Decision::Refuse(format!(
            "holds what looks like {} at line {} — semlith does not index it",
            first.kind, first.line
        )));
    };
    // An acceptance is made with the store's salt, so a store with an
    // acceptance and no salt has been tampered with or half-restored; the
    // safe answer is to refuse, never to compare against a made-up salt.
    let Some(salt) = crate::store::salt_if_any(db)? else {
        return Ok(Decision::Refuse(
            "accepted, but this store has no salt to check the acceptance against".to_string(),
        ));
    };
    if let Some(new) = live.iter().find(|m| {
        !acceptance
            .fingerprints
            .contains(&fingerprint(&salt, text, m))
    }) {
        return Ok(Decision::Refuse(format!(
            "new match since accepted, line {}: what looks like {}",
            new.line, new.kind
        )));
    }
    Ok(Decision::Index {
        text: if acceptance.mode == "redacted" {
            redact(text, &held(&salt, text, found, &acceptance.fingerprints))
        } else {
            text.to_string()
        },
        accepted: true,
    })
}

/// Every value in `text` an acceptance holds a fingerprint of: today's
/// matches, dummies among them, and what the 0.50.0 generic rule matched,
/// which a later rule may no longer name.
fn held(salt: &[u8; 32], text: &str, found: &[Match], prints: &[String]) -> Vec<Match> {
    let mut out: Vec<Match> = found
        .iter()
        .filter(|m| prints.contains(&fingerprint(salt, text, m)))
        .cloned()
        .collect();
    for range in legacy_generic(text) {
        if out
            .iter()
            .any(|m| m.range.start < range.end && range.start < m.range.end)
        {
            continue;
        }
        let value = &text[range.clone()];
        if prints.contains(&print(salt, value)) {
            out.push(Match {
                kind: "a secret assigned to a key-like name".to_string(),
                provider: "generic",
                label: "secret",
                line: filter::line_of(text, range.start),
                masked: mask(value, 0),
                dummy: None,
                confidence: 0,
                signals: Vec::new(),
                range,
            });
        }
    }
    out
}

/// The 0.50.0 generic rule, kept only so an acceptance made under it keeps
/// redacting what it matched (see [`decide`]). It decides nothing else.
fn legacy_generic(text: &str) -> Vec<std::ops::Range<usize>> {
    static RULES: std::sync::OnceLock<[regex::Regex; 2]> = std::sync::OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            r#"(?i)[a-z0-9_-]*(?:api[_-]?key|secret|token|password|passwd|auth|[_-]key)[a-z0-9_-]*[ \t]*[:=][ \t]*["']([^"'\r\n]{20,})["']"#,
            r#"(?im)^[ \t]*(?:export[ \t]+)?[a-z0-9_.-]*(?:api[_-]?key|secret|token|password|passwd|auth|[_-]key)[a-z0-9_.-]*[ \t]*[:=][ \t]*([A-Za-z0-9+/=_.-]{20,})[ \t]*\r?$"#,
        ]
        .map(|r| regex::Regex::new(r).expect("the 0.50.0 rules compile"))
    });
    rules
        .iter()
        .flat_map(|rule| rule.captures_iter(text).filter_map(|c| c.get(1)))
        .filter(|v| {
            !filter::is_placeholder(v.as_str())
                && filter::entropy(v.as_str()) >= filter::MIN_ENTROPY
        })
        .map(|v| v.range())
        .collect()
}

/// `text` as it may be served from disk: redacted if the file was accepted
/// with redaction, whole if it is clean or accepted as-is, and `None` if it
/// holds something live that nobody accepted.
pub fn readable(
    db: &rusqlite::Connection,
    path: &str,
    text: &str,
) -> anyhow::Result<Option<String>> {
    Ok(match decide(db, path, text, &scan(path, text))? {
        Decision::Index { text, .. } => Some(text),
        Decision::Refuse(_) => None,
    })
}

/// The verdict on a match of a row of [`SHAPES`], or `None` when a check the
/// pattern cannot make says it is not one after all: an encoded block that is
/// a certificate, a Discord-shaped string whose first part is no user id, a
/// connection string or header whose secret part is a word.
fn judge(path: &str, text: &str, index: usize, range: std::ops::Range<usize>) -> Option<Match> {
    let shape = &SHAPES[index];
    let value = &text[range.clone()];
    let body = value.get(shape.prefix_len.min(value.len())..).unwrap_or("");
    let mut signals = Vec::new();
    let mut score: i32 = 60;

    // What the key is when the value holds it encoded or in pieces.
    let decoded = match shape.label {
        "encoded private key" => {
            let bytes = base64_decode(value, false)?;
            let pem = String::from_utf8_lossy(&bytes).into_owned();
            if !pem
                .lines()
                .next()
                .is_some_and(|l| l.contains("PRIVATE KEY"))
            {
                return None;
            }
            pem
        }
        _ => String::new(),
    };
    if shape.provider == "discord" && shape.label == "bot token" {
        let id = base64_decode(value.split('.').next().unwrap_or(""), true)?;
        if !(17..=20).contains(&id.len()) || !id.iter().all(u8::is_ascii_digit) {
            return None;
        }
    }
    // The secret part of a header or a connection string, judged as the
    // generic rule judges a value. Basic carries `user:password` encoded.
    let mut placeholder = false;
    if matches!(shape.provider, "connection" | "header") {
        let secret = if shape.provider == "header" && scheme_before(text, range.start) == "basic" {
            let pair = String::from_utf8(base64_decode(value, false)?).ok()?;
            pair.split_once(':').map(|(_, p)| p.to_string())?
        } else {
            value.to_string()
        };
        placeholder = shape.provider == "connection" && placeholder_password(&secret);
        if !placeholder && value_shape(&secret, true).is_none() {
            return None;
        }
    }
    let pem = match shape.label {
        "private key" => pem_body(value),
        "encoded private key" => pem_body(&decoded),
        "putty key" => putty_body(value),
        _ => String::new(),
    };

    let dummy = if shape.provider == "pem" {
        (pem.len() < 40).then_some("(c) a private-key header with no key body")
    } else if placeholder {
        Some("(b) a placeholder password, such as ${…}, <…> or password")
    } else {
        dummy_rule(value, body)
            .or_else(|| prefixed_dummy(shape, body))
            .or_else(|| {
                (shape.provider == "jwt")
                    .then(|| jwt_dummy(value))
                    .flatten()
            })
    }
    .or_else(|| allowed(text, range.start));

    // Format: the issuer's documented length and alphabet.
    match format_fits(shape.provider, value, body) {
        Some(true) => {
            score += 15;
            signals.push(signal(
                "format",
                true,
                "the provider's documented length and characters",
            ));
        }
        Some(false) => {
            score -= 25;
            signals.push(signal(
                "format",
                false,
                "off the provider's documented length",
            ));
        }
        None => signals.push(signal(
            "format",
            true,
            "matches the provider's prefix and pattern",
        )),
    }

    // Randomness, against what the generator produces: the key body, the
    // token at the end of a webhook, the parts of a bot token after its id.
    let random_part = if shape.provider == "pem" {
        pem.as_str()
    } else if shape.label == "webhook" {
        value.rsplit('/').next().unwrap_or(body)
    } else if shape.label == "bot token" {
        value.split_once('.').map_or(body, |(_, rest)| rest)
    } else {
        body
    };
    let (random, why) = randomness(random_part);
    score += random;
    if random != 0 {
        signals.push(signal("randomness", random > 0, &why));
    }

    // Companions.
    if shape.provider == "aws" && aws_secret_nearby(text, range.clone()) {
        score += 15;
        signals.push(signal(
            "companion",
            true,
            "a 40-character secret access key within five lines",
        ));
    }
    if shape.provider == "pem" && dummy.is_none() && pem_is_der(&pem) {
        score += 15;
        signals.push(signal(
            "companion",
            true,
            "the body decodes to a DER sequence of plausible length",
        ));
    }

    // Expiry.
    if shape.provider == "jwt"
        && let Some(exp) = jwt_exp(value)
        && exp < now()
    {
        score -= 30;
        signals.push(signal("expiry", false, "its exp claim has passed"));
    }

    score += location(path, text, range.start, &mut signals);

    // Checksum, last: a valid one is close to proof and an invalid one is
    // proof, so it bounds whatever the softer signals added up to. Proof that
    // a token was never issued is a dummy rule of its own (d), not only a low
    // score: a file of such tokens is indexed.
    let mut dummy = dummy;
    if let Some(valid) = checksum(shape.provider, value, body) {
        if valid {
            score = score.max(90);
            signals.push(signal("checksum", true, "its checksum is valid"));
        } else {
            score = score.min(10);
            dummy = dummy.or(Some(
                "(d) fails the issuer's checksum, so it was never issued",
            ));
        }
    }

    if let Some(rule) = dummy {
        score = score.min(10);
        signals.push(signal("dummy", false, rule));
    }

    Some(Match {
        kind: shape.kind.to_string(),
        provider: shape.provider,
        label: shape.label,
        // Set by `scan` once every match is in, in one pass.
        line: 0,
        masked: mask(value, shape.prefix_len),
        dummy,
        confidence: score.clamp(0, 100) as u8,
        signals,
        range,
    })
}

/// What a generic value is, when it is one that counts (0.51.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A stretch a generator wrote, or 32 or more hex.
    Generated,
    /// A short password: 12 or more, three kinds of character, no word.
    Password,
}

/// A secret-sounding name, read as words (0.51.0).
struct Named {
    /// Its secret word is one that only ever names a secret.
    strong: bool,
    /// Its last word says password, so the shorter floor applies.
    password: bool,
}

/// Whether a value is shaped like a credential, and how; `None` when it is
/// one of the things a credential is never written as.
///
/// It counts when it has a stretch a generator wrote, 32 or more hex, or —
/// for a password — passes the short-password test. It never counts as a
/// member path, a URL with no credentials in it, prose, or with a sequential
/// run of eight.
fn value_shape(value: &str, password: bool) -> Option<Shape> {
    let bare = ["Bearer ", "Basic ", "Token "]
        .iter()
        .find_map(|p| value.strip_prefix(p))
        .unwrap_or(value);
    // Prose, and a value elided as documentation elides one (`eyJ…X9oN`).
    if bare.chars().any(char::is_whitespace) || bare.contains("...") || bare.contains('…') {
        return None;
    }
    // Code: an address taken, or a block or call opened (`&schemas.Key{`).
    if bare.starts_with('&') || bare.ends_with(['{', '(', '[']) {
        return None;
    }
    if let Some((_, rest)) = bare.split_once("://") {
        let authority = rest.split('/').next().unwrap_or("");
        if !authority.contains('@') {
            return None;
        }
    }
    let identifier = |s: &str| {
        s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    };
    let parts: Vec<&str> = bare.split('.').collect();
    if parts.len() >= 2 && parts.iter().all(|p| identifier(p) && !generated(p)) {
        return None;
    }
    if bare
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|s| sequential(&s.chars().collect::<Vec<_>>()))
    {
        return None;
    }
    let hex = bare
        .split(|c: char| !c.is_ascii_hexdigit())
        .any(|s| s.len() >= 32);
    if has_generated(bare) || hex {
        return Some(Shape::Generated);
    }
    (password && short_password(bare)).then_some(Shape::Password)
}

/// A password too short for the generic rule's 20, still worth refusing: 12
/// or more characters, at least three of lower case, upper case, digits and
/// symbols, and no segment that is a word.
fn short_password(value: &str) -> bool {
    let kinds = [
        value.chars().any(|c| c.is_ascii_lowercase()),
        value.chars().any(|c| c.is_ascii_uppercase()),
        value.chars().any(|c| c.is_ascii_digit()),
        value
            .chars()
            .any(|c| c.is_ascii_punctuation() || !c.is_ascii()),
    ];
    value.chars().count() >= 12 && kinds.iter().filter(|k| **k).count() >= 3 && !has_word(value)
}

/// Whether a value holds a run of four or more letters written as a word is:
/// all lower case, all upper case, or capitalised. A random password holds one
/// of those now and then (a few in a hundred at 16 characters); a person's
/// holds one nearly always.
fn has_word(value: &str) -> bool {
    value.split(|c: char| !c.is_ascii_alphabetic()).any(|w| {
        let rest = w.get(1..).unwrap_or("");
        w.len() >= 4
            && (w.chars().all(|c| c.is_ascii_lowercase())
                || w.chars().all(|c| c.is_ascii_uppercase())
                || (w.starts_with(|c: char| c.is_ascii_uppercase())
                    && rest.chars().all(|c| c.is_ascii_lowercase())))
    })
}

/// Whether a stretch of `s` between non-alphanumerics reads as a
/// generator's output: [`generated`], and also not a camelCase identifier
/// (`hideDecisionModelsBanner`) and not a word with a number on it
/// (`dbpassword9090`), which mix case or digits only at word edges. A
/// generator switches between lower case, upper case and digits all along.
///
/// ponytail: both are heuristics with a measured ceiling. A uniform random
/// 20-character alphanumeric reads as camelCase about once in 35 000 draws
/// (it needs no digit at all) and switches fewer than twice about once in
/// three million; both fall off fast with length. Raise the bar if a real
/// key format ever turns out to be letters only.
fn has_generated(s: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|part| generated(part) && switches(part) >= 2 && !camel(part))
}

/// How many times a stretch changes between lower case, upper case and
/// digits.
fn switches(s: &str) -> usize {
    let kind = |c: char| (c.is_ascii_lowercase(), c.is_ascii_uppercase());
    s.chars()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| kind(w[0]) != kind(w[1]))
        .count()
}

/// Whether a stretch is a camelCase identifier: letters only, in pieces of
/// one optional capital and two or more lower-case letters, four letters a
/// piece on average. A generator's letters break into pieces of about two.
fn camel(s: &str) -> bool {
    if !s.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let mut pieces: Vec<String> = vec![String::new()];
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            pieces.push(String::new());
        }
        pieces.last_mut().expect("a piece is open").push(c);
        prev_lower = c.is_ascii_lowercase();
    }
    pieces.len() * 4 <= s.len()
        && pieces.iter().all(|p| {
            let rest = p
                .strip_prefix(|c: char| c.is_ascii_uppercase())
                .unwrap_or(p);
            rest.len() >= 2 && rest.chars().all(|c| c.is_ascii_lowercase())
        })
}

/// Words that only ever name a secret. `key`, `token` and `auth` also name a
/// great many things that are not, and score lower.
const STRONG_WORDS: [&str; 8] = [
    "secret",
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "apikey",
    "credential",
    "credentials",
];

/// A name holding one of these as a whole word names a credential.
const SECRET_WORDS: [&str; 15] = [
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "apikey",
    "key",
    "auth",
    "authorization",
    "credential",
    "credentials",
    "creds",
    "pat",
    "bearer",
];

/// One word that ends in one of these is a secret word glued to another, as
/// `accesstoken` or `clientsecret` are. Only ever widens what counts.
const GLUED: [&str; 13] = [
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "passphrase",
    "apikey",
    "secretkey",
    "accesskey",
    "privatekey",
    "masterkey",
    "signingkey",
    "encryptionkey",
];

/// Last words that say a name holds something about a secret, not the secret:
/// `token_url`, `secret_manager_client`, `auth_type`. `id` is not one, so
/// Vault's `secret_id` stays a credential.
const METADATA: &[&str] = &[
    "url",
    "uri",
    "urls",
    "type",
    "types",
    "name",
    "names",
    "count",
    "limit",
    "endpoint",
    "endpoints",
    "header",
    "headers",
    "prefix",
    "suffix",
    "ttl",
    "expiry",
    "expires",
    "expiration",
    "manager",
    "policy",
    "length",
    "len",
    "field",
    "fields",
    "path",
    "paths",
    "file",
    "files",
    "filename",
    "filepath",
    "dir",
    "ref",
    "refs",
    "arn",
    "label",
    "format",
    "mode",
    "method",
    "kind",
    "scope",
    "scopes",
    "version",
    "usage",
    "provider",
    "providers",
    "flow",
    "base",
    "host",
    "hostname",
    "port",
    "client",
    "size",
    "max",
    "min",
    "timeout",
    "lifetime",
    "duration",
    "enabled",
    "disabled",
    "required",
    "regex",
    "pattern",
    "template",
    "description",
    "desc",
    "hint",
    "location",
    "algorithm",
    "alg",
    "class",
    "handler",
    "service",
    "status",
    "state",
    "error",
    "errors",
    "message",
    "msg",
    "title",
    "link",
    "domain",
    "region",
    "env",
    "var",
    "variable",
    "user",
    "username",
    "email",
    "issuer",
    "audience",
    "at",
    "time",
    "timestamp",
    "date",
    "fingerprint",
    "kid",
    "param",
    "params",
    "schema",
    "spec",
    "list",
    "map",
    "store",
    "table",
    "column",
    "rotation",
    "interval",
    "period",
    "retries",
    "delay",
    "placeholder",
    "validator",
    "callback",
    "route",
    "source",
    "option",
    "options",
    "display",
    "hash",
    "hashed",
    "digest",
];

/// Words that, in front of a last word `key`, make it a lookup key and not a
/// credential: `cache_key`, `object_key`, `sort_key`. `storage` is not one:
/// `AZURE_STORAGE_KEY` is an Azure account key.
const LOOKUP: &[&str] = &[
    "cache",
    "object",
    "sort",
    "primary",
    "foreign",
    "partition",
    "map",
    "lookup",
    "row",
    "idempotency",
    "routing",
    "hash",
    "bucket",
    "state",
    "cursor",
    "result",
    "reference",
    "index",
    "query",
    "event",
    "i18n",
    "translation",
    "dict",
    "item",
    "entry",
    "group",
    "composite",
    "unique",
    "dedup",
    "dedupe",
    "lock",
    "redis",
    "metric",
    "tag",
    "column",
    "field",
    "attribute",
    "attr",
    "property",
    "prop",
    "public",
    "pub",
    "file",
    "asset",
    "order",
    "shard",
    "range",
    "natural",
    "surrogate",
    "react",
    "list",
    "record",
    "node",
    "label",
    "message",
    "feature",
    "flag",
    "memo",
    "parent",
    "child",
    "tree",
    "span",
    "trace",
];

/// Value words a name may end on that say how the value is written, not what
/// it is: `password_b64` is a password.
const ENCODINGS: [&str; 8] = [
    "value", "str", "string", "b64", "base64", "hex", "raw", "plain",
];

/// Whether a name is a credential's name, read as words (0.51.0).
///
/// Split on `_`, `-`, `.` and camelCase, digits trimmed from word ends, the
/// last dotted component only, encoding words dropped from the end. It is a
/// credential name when a secret word is one of its words and its last word
/// is not metadata, and not when its last word is `key` after a lookup word.
/// Deny lists only: an allow list of what may come before `key` dropped a
/// live SMTP key in the audit that led here.
fn credential_name(name: &str) -> Option<Named> {
    let last = name.rsplit('.').next().unwrap_or(name);
    let mut words = words(last);
    while words.len() > 1
        && words
            .last()
            .is_some_and(|w| ENCODINGS.contains(&w.as_str()))
    {
        words.pop();
    }
    let final_word = words.last()?.as_str();
    if METADATA.contains(&final_word) {
        return None;
    }
    if final_word == "key" && words.len() >= 2 && LOOKUP.contains(&words[words.len() - 2].as_str())
    {
        return None;
    }
    let secret = |w: &str| {
        SECRET_WORDS.contains(&w) || GLUED.iter().any(|g| w.len() > g.len() && w.ends_with(g))
    };
    if !words.iter().any(|w| secret(w)) {
        return None;
    }
    let strong = words.iter().any(|w| {
        STRONG_WORDS.contains(&w.as_str())
            || GLUED.iter().any(|g| w.len() > g.len() && w.ends_with(g))
    });
    let password = ["password", "passwd", "pwd", "passphrase"]
        .iter()
        .any(|p| final_word.ends_with(p));
    Some(Named { strong, password })
}

/// A name's words, lower case: split on `_`, `-` and camelCase, digits
/// trimmed from each word's ends.
fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out = Vec::new();
    let mut word = String::new();
    for (i, c) in chars.iter().enumerate() {
        if !c.is_ascii_alphanumeric() {
            out.push(std::mem::take(&mut word));
            continue;
        }
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1);
        let boundary = c.is_ascii_uppercase()
            && prev.is_some_and(|p| {
                p.is_ascii_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_ascii_uppercase() && next.is_some_and(char::is_ascii_lowercase))
            });
        if boundary {
            out.push(std::mem::take(&mut word));
        }
        word.push(c.to_ascii_lowercase());
    }
    out.push(word);
    out.into_iter()
        .map(|w| w.trim_matches(|c: char| c.is_ascii_digit()).to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Whether the unquoted form of the generic rule applies to a file: not in
/// code, where `a.b.token = c.d.e` is an assignment of code. Shell,
/// Dockerfiles, Makefiles and Terraform are configuration for this purpose,
/// and a path in no language (`.env`, a `.txt` note) gets every rule.
fn unquoted_applies(path: &str) -> bool {
    !filter::is_code(path)
        || filter::language_of_path(path)
            .is_some_and(|l| matches!(l.name, "shell" | "dockerfile" | "makefile" | "terraform"))
}

/// The verdict on a value assigned to a secret-sounding name (2.2, 0.51.0).
///
/// Scored from its evidence, so a fixture and a live key land apart: a
/// strong secret word and a generated value raise it, a configuration file
/// raises it, a test file lowers it.
fn judge_assignment(
    path: &str,
    text: &str,
    range: std::ops::Range<usize>,
    named: &Named,
    shape: Shape,
) -> Match {
    let value = &text[range.clone()];
    let mut score: i32 = 40 + if named.strong { 10 } else { 5 };
    let mut signals = vec![signal(
        "format",
        true,
        match shape {
            Shape::Generated => {
                score += 10;
                "a secret-sounding name holding a generated value"
            }
            Shape::Password => {
                score += 5;
                "a password name holding 12 or more mixed characters"
            }
        },
    )];
    let (random, why) = randomness(value);
    score += random;
    if random != 0 {
        signals.push(signal("randomness", random > 0, &why));
    }
    score += location(path, text, range.start, &mut signals);
    let dummy = dummy_rule(value, value)
        .or_else(|| test_word(value))
        .or_else(|| allowed(text, range.start));
    if let Some(rule) = dummy {
        score = score.min(10);
        signals.push(signal("dummy", false, rule));
    }
    Match {
        kind: "a secret assigned to a key-like name".to_string(),
        provider: "generic",
        label: "secret",
        line: 0,
        masked: mask(value, 0),
        dummy,
        confidence: score.clamp(0, 100) as u8,
        signals,
        range,
    }
}

/// Rule (b) for a generic value: a whole segment that is a test word.
/// Inside a generated stretch it is not a segment, so it does not count.
fn test_word(value: &str) -> Option<&'static str> {
    value
        .split(|c: char| matches!(c, '-' | '_' | '.') || c.is_whitespace())
        .any(|s| TEST_WORDS.iter().any(|w| s.eq_ignore_ascii_case(w)))
        .then_some("(b) a test word such as fake or example as a whole part of the value")
}

/// Rule (e): the match's own line carries an allow tag. That line only,
/// never a neighbour, so a tag cannot reach a key it was not written beside.
fn allowed(text: &str, at: usize) -> Option<&'static str> {
    let from = window_start(text, at, CONTEXT_BYTES);
    let start = text[from..at].rfind('\n').map_or(from, |i| from + i + 1);
    let mut to = (at + CONTEXT_BYTES).min(text.len());
    while !text.is_char_boundary(to) {
        to -= 1;
    }
    let end = text[at..to].find('\n').map_or(to, |i| at + i);
    let line = &text[start..end];
    ALLOW_TAGS
        .iter()
        .any(|t| line.contains(t))
        .then_some("(e) marked allowed on its own line")
}

/// The scheme word in front of a header value: `bearer`, `basic` or `token`.
fn scheme_before(text: &str, at: usize) -> String {
    let from = window_start(text, at, 64);
    text[from..at]
        .split_whitespace()
        .last()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_ascii_alphabetic())
        .to_ascii_lowercase()
}

/// A connection string's password that stands in for one.
fn placeholder_password(password: &str) -> bool {
    let lower = password.to_ascii_lowercase();
    filter::is_placeholder(password)
        || password.starts_with('$')
        || matches!(
            lower.as_str(),
            "password" | "pass" | "passwd" | "pwd" | "secret" | "changeme"
        )
        || password.chars().all(|c| c == '*')
        || lower.chars().all(|c| c == 'x')
}

/// Rule (b) widened for prefixed shapes (0.51.0): the body is plainly not a
/// generator's.
fn prefixed_dummy(shape: &filter::Shape, body: &str) -> Option<&'static str> {
    let runs = body.split(|c: char| !c.is_ascii_alphanumeric()).any(|s| {
        let chars: Vec<char> = s.chars().collect();
        let longest = longest_run(&chars);
        if chars.iter().all(char::is_ascii_digit) {
            longest >= 10
        } else {
            longest >= 8
        }
    });
    if runs {
        return Some("(b) a sequential run such as abcdefgh in the body");
    }
    if repeated_blocks(body) || repeated_unit(body) {
        return Some("(b) a repeating pattern, such as aaaaBBBBcccc or TESTTESTTEST");
    }
    // A body of words after the issuer's prefix, on a shape whose generator
    // mixes case: no stretch a generator wrote, at least one word, and every
    // part a word, a number, a word with a number on it (`oat01`) or three
    // characters or fewer. AWS, Twilio and semlith bodies are one case by
    // format, and a random one spells only letters now and then; a shape with
    // no prefix has no body to read this way.
    let mixed = shape.pattern.contains("A-Z") && shape.pattern.contains("a-z");
    let parts: Vec<&str> = body
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();
    let plain = |p: &&str| {
        let letters: String = p.chars().filter(char::is_ascii_alphabetic).collect();
        let rest = letters.get(1..).unwrap_or("");
        p.len() <= 3
            || (switches_between_letters_and_digits(p) <= 1
                && (letters == letters.to_ascii_lowercase()
                    || letters == letters.to_ascii_uppercase()
                    || rest == rest.to_ascii_lowercase()))
    };
    if mixed
        && shape.prefix_len > 0
        && !has_generated(body)
        && parts.iter().any(|p| has_word(p))
        && parts.iter().all(plain)
    {
        return Some("(b) a body of words, not a generated value");
    }
    None
}

/// How many times a part changes between letters and digits.
fn switches_between_letters_and_digits(s: &str) -> usize {
    s.chars()
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| w[0].is_ascii_digit() != w[1].is_ascii_digit())
        .count()
}

/// A unit of one to eight characters written three times or more, twelve or
/// more characters in all: `hunter2hunter2hunter2`, `TESTTESTTEST`. A
/// generator writes one by chance about never: at least eight characters
/// must each repeat the one a unit before, under one in four billion even
/// for a hex body.
fn repeated_unit(body: &str) -> bool {
    let b = body.as_bytes();
    (1..=8).any(|unit| {
        let need = (12 - unit).max(2 * unit);
        let mut run = 0;
        (unit..b.len()).any(|i| {
            if b[i] == b[i - unit] {
                run += 1;
            } else {
                run = 0;
            }
            run >= need
        })
    })
}

/// Three blocks in a row of four or more of one character each.
fn repeated_blocks(body: &str) -> bool {
    let mut blocks: Vec<usize> = Vec::new();
    let mut last: Option<char> = None;
    for c in body.chars() {
        if Some(c) == last {
            *blocks.last_mut().expect("a block is open") += 1;
        } else {
            blocks.push(1);
            last = Some(c);
        }
    }
    blocks.windows(3).any(|w| w.iter().all(|n| *n >= 4))
}

/// Rule (a) for a JWT: unsigned (`alg` none), which no server should accept,
/// or one of Supabase's published local-development keys.
fn jwt_dummy(value: &str) -> Option<&'static str> {
    // No signature at all is an unsigned token. HS256, the shortest
    // signature in use, is 43 characters; one shorter was never made by a
    // signer (`some_signature`, base64 of a word).
    match value.rsplit('.').next().map(str::len) {
        Some(0) => return Some("(a) an unsigned token, which no server should accept"),
        Some(n) if n < 40 => {
            return Some("(d) a signature too short for any signing algorithm, so never issued");
        }
        _ => {}
    }
    let mut parts = value.split('.');
    let header = parts
        .next()
        .and_then(|h| base64_decode(h, true))
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())?;
    if header
        .get("alg")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|a| a.eq_ignore_ascii_case("none"))
    {
        return Some("(a) an unsigned token (alg none), which no server should accept");
    }
    let claims = parts
        .next()
        .and_then(|p| base64_decode(p, true))
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())?;
    (claims.get("iss").and_then(serde_json::Value::as_str) == Some("supabase-demo"))
        .then_some("(a) a published example: Supabase's local development key")
}

/// Which declared dummy rule a value meets, if any (2.1).
///
/// (a) a published documentation example, anywhere in the value; (b) a body
/// that carries a marker word, a run of eight or more `X`, or one character
/// repeated. Markers are read in the body only — after the issuer's prefix —
/// and never in the text around it: a comment saying "test" beside a live key
/// changes nothing.
pub fn dummy_rule(value: &str, body: &str) -> Option<&'static str> {
    if DOC_EXAMPLES.iter().any(|e| value.contains(e)) {
        return Some("(a) a published documentation example");
    }
    if MARKERS.iter().any(|m| has_marker(body, m)) {
        return Some("(b) a marker such as EXAMPLE or FAKE in the body");
    }
    let upper = body.to_ascii_uppercase();
    if upper.contains("XXXXXXXX") {
        return Some("(b) a run of eight or more X");
    }
    let significant: Vec<char> = body.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if !significant.is_empty() && significant.iter().all(|c| *c == significant[0]) {
        return Some("(b) one character repeated");
    }
    None
}

/// Whether `body` carries `marker` as someone wrote it, not as a generator
/// happened to spell it.
///
/// Written in one case, `FAKE` or `fake`: a random mixed-case body spells
/// `FAKe` about once in 20 000 Slack tokens, and a live key read as a dummy is
/// indexed. A short marker also has to start or end the body or one of its
/// `-`, `_` or `.` segments, because four letters in one case still turn up by
/// chance; six or more (`SAMPLE`, `EXAMPLE`, `REDACTED`) do not, and sit
/// anywhere, as in `github_pat_11EXAMPLE0…`.
///
/// Even at a segment's edge a short marker counts only when the rest of the
/// segment is not generator-shaped: a live Slack secret opens on `FAKE` about
/// once in 3.7 million, and read as a dummy it was indexed. `FAKEtokenvalue`,
/// `FAKE000000000000` and `fake` alone still are markers.
fn has_marker(body: &str, marker: &str) -> bool {
    let lower = marker.to_ascii_lowercase();
    let segments = || body.split(['-', '_', '.']);
    [marker, lower.as_str()].iter().any(|m| {
        if marker.len() >= 6 {
            body.contains(m)
        } else {
            segments().any(|seg| {
                seg.strip_prefix(m)
                    .or_else(|| seg.strip_suffix(m))
                    .is_some_and(|rest| !generated(rest))
            })
        }
    })
}

/// Whether a stretch reads as a generator's output: twelve or more
/// alphanumerics mixing upper and lower case, or letters and digits.
fn generated(s: &str) -> bool {
    let has = |f: fn(&char) -> bool| s.chars().any(|c| f(&c));
    s.len() >= 12
        && s.chars().all(|c| c.is_ascii_alphanumeric())
        && ((has(char::is_ascii_uppercase) && has(char::is_ascii_lowercase))
            || (has(char::is_ascii_alphabetic) && has(char::is_ascii_digit)))
}

/// Whether the value has the length its issuer documents, when that is known.
fn format_fits(provider: &str, value: &str, body: &str) -> Option<bool> {
    let alnum = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric());
    Some(match provider {
        "aws" => body.len() == 16,
        "github" if value.starts_with("github_pat_") => body.len() >= 82,
        "github" | "npm" => body.len() == 36 && alnum(body),
        "google" => body.len() == 35,
        "twilio" => body.len() == 32,
        "anthropic" => value.len() >= 90,
        "openai" => value.len() >= 40,
        "sendgrid" => {
            let parts: Vec<&str> = value.split('.').collect();
            parts.len() == 3 && parts[1].len() == 22 && parts[2].len() == 43
        }
        "slack" if value.starts_with("https://") => true,
        "slack" => value.split('-').count() >= 4,
        "stripe" => body.len() >= 24 && alnum(body),
        "semlith" => body.len() == 64,
        "jwt" => jwt_header_parses(value),
        // 0.51.0's rows: each pattern fixes the length its source documents,
        // so a match fits by construction, except where the length is open.
        "gitlab" if value.starts_with("gloas-") => body.len() == 64,
        "gitlab" if value.starts_with("glptt-") => body.len() == 40,
        "gitlab" => body.len() >= 20,
        "vault" if value.starts_with("hvs.") => body.len() >= 90,
        "vault" if value.starts_with("hvb.") => body.len() >= 138,
        "pypi" => body.len() >= 50,
        "azure" => (37..=40).contains(&value.len()),
        "openrouter" | "huggingface" | "vault" | "groq" | "perplexity" | "replicate" | "xai"
        | "langsmith" | "pinecone" | "digitalocean" | "databricks" | "shopify" | "linear"
        | "doppler" | "discord" | "connection" | "header" => true,
        _ => return None,
    })
}

/// A score adjustment for how random a body is, and the words for it.
fn randomness(body: &str) -> (i32, String) {
    let chars: Vec<char> = body.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if chars.len() < 8 {
        return (0, String::new());
    }
    // A run is read inside one segment: across a separator, Slack's
    // `…3312-3456786…` spelled `12345678` by chance and a live token scored
    // under the floor (CI run 39255714448 on Windows).
    let in_a_segment = body
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|segment| sequential(&segment.chars().collect::<Vec<_>>()));
    if in_a_segment {
        return (
            -20,
            "a sequential run such as abcdefgh or 12345678".to_string(),
        );
    }
    let bits = filter::entropy(&chars.iter().collect::<String>());
    // What a uniformly random body of this length over its own alphabet
    // could reach at most.
    let distinct = {
        let mut seen: Vec<char> = chars.clone();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    };
    let alphabet = if chars.iter().all(|c| c.is_ascii_hexdigit()) {
        16.0f64
    } else if distinct > 36
        || chars.iter().any(|c| c.is_ascii_lowercase())
            && chars.iter().any(|c| c.is_ascii_uppercase())
    {
        62.0
    } else {
        36.0
    };
    let ceiling = alphabet.log2().min((chars.len() as f64).log2());
    let ratio = bits / ceiling;
    if ratio >= 0.8 {
        (
            10,
            format!("{bits:.1} bits a character, near what a generator produces"),
        )
    } else if ratio < 0.6 {
        (
            -20,
            format!("{bits:.1} bits a character, far below a generated key"),
        )
    } else {
        (0, String::new())
    }
}

/// Eight or more characters rising by one, or falling by one.
fn sequential(chars: &[char]) -> bool {
    longest_run(chars) >= 8
}

/// The longest run of characters rising by one, or falling by one.
///
/// One direction per run: `2123212` steps by one each time but is not
/// sequential, and counting it as a run marked random Slack tokens, whose
/// numeric segments zigzag like that by chance, as typed by hand.
fn longest_run(chars: &[char]) -> usize {
    let mut run = 1;
    let mut longest = chars.len().min(1);
    let mut direction = 0;
    for pair in chars.windows(2) {
        let step = pair[1] as i32 - pair[0] as i32;
        if (step == 1 || step == -1) && (run == 1 || step == direction) {
            direction = step;
            run += 1;
        } else if step == 1 || step == -1 {
            direction = step;
            run = 2;
        } else {
            run = 1;
        }
        longest = longest.max(run);
    }
    longest
}

/// The location signal: where a secret sits says something about whether it
/// is a real one. Returns the adjustment and pushes the reason.
fn location(path: &str, text: &str, at: usize, signals: &mut Vec<Signal>) -> i32 {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    let segments: Vec<&str> = lower.split('/').collect();
    let quiet = [
        "tests",
        "test",
        "fixtures",
        "examples",
        "example",
        "docs",
        "__tests__",
        "spec",
    ];
    if segments.iter().any(|s| quiet.contains(s)) || lower.ends_with(".md") {
        signals.push(signal(
            "location",
            false,
            "under tests, fixtures, examples or docs, or in Markdown",
        ));
        return -20;
    }
    if in_test_function(text, at) {
        signals.push(signal("location", false, "inside a test function"));
        return -20;
    }
    let name = segments.last().copied().unwrap_or("");
    let config = [
        ".env",
        ".yml",
        ".yaml",
        ".toml",
        ".ini",
        ".cfg",
        ".conf",
        ".json",
        ".properties",
    ];
    if name.starts_with(".env")
        || config.iter().any(|e| name.ends_with(e))
        || segments
            .iter()
            .any(|s| matches!(*s, ".github" | "deploy" | "k8s" | "helm" | "terraform"))
        || name == "dockerfile"
        || name.starts_with("docker-compose")
    {
        signals.push(signal(
            "location",
            true,
            "in a configuration, CI or deploy file",
        ));
        return 10;
    }
    0
}

/// Whether the forty lines above `at` open a test function.
fn in_test_function(text: &str, at: usize) -> bool {
    let before = &text[window_start(text, at, CONTEXT_BYTES)..at];
    let window: Vec<&str> = before.lines().rev().take(40).collect();
    window.iter().any(|l| {
        let t = l.trim_start();
        t.starts_with("#[test]")
            || t.starts_with("fn test_")
            || t.starts_with("def test_")
            || t.starts_with("func Test")
            || t.starts_with("it(")
            || t.starts_with("test(")
    })
}

/// Whether a 40-character AWS secret access key sits within five lines.
///
/// Read from a window around the match, not the whole file.
/// ponytail: the window is 16 KiB each side, so a secret more than that away
/// on very long lines is not seen; widen it if that ever matters.
fn aws_secret_nearby(text: &str, range: std::ops::Range<usize>) -> bool {
    let from = window_start(text, range.start, CONTEXT_BYTES);
    let mut to = (range.start + CONTEXT_BYTES).min(text.len());
    while !text.is_char_boundary(to) {
        to -= 1;
    }
    let window = &text[from..to];
    let line = filter::line_of(window, range.start - from) as usize;
    window
        .lines()
        .enumerate()
        .filter(|(i, _)| i + 1 + 5 >= line && *i < line + 5)
        .any(|(_, l)| {
            l.split(|c: char| !(c.is_ascii_alphanumeric() || c == '/' || c == '+'))
                .any(|w| {
                    w.len() == 40
                        && w.chars().any(|c| c.is_ascii_digit())
                        && w.chars().any(|c| c.is_ascii_lowercase())
                })
        })
}

/// The whole PEM block from its BEGIN line: through the END line, or to the
/// end of the base64 that follows when there is no END line.
fn pem_block(text: &str, header: std::ops::Range<usize>) -> std::ops::Range<usize> {
    // Up to the next BEGIN at most: a file of headers with no END lines
    // searched the whole rest of the file once per header (audit S18).
    let all = &text[header.end..];
    let rest = &all[..all.find("-----BEGIN").unwrap_or(all.len())];
    if let Some(end) = rest.find("-----END") {
        let tail = &rest[end..];
        let close = tail[8..]
            .find("-----")
            .map(|i| i + 8 + 5)
            .unwrap_or(tail.len());
        return header.start..header.end + end + close;
    }
    // A body written inside a string runs on through its escapes and the
    // quotes that join one string to the next.
    let body: usize = rest
        .char_indices()
        .take_while(|(_, c)| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '+' | '/' | '=' | '\n' | '\r' | ' ' | '\t' | '\\' | '"' | '\''
                )
        })
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    header.start..header.end + body
}

/// The base64 characters of a PEM block's body: after the header's closing
/// dashes, up to the END line, read through what a string does to it (0.51.0).
///
/// A key in a JSON file, or in a TypeScript, Go, Python or Rust string, has
/// its line breaks escaped (`\n`, `\r\n`, a JSON string inside a JSON string's
/// `\\n`) and may be cut into joined strings. Until 0.51.0 the body was read
/// only after a real line break, so such a key had none, was taken for a bare
/// header under rule (c), and was indexed: every Google service-account file.
fn pem_body(block: &str) -> String {
    let start = block
        .strip_prefix("-----BEGIN")
        .and_then(|rest| rest.find("-----"))
        .map_or(0, |i| "-----BEGIN".len() + i + 5);
    let body = &block[start.min(block.len())..];
    base64_chars(body.split("-----END").next().unwrap_or(""))
}

/// The base64 characters of a key body, read through string escapes.
fn base64_chars(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // An escape: the backslashes and the letter they escape.
            while chars.peek() == Some(&'\\') {
                chars.next();
            }
            if matches!(chars.peek(), Some('n' | 'r' | 't')) {
                chars.next();
            }
        } else if c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=') {
            out.push(c);
        }
    }
    out
}

/// The base64 of a PuTTY key's private lines.
fn putty_body(block: &str) -> String {
    let Some((_, rest)) = block.split_once("Private-Lines:") else {
        return String::new();
    };
    let rest = rest.trim_start_matches(|c: char| c == ' ' || c.is_ascii_digit());
    base64_chars(rest.split("Private-MAC").next().unwrap_or(""))
}

/// Whether a PEM body decodes to a DER `SEQUENCE` whose length matches.
fn pem_is_der(body: &str) -> bool {
    let Some(bytes) = base64_decode(body, false) else {
        return false;
    };
    if bytes.len() < 64 || bytes[0] != 0x30 {
        return false;
    }
    let (len, header) = match bytes[1] {
        n if n < 0x80 => (n as usize, 2),
        0x81 => (bytes[2] as usize, 3),
        0x82 => (((bytes[2] as usize) << 8) | bytes[3] as usize, 4),
        _ => return false,
    };
    len + header == bytes.len()
}

/// Whether a JWT's first segment is a JSON object.
fn jwt_header_parses(value: &str) -> bool {
    value
        .split('.')
        .next()
        .and_then(|h| base64_decode(h, true))
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|v| v.is_object())
}

/// A JWT's `exp` claim, when it has one.
fn jwt_exp(value: &str) -> Option<i64> {
    let payload = value.split('.').nth(1)?;
    let bytes = base64_decode(payload, true)?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    json.get("exp")?.as_i64()
}

/// Whether a token's built-in checksum holds, for the formats that have one.
///
/// GitHub's classic tokens and npm's carry a CRC32 of their 30 random
/// characters, base62-encoded into the last six. A token whose checksum does
/// not hold was never issued.
fn checksum(provider: &str, value: &str, body: &str) -> Option<bool> {
    if !matches!(provider, "github" | "npm") || value.starts_with("github_pat_") || body.len() != 36
    {
        return None;
    }
    let (random, check) = body.split_at(30);
    let crc = crc32(random.as_bytes());
    Some(
        [BASE62_UPPER_FIRST, BASE62_LOWER_FIRST]
            .iter()
            .any(|alphabet| base62(crc, alphabet) == check),
    )
}

const BASE62_UPPER_FIRST: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE62_LOWER_FIRST: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// `n` in base 62, zero-padded to six characters.
fn base62(mut n: u32, alphabet: &[u8]) -> String {
    let mut out = [alphabet[0]; 6];
    for slot in out.iter_mut().rev() {
        *slot = alphabet[(n % 62) as usize];
        n /= 62;
    }
    String::from_utf8(out.to_vec()).unwrap_or_default()
}

/// CRC-32 (IEEE), bit by bit: six characters of a token do not need a table.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for b in bytes {
        crc ^= *b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Standard or URL-safe base64, padding optional. `None` for anything else.
fn base64_decode(text: &str, url: bool) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' if !url => 62,
            b'/' if !url => 63,
            b'-' if url => 62,
            b'_' if url => 63,
            _ => return None,
        })
    };
    let clean: Vec<u8> = text.bytes().filter(|b| *b != b'=').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= value(*c)? << (18 - 6 * i);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

/// The prefix and the last four characters. A value too short to show four
/// without showing most of it shows none.
fn mask(value: &str, prefix_len: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    let prefix: String = chars.iter().take(prefix_len.min(8)).collect();
    if value.starts_with("-----BEGIN") {
        let header = value
            .lines()
            .next()
            .unwrap_or("-----BEGIN PRIVATE KEY-----");
        return format!("{header}…");
    }
    if chars.len() < prefix_len + 12 {
        return format!("{prefix}…");
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}…{tail}")
}

fn signal(name: &'static str, up: bool, detail: &str) -> Signal {
    Signal {
        name,
        effect: if up { "up" } else { "down" },
        detail: detail.to_string(),
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A live-shaped value for row `index` of [`SHAPES`], built at run time.
///
/// For tests, and for the calibration set: the source never holds a
/// live-looking literal, so it cannot be refused itself, and GitHub's push
/// protection has nothing to stop. Random bodies of the documented length and
/// alphabet, with a valid checksum where the format has one.
#[doc(hidden)]
pub fn forge(index: usize) -> String {
    // A value holding a run of eight like `12345678` is one this scanner
    // rightly reads as a dummy, and random digits spell one now and then --
    // a forged Slack token did, and failed the calibration test about once in
    // a hundred runs. A forged value is meant to look live, so such a draw is
    // drawn again.
    loop {
        let value = forge_once(index);
        if !sequential(&value.chars().collect::<Vec<_>>()) {
            return value;
        }
    }
}

fn forge_once(index: usize) -> String {
    let shape = &SHAPES[index];
    let alnum = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let upper = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let hex = b"0123456789abcdef";
    let pick = |alphabet: &[u8], n: usize| -> String {
        let mut bytes = vec![0u8; n];
        let _ = getrandom::fill(&mut bytes);
        bytes
            .iter()
            .map(|b| alphabet[*b as usize % alphabet.len()] as char)
            .collect()
    };
    let letters = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let digits = b"0123456789";
    match (shape.provider, shape.label) {
        ("openrouter", _) => return format!("sk-or-v1-{}", pick(hex, 64)),
        ("huggingface", _) => return format!("hf_{}", pick(letters, 34)),
        ("gitlab", _) => return format!("glpat-{}", pick(alnum, 20)),
        ("vault", _) => return format!("hvs.{}", pick(alnum, 95)),
        ("groq", _) => return format!("gsk_{}", pick(alnum, 52)),
        ("perplexity", _) => return format!("pplx-{}", pick(alnum, 48)),
        ("replicate", _) => return format!("r8_{}", pick(alnum, 37)),
        ("xai", _) => return format!("xai-{}", pick(alnum, 80)),
        ("langsmith", _) => return format!("lsv2_pt_{}_{}", pick(hex, 32), pick(hex, 10)),
        ("pinecone", _) => return format!("pcsk_{}_{}", pick(alnum, 5), pick(alnum, 63)),
        ("digitalocean", _) => return format!("dop_v1_{}", pick(hex, 64)),
        ("databricks", _) => return format!("dapi{}", pick(hex, 32)),
        ("pypi", _) => return format!("pypi-AgEIcHlwaS5vcmc{}", pick(alnum, 70)),
        ("shopify", _) => return format!("shpat_{}", pick(hex, 32)),
        ("linear", _) => return format!("lin_api_{}", pick(alnum, 40)),
        ("doppler", _) => return format!("dp.pt.{}", pick(alnum, 43)),
        ("azure", _) => {
            return format!("{}{}Q~{}", pick(alnum, 3), pick(digits, 1), pick(alnum, 34));
        }
        ("slack", "webhook") => {
            return format!(
                "https://hooks.slack.com/services/T{}/B{}/{}",
                pick(upper, 10),
                pick(upper, 10),
                pick(alnum, 24)
            );
        }
        ("discord", "webhook") => {
            return format!(
                "https://discord.com/api/webhooks/1{}/{}",
                pick(digits, 17),
                pick(alnum, 68)
            );
        }
        ("discord", "bot token") => {
            let id = format!("1{}", pick(digits, 17));
            return format!(
                "{}.{}.{}",
                base64_url(id.as_bytes()),
                pick(alnum, 6),
                pick(alnum, 38)
            );
        }
        ("pem", "encoded private key") => {
            let pem = SHAPES
                .iter()
                .position(|s| s.provider == "pem" && s.label == "private key")
                .expect("the private key row");
            return base64_encode(forge_once(pem).as_bytes());
        }
        ("pem", "putty key") => {
            let mut random = vec![0u8; 288];
            let _ = getrandom::fill(&mut random);
            let lines: Vec<String> = base64_encode(&random)
                .as_bytes()
                .chunks(64)
                .map(|c| String::from_utf8_lossy(c).into_owned())
                .collect();
            return format!(
                "PuTTY-User-Key-File-3: ssh-rsa\nEncryption: none\nComment: rsa-key\n\
                 Public-Lines: 1\nAAAAB3NzaC1yc2EAAAADAQABAAAAgQ\nPrivate-Lines: {}\n{}\n\
                 Private-MAC: {}",
                lines.len(),
                lines.join("\n"),
                pick(hex, 64)
            );
        }
        ("connection", _) => {
            return format!("postgres://app:{}@db.internal:5432/app", pick(alnum, 24));
        }
        ("header", _) => return format!("Authorization: Bearer {}", pick(alnum, 40)),
        _ => {}
    }
    match shape.provider {
        "anthropic" => format!("sk-ant-api03-{}", pick(alnum, 93)),
        "openai" => format!("sk-proj-{}", pick(alnum, 48)),
        "aws" => format!("AKIA{}", pick(b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567", 16)),
        "github" if shape.label == "fine-grained token" => {
            format!("github_pat_11{}_{}", pick(alnum, 22), pick(alnum, 59))
        }
        "github" | "npm" => {
            let random = pick(alnum, 30);
            let check = base62(crc32(random.as_bytes()), BASE62_UPPER_FIRST);
            let prefix = if shape.provider == "npm" {
                "npm_"
            } else {
                "ghp_"
            };
            format!("{prefix}{random}{check}")
        }
        "slack" => format!(
            "xoxb-{}-{}-{}",
            pick(b"0123456789", 12),
            pick(b"0123456789", 13),
            pick(alnum, 24)
        ),
        "stripe" => format!("sk_live_{}", pick(alnum, 24)),
        "google" => format!("AIza{}", pick(alnum, 35)),
        "twilio" => format!("SK{}", pick(hex, 32)),
        "sendgrid" => format!("SG.{}.{}", pick(alnum, 22), pick(alnum, 43)),
        "semlith" => format!("sml_{}", pick(hex, 64)),
        "pem" => {
            // A DER SEQUENCE of the right length, then random bytes.
            let mut der = vec![0x30u8, 0x82, 0x04, 0xa0];
            let mut rest = vec![0u8; 0x04a0];
            let _ = getrandom::fill(&mut rest);
            der.extend(rest);
            let b64 = base64_encode(&der);
            let lines: Vec<String> = b64
                .as_bytes()
                .chunks(64)
                .map(|c| String::from_utf8_lossy(c).into_owned())
                .collect();
            format!(
                "-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----",
                lines.join("\n")
            )
        }
        "jwt" => {
            let header = base64_url(br#"{"alg":"HS256","typ":"JWT"}"#);
            let claims = format!(
                r#"{{"sub":"{}","exp":{}}}"#,
                pick(alnum, 12),
                now() + 86_400
            );
            format!(
                "{header}.{}.{}",
                base64_url(claims.as_bytes()),
                pick(alnum, 43)
            )
        }
        _ => pick(alnum, 40),
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_url(bytes: &[u8]) -> String {
    base64_encode(bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(text: &str) -> Vec<serde_json::Value> {
        scan("", text)
            .iter()
            .map(|m| serde_json::to_value(m).unwrap())
            .collect()
    }

    /// The review list's risk is read off the scan's own verdict: a forged
    /// live key is high and red and suggested out, a documented example is
    /// low and green, a credential file is never suggested in, and nothing
    /// in the answer is the value.
    #[test]
    fn a_review_row_is_scored_from_the_scan_and_never_shows_the_value() {
        let live = forge(
            SHAPES
                .iter()
                .position(|s| s.kind == "a GitHub token")
                .unwrap(),
        );
        let text = format!("token = {live}\n");
        let found = stored(&text);
        let a = assess(crate::store::class::CONTENT, "/x/a.rs", &found, None);
        assert_eq!(a["band"], "high", "{a}");
        assert_eq!(a["tone"], "red");
        assert_eq!(a["suggest"], "out");
        assert_eq!(a["kind"], "Access token");
        assert_eq!(a["likely"], "likely a real key");
        assert!(a["risk"].as_u64().unwrap() >= 70);
        assert!(!a.to_string().contains(&live), "the value leaked: {a}");
        assert!(a["evidence"].as_str().unwrap().starts_with("line 1"));

        let example = concat!("AKIAIOSFODNN7", "EXAMPLE");
        let doc = assess(
            crate::store::class::DUMMY,
            "/x/b.rs",
            &stored(&format!("key = {example}\n")),
            None,
        );
        assert_eq!(doc["band"], "low", "{doc}");
        assert_eq!(doc["tone"], "green");
        assert_eq!(doc["likely"], "documented example key");
        assert_eq!(doc["suggest"], "in");

        let cred = assess(crate::store::class::CREDENTIAL, "/x/.npmrc", &[], None);
        assert_eq!(cred["suggest"], "out");
        assert_eq!(cred["band"], "high");

        let big = assess(crate::store::class::POLICY, "/x/huge.log", &[], None);
        assert_eq!(big["likely"], "no secret found");
        assert_eq!(big["tone"], "grey");
        assert_eq!(big["band"], "low");
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn every_row_example_is_a_dummy_and_every_forged_value_is_live() {
        for (i, shape) in SHAPES.iter().enumerate() {
            let example = scan("", shape.example);
            assert!(
                !example.is_empty(),
                "{} example matched nothing",
                shape.kind
            );
            assert!(
                !refuses(&example),
                "{} example is not a dummy: {example:?}",
                shape.kind
            );
            let live = forge(i);
            let found = scan("src/config.rs", &live);
            assert!(
                refuses(&found),
                "{} forged value let through: {found:?}",
                shape.kind
            );
            assert!(found[0].confidence >= 70, "{}: {:?}", shape.kind, found[0]);
            // A mask shows the prefix and four characters, never the body.
            let masked = &found[0].masked;
            assert!(
                masked.starts_with("-----BEGIN") || masked.chars().count() <= 8 + 1 + 4,
                "{} mask shows too much: {masked}",
                shape.kind
            );
        }
    }

    /// The confidence is calibrated on a labelled set built now, so no
    /// live-looking literal is in the source: every live-shaped value scores
    /// at least 70 %, every dummy, placeholder, invalid checksum and header
    /// only case at most 30 %, and every row shows a signal. The spread per
    /// class is printed for the release record (`--nocapture`).
    #[test]
    fn confidence_is_calibrated_on_two_hundred_labelled_cases() {
        let mut live: Vec<(String, String)> = Vec::new();
        let mut not: Vec<(String, String)> = Vec::new();
        for (i, shape) in SHAPES.iter().enumerate() {
            for _ in 0..10 {
                live.push((shape.kind.to_string(), forge(i)));
            }
            not.push((format!("{} example", shape.kind), shape.example.to_string()));
            let value = forge(i);
            if shape.prefix_len > 0 && value.len() > shape.prefix_len + 8 {
                let prefix = &value[..shape.prefix_len];
                let body_len = value.len() - shape.prefix_len;
                for marker in ["FAKE", "EXAMPLE", "DUMMY"] {
                    let body: String = marker.chars().cycle().take(body_len).collect();
                    not.push((
                        format!("{} {marker}", shape.kind),
                        format!("{prefix}{body}"),
                    ));
                }
                not.push((
                    format!("{} X run", shape.kind),
                    format!("{prefix}{}", "X".repeat(body_len)),
                ));
            }
        }
        for provider in ["github", "npm"] {
            let i = SHAPES
                .iter()
                .position(|s| s.provider == provider && s.label == "token")
                .unwrap();
            for _ in 0..10 {
                // One character of the random part, not of the checksum: a
                // changed checksum character is valid in the other base62
                // alphabet about twice in ten thousand (0.50.0, Windows CI).
                let mut chars: Vec<char> = forge(i).chars().collect();
                chars[4] = if chars[4] == 'Z' { 'Y' } else { 'Z' };
                let value: String = chars.into_iter().collect();
                not.push((format!("{provider} invalid checksum"), value));
            }
        }
        for header in ["RSA ", "EC ", "OPENSSH ", "", "ENCRYPTED "] {
            not.push((
                "private key header only".to_string(),
                format!("-----BEGIN {header}PRIVATE KEY-----\n-----END {header}PRIVATE KEY-----"),
            ));
        }
        assert!(
            live.len() + not.len() >= 200,
            "{} cases",
            live.len() + not.len()
        );

        let score = |value: &str| -> (u8, usize) {
            let found = scan("src/config.rs", value);
            let best = found.iter().map(|m| m.confidence).max().unwrap_or(0);
            let signals = found.iter().map(|m| m.signals.len()).min().unwrap_or(1);
            (best, signals)
        };
        let mut spread: std::collections::BTreeMap<String, (u8, u8)> = Default::default();
        for (class, value) in &live {
            let (c, signals) = score(value);
            assert!(c >= 70, "{class}: live-shaped scored {c}");
            assert!(signals >= 1, "{class}: no signal");
            let e = spread.entry(format!("live {class}")).or_insert((100, 0));
            *e = (e.0.min(c), e.1.max(c));
        }
        for (class, value) in &not {
            let (c, signals) = score(value);
            assert!(c <= 30, "{class}: scored {c}");
            assert!(signals >= 1, "{class}: no signal");
            let e = spread.entry(class.clone()).or_insert((100, 0));
            *e = (e.0.min(c), e.1.max(c));
        }
        for (class, (lo, hi)) in &spread {
            println!("{class}: {lo}-{hi} %");
        }
        println!("{} live, {} not live", live.len(), not.len());
    }

    #[test]
    fn a_bad_checksum_cannot_be_a_real_token() {
        let github = SHAPES
            .iter()
            .position(|s| s.kind == "a GitHub token")
            .unwrap();
        let mut chars: Vec<char> = forge(github).chars().collect();
        chars[4] = if chars[4] == 'a' { 'b' } else { 'a' };
        let live: String = chars.into_iter().collect();
        let found = scan("src/config.rs", &live);
        assert!(found[0].confidence <= 10, "{:?}", found[0]);
    }

    #[test]
    fn redaction_keeps_line_numbers() {
        let pem = SHAPES.iter().position(|s| s.provider == "pem").unwrap();
        let text = format!("a\n{}\nb\n", forge(pem));
        let found = scan("", &text);
        let redacted = redact(&text, &found);
        assert_eq!(redacted.lines().count(), text.lines().count());
        assert!(redacted.contains("[REDACTED:pem private key]"));
        assert!(redacted.ends_with("b\n"));
    }

    /// A zigzag is not a run, and a marker a generator spelled by chance is
    /// not a marker: both once turned a live-shaped Slack token into a
    /// dummy-looking one, about one run in fifty of the calibration test.
    #[test]
    fn chance_spellings_are_neither_runs_nor_markers() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(!sequential(&chars("5312123212270893")));
        assert!(sequential(&chars("x12345678x")));
        assert!(sequential(&chars("hgfedcba")));
        assert!(dummy_rule("x", "gZZFFZnsFAKeKZFNY0rfxRAk").is_none());
        assert!(dummy_rule("x", "gZZFFZnsFAKEKZFNY0rfxRAk").is_none());
        assert!(dummy_rule("x", "FAKEFAKEFAKEFAKE").is_some());
        assert!(dummy_rule("x", "11EXAMPLE0aaaaaaaaaaaa_aaaa").is_some());
        assert!(dummy_rule("x", "1234-fake-token").is_some());
        // A random segment that opens or closes on a marker by chance: the
        // forged Slack value of CI run 37407715809 on Windows.
        assert!(dummy_rule("x", "FAKEve8w49OZUI0chMPclUPm").is_none());
        assert!(dummy_rule("x", "ve8w49OZUI0chMPclUPmFAKE").is_none());
        assert!(dummy_rule("x", "dummyQ3vT8kLm2Rx9wYp").is_none());
        // Hand-written ones still are: the rest is a word, one case, or digits.
        assert!(dummy_rule("x", "FAKEtokenvalue").is_some());
        assert!(dummy_rule("x", "FAKE000000000000").is_some());
        assert!(dummy_rule("x", "FAKEcccccccccccc").is_some());
        assert!(dummy_rule("x", "sk_test_DUMMY").is_some());
        // A rising run across a segment separator is chance, not a dummy: the
        // forged Slack value of CI run 39255714448 on Windows scored 55.
        // Built from its parts, as `forge` builds its values: a token-shaped
        // literal in the source is one GitHub's push protection refuses.
        let value = [
            "xoxb",
            "188119213312",
            "3456786401962",
            "75ZcJ4TEuvBXoX62p3LJryGu",
        ]
        .join("-");
        let joined = scan("src/config.rs", &value);
        assert!(
            joined.iter().map(|m| m.confidence).max().unwrap_or(0) >= 70,
            "{joined:?}"
        );
        let slack = crate::filter::SHAPES
            .iter()
            .position(|s| s.provider == "slack")
            .unwrap();
        for _ in 0..5000 {
            let value = forge(slack);
            let best = scan("src/config.rs", &value)
                .iter()
                .map(|m| m.confidence)
                .max()
                .unwrap_or(0);
            assert!(best >= 70, "{value} scored {best}");
        }
    }
}
