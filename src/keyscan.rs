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
const DOC_EXAMPLES: [&str; 2] = [
    concat!("AKIAIOSFODNN7", "EXAMPLE"),
    concat!("wJalrXUtnFEMI/K7MDENG/", "bPxRfiCYEXAMPLEKEY"),
];

/// Words that mark a value as standing in for a key rather than being one.
const MARKERS: [&str; 5] = ["EXAMPLE", "FAKE", "DUMMY", "PLACEHOLDER", "REDACTED"];

/// Every secret-shaped match in `text`, in the order they appear.
///
/// `path` is used only for the location signal, and may be empty.
pub fn scan(path: &str, text: &str) -> Vec<Match> {
    let (each, assignment, env) = filter::shapes();
    let mut found: Vec<Match> = Vec::new();
    let overlaps = |found: &[Match], at: &std::ops::Range<usize>| {
        found
            .iter()
            .any(|m| m.range.start < at.end && at.start < m.range.end)
    };
    for (index, regex) in each.iter().enumerate() {
        for m in regex.find_iter(text) {
            let mut range = m.range();
            // Table order decides a match two rows both claim: `sk-ant-` is an
            // `sk-` too, and the Anthropic row comes first.
            if overlaps(&found, &range) {
                continue;
            }
            let shape = &SHAPES[index];
            if shape.provider == "pem" {
                range = pem_block(text, range);
            }
            found.push(judge(path, text, index, range));
        }
    }
    for rule in [assignment, env] {
        for caps in rule.captures_iter(text) {
            let Some(value) = caps.get(1) else { continue };
            let raw = value.as_str();
            if filter::is_placeholder(raw) || filter::entropy(raw) < filter::MIN_ENTROPY {
                continue;
            }
            let range = value.range();
            if overlaps(&found, &range) {
                continue;
            }
            found.push(judge_assignment(path, text, range));
        }
    }
    found.sort_by_key(|m| m.range.start);
    found
}

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
    let value = &text[m.range.clone()];
    blake3::keyed_hash(salt, value.as_bytes()).to_hex()[..32].to_string()
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
    let Some(first) = live.first() else {
        return Ok(Decision::Index {
            text: text.to_string(),
            accepted: false,
        });
    };
    let Some(acceptance) = accepted.filter(|a| a.class == crate::store::class::CONTENT) else {
        return Ok(Decision::Refuse(format!(
            "holds what looks like {} at line {} — semlith does not index it",
            first.kind, first.line
        )));
    };
    let salt = crate::store::salt_if_any(db)?.unwrap_or([0; 32]);
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
    let owned: Vec<Match> = live.into_iter().cloned().collect();
    Ok(Decision::Index {
        text: if acceptance.mode == "redacted" {
            redact(text, &owned)
        } else {
            text.to_string()
        },
        accepted: true,
    })
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

