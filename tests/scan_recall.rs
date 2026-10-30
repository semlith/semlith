//! The recall gate for the credential scan (0.51.0): a live-shaped value of
//! every shape and every generic class, written into each of twelve kinds of
//! file the way people write them, is refused, and never suggested `in`.
//!
//! The false-positive work that led to 0.51.0 lost a real key three times in
//! draft (an SMTP key, Vault and Discord tokens, a key path with `/test/` in
//! it). A rule that lets a fixture in can let a key in; this file is what
//! says it did not. Every value is built now: a live-looking literal in the
//! source would be refused itself, and GitHub's push protection stops one.

use semlith::filter::SHAPES;
use semlith::keyscan;

/// How a class is written where a line of text, not a string, holds it.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    /// A token or key: sent as a bearer token.
    Token,
    /// A password: sent as Basic `user:password`.
    Password,
    /// A private key: posted as a JSON body.
    Key,
    /// A URL that is itself the secret, or holds one.
    Url,
    /// A whole `Authorization: …` header.
    Header,
}

struct Case {
    what: String,
    /// The name it is assigned to, snake case.
    name: &'static str,
    value: String,
    class: Class,
}

fn random(alphabet: &[u8], n: usize) -> String {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).unwrap();
    bytes
        .iter()
        .map(|b| alphabet[*b as usize % alphabet.len()] as char)
        .collect()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

const ALNUM: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A 16-character password a generator made.
///
/// Drawn again when the draw holds a run of four letters written as a word
/// (`abcd`, `ABCD`, `Abcd`), fewer than three kinds of character, or a run of
/// eight: the short-password test reads those as a person's password or a
/// placeholder, by design, and lets them through.
/// ponytail: that is a known ceiling of the 12-character floor, a few random
/// 16-character passwords in a hundred; the 20-character rule still sees any
/// longer one with a generated stretch.
fn password16() -> String {
    let alphabet = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789!#&*@^-_=+?.~";
    loop {
        let value = random(alphabet, 16);
        let has = |f: fn(&char) -> bool| value.chars().any(|c| f(&c));
        let kinds = [
            has(char::is_ascii_lowercase),
            has(char::is_ascii_uppercase),
            has(char::is_ascii_digit),
            has(char::is_ascii_punctuation),
        ];
        let word = value
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|w| w.len() >= 4);
        if kinds.iter().filter(|k| **k).count() >= 3 && !word {
            return value;
        }
    }
}

fn cases() -> Vec<Case> {
    let mut out: Vec<Case> = SHAPES
        .iter()
        .enumerate()
        .map(|(i, shape)| Case {
            what: shape.kind.to_string(),
            name: "service_token",
            value: keyscan::forge(i),
            class: match (shape.provider, shape.label) {
                ("pem", _) => Class::Key,
                ("connection", _) | (_, "webhook") => Class::Url,
                ("header", _) => Class::Header,
                _ => Class::Token,
            },
        })
        .collect();
    let pem = SHAPES
        .iter()
        .position(|s| s.provider == "pem" && s.label == "private key")
        .unwrap();
    let mut der = [0u8; 32];
    getrandom::fill(&mut der).unwrap();
    out.extend([
        Case {
            what: "an AWS secret access key".into(),
            name: "aws_secret_access_key",
            value: random(
                b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz/+",
                40,
            ),
            class: Class::Token,
        },
        Case {
            what: "a 16-character password".into(),
            name: "db_password",
            value: password16(),
            class: Class::Password,
        },
        Case {
            what: "64 hex".into(),
            name: "webhook_secret",
            value: random(b"0123456789abcdef", 64),
            class: Class::Token,
        },
        Case {
            what: "44 base64".into(),
            name: "api_key",
            value: base64(&der),
            class: Class::Token,
        },
        Case {
            what: "a private key escaped in a JSON string".into(),
            name: "private_key",
            value: keyscan::forge(pem).replace('\n', "\\n"),
            class: Class::Key,
        },
        Case {
            what: "a private key escaped in a TypeScript string".into(),
            name: "private_key",
            value: keyscan::forge(pem)
                .lines()
                .map(|l| format!("{l}\\n"))
                .collect::<Vec<_>>()
                .join("\" +\n  \""),
            class: Class::Key,
        },
    ]);
    out
}

/// `snake_case` as camelCase and as SCREAMING_SNAKE.
fn spellings(name: &str) -> (String, String) {
    let mut camel = String::new();
    let mut up = false;
    for c in name.chars() {
        if c == '_' {
            up = true;
        } else if up {
            camel.push(c.to_ascii_uppercase());
            up = false;
        } else {
            camel.push(c);
        }
    }
    (camel, name.to_ascii_uppercase())
}