/// The verdict on a prefixed match.
fn judge(path: &str, text: &str, index: usize, range: std::ops::Range<usize>) -> Match {
    let shape = &SHAPES[index];
    let value = &text[range.clone()];
    let body = value.get(shape.prefix_len.min(value.len())..).unwrap_or("");
    let mut signals = Vec::new();
    let mut score: i32 = 60;

    let dummy = if shape.provider == "pem" {
        let key_body = pem_body(value);
        (key_body.len() < 40).then_some("(c) a private-key header with no key body")
    } else {
        dummy_rule(value, body)
    };

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

    // Randomness, against what the generator produces.
    let pem = if shape.provider == "pem" {
        pem_body(value)
    } else {
        String::new()
    };
    let (random, why) = randomness(if shape.provider == "pem" { &pem } else { body });
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
    if shape.provider == "pem" && dummy.is_none() && pem_is_der(&pem_body(value)) {
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
    // proof, so it bounds whatever the softer signals added up to.
    if let Some(valid) = checksum(shape.provider, value, body) {
        if valid {
            score = score.max(90);
            signals.push(signal("checksum", true, "its CRC32 checksum is valid"));
        } else {
            score = score.min(10);
            signals.push(signal(
                "checksum",
                false,
                "its CRC32 checksum is invalid, so it cannot be a real token",
            ));
        }
    }

    if let Some(rule) = dummy {
        score = score.min(10);
        signals.push(signal("dummy", false, rule));
    }

    Match {
        kind: shape.kind.to_string(),
        provider: shape.provider,
        label: shape.label,
        line: filter::line_of(text, range.start),
        masked: mask(value, shape.prefix_len),
        dummy,
        confidence: score.clamp(0, 100) as u8,
        signals,
        range,
    }
}

/// The verdict on a value assigned to a secret-sounding name (2.2).
fn judge_assignment(path: &str, text: &str, range: std::ops::Range<usize>) -> Match {
    let value = &text[range.clone()];
    let mut signals = vec![signal(
        "format",
        true,
        "a literal of 20 or more characters assigned to a secret-sounding name",
    )];
    let mut score: i32 = 50;
    let (random, why) = randomness(value);
    score += random;
    if random != 0 {
        signals.push(signal("randomness", random > 0, &why));
    }
    score += location(path, text, range.start, &mut signals);
    let dummy = dummy_rule(value, value);
    if let Some(rule) = dummy {
        score = score.min(10);
        signals.push(signal("dummy", false, rule));
    }
    Match {
        kind: "a secret assigned to a key-like name".to_string(),
        provider: "generic",
        label: "secret",
        line: filter::line_of(text, range.start),
        masked: mask(value, 0),
        dummy,
        confidence: score.clamp(0, 100) as u8,
        signals,
        range,
    }
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
    let upper = body.to_ascii_uppercase();
    if MARKERS.iter().any(|m| upper.contains(m)) {
        return Some("(b) a marker such as EXAMPLE or FAKE in the body");
    }
    if upper.contains("XXXXXXXX") {
        return Some("(b) a run of eight or more X");
    }
    let significant: Vec<char> = body.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if !significant.is_empty() && significant.iter().all(|c| *c == significant[0]) {
        return Some("(b) one character repeated");
    }
    None
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
        "slack" => value.split('-').count() >= 4,
        "stripe" => body.len() >= 24 && alnum(body),
        "semlith" => body.len() == 64,
        "jwt" => jwt_header_parses(value),
        _ => return None,
    })
}

/// A score adjustment for how random a body is, and the words for it.
fn randomness(body: &str) -> (i32, String) {
    let chars: Vec<char> = body.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if chars.len() < 8 {
        return (0, String::new());
    }
    if sequential(&chars) {
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

/// Eight or more characters rising or falling by one, or a dictionary run.
fn sequential(chars: &[char]) -> bool {
    let mut run = 1;
    for pair in chars.windows(2) {
        let step = pair[1] as i32 - pair[0] as i32;
        if step == 1 || step == -1 {
            run += 1;
            if run >= 8 {
                return true;
            }
        } else {
            run = 1;
        }
    }
    false
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
    let before = &text[..at];
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
fn aws_secret_nearby(text: &str, range: std::ops::Range<usize>) -> bool {
    let line = filter::line_of(text, range.start) as usize;
    text.lines()
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
    let rest = &text[header.end..];
    if let Some(end) = rest.find("-----END") {
        let tail = &rest[end..];
        let close = tail[8..]
            .find("-----")
            .map(|i| i + 8 + 5)
            .unwrap_or(tail.len());
        return header.start..header.end + end + close;
    }
    let body: usize = rest
        .char_indices()
        .take_while(|(_, c)| {
            c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '\n' | '\r' | ' ' | '\t')
        })
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    header.start..header.end + body
}

/// The base64 characters of a PEM block's body.
fn pem_body(block: &str) -> String {
    let after = block
        .find("-----\n")
        .or_else(|| block.find("-----\r\n"))
        .map(|i| i + 5);
    let Some(start) = after.or_else(|| block.rfind("-----").map(|i| i + 5)) else {
        return String::new();
    };
    let body = &block[start.min(block.len())..];
    let body = body.split("-----END").next().unwrap_or("");
    body.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
        .collect()
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
    match shape.provider {
        "anthropic" => format!("sk-ant-api03-{}", pick(alnum, 93)),
        "openai" => format!("sk-proj-{}", pick(alnum, 48)),
        "aws" => format!("AKIA{}", pick(upper, 16)),
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
                let mut value = forge(i);
                let last = value.pop().unwrap();
                value.push(if last == 'Z' { 'Y' } else { 'Z' });
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
        let mut live = forge(github);
        let last = live.pop().unwrap();
        live.push(if last == 'a' { 'b' } else { 'a' });
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
}