/// A value inside a double-quoted string, escaped as JSON escapes it.
fn quoted(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// A value where YAML or a shell needs quotes for it, and not otherwise.
fn bare(value: &str) -> String {
    if value.contains(|c: char| c.is_whitespace() || c == ':' || c == '"' || c == '\'') {
        format!("\"{}\"", quoted(value))
    } else {
        value.to_string()
    }
}

/// The twelve files: name and contents.
fn contexts(case: &Case) -> Vec<(&'static str, String)> {
    let (snake, v) = (case.name, case.value.as_str());
    let (camel, upper) = spellings(snake);
    let q = quoted(v);
    let curl = match case.class {
        Class::Token => format!("curl -H \"Authorization: Bearer {v}\" https://api.internal/v1\n"),
        Class::Password => format!(
            "curl -H \"Authorization: Basic {}\" https://api.internal/v1\n",
            base64(format!("admin:{v}").as_bytes())
        ),
        Class::Key => format!("curl --data '{{\"key\": \"{q}\"}}' https://api.internal/v1/keys\n"),
        Class::Url => format!("curl -X POST \"{v}\"\n"),
        Class::Header => format!("curl -H \"{v}\" https://api.internal/v1\n"),
    };
    vec![
        (".env", format!("# local\n{upper}={}\n", bare(v))),
        ("config.yaml", format!("service:\n  {snake}: {}\n", bare(v))),
        ("settings.json", format!("{{\n  \"{snake}\": \"{q}\"\n}}\n")),
        ("client.py", format!("client = Client({snake}=\"{q}\")\n")),
        (
            "config.go",
            format!("package config\n\nconst {camel} = \"{q}\"\n"),
        ),
        (
            "config.ts",
            format!("export const config = {{\n  {camel}: \"{q}\",\n}};\n"),
        ),
        (
            "deploy.sh",
            format!("#!/bin/sh\nexport {upper}={}\n", bare(v)),
        ),
        (
            "README.md",
            format!("Set it:\n\n```bash\nexport {upper}={}\n```\n", bare(v)),
        ),
        (
            "docker-compose.yml",
            format!(
                "services:\n  app:\n    environment:\n      - {upper}={}\n",
                bare(v)
            ),
        ),
        (
            "secret.yaml",
            format!(
                "apiVersion: v1\nkind: Secret\ndata:\n  {snake}: {}\n",
                base64(v.as_bytes())
            ),
        ),
        (
            "main.tf",
            format!("resource \"app\" \"main\" {{\n  {snake} = \"{q}\"\n}}\n"),
        ),
        ("call.sh", curl),
    ]
}

/// Every forged value, in every context, is refused, and the review list never
/// suggests letting the file in.
#[test]
fn every_live_value_is_refused_in_every_context() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for case in cases() {
        for (file, text) in contexts(&case) {
            checked += 1;
            let path = format!("/work/app/{file}");
            let found = keyscan::scan(&path, &text);
            if !keyscan::refuses(&found) {
                failures.push(format!("{} in {file}: let through {found:?}", case.what));
                continue;
            }
            let stored: Vec<serde_json::Value> = found
                .iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect();
            let review = keyscan::assess(semlith::store::class::CONTENT, &path, &stored, None);
            if review["suggest"] == "in" {
                failures.push(format!("{} in {file}: suggested in: {review}", case.what));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {checked} let through or suggested in:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(checked >= 12 * (SHAPES.len() + 6), "{checked} cases");
}

/// The cases each draft of 0.51.0's rules broke, named so they stay fixed: a
/// live SMTP key whose name an allow list did not expect, Vault's `secret_id`,
/// a Vault token and a Discord token that look like member paths, and a key
/// path that holds `/test/`.
#[test]
fn the_recall_cases_the_drafts_broke_stay_refused() {
    let key = random(ALNUM, 40);
    let vault = SHAPES.iter().position(|s| s.provider == "vault").unwrap();
    let discord = SHAPES.iter().position(|s| s.label == "bot token").unwrap();
    for (what, path, text) in [
        (
            "an SMTP key under a name nobody listed",
            "/work/notes/smtp.txt",
            format!("semlith-cloud-smtp-key: {key}\n"),
        ),
        (
            "Vault's secret_id",
            "/work/app/vault.hcl",
            format!("secret_id = \"{key}\"\n"),
        ),
        (
            "a Vault token",
            "/work/app/main.go",
            format!("token := \"{}\"\n", keyscan::forge(vault)),
        ),
        (
            "a Discord token",
            "/work/bot/main.py",
            format!("bot.run(\"{}\")\n", keyscan::forge(discord)),
        ),
        (
            "a key with /test/ in its body",
            "/work/app/config.yaml",
            format!(
                "signing_key: {}/test/{}\n",
                random(ALNUM, 20),
                random(ALNUM, 20)
            ),
        ),
    ] {
        let found = keyscan::scan(path, &text);
        assert!(keyscan::refuses(&found), "{what} let through: {found:?}");
    }
}
