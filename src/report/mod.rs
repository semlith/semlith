//! Five reports, generated from this machine's ledger, index and graph.
//!
//! Nothing here reaches the network, nothing here is a model's opinion, and
//! nothing here is a number without its denominator. A report is one typed
//! structure — a lede, a meta strip, KPI figures and a list of blocks (tables,
//! facts, notes, a recompute command) — and five renderers turn that one
//! structure into Markdown, CSV, JSON, HTML and PDF, so the formats cannot
//! disagree about what a report says and `semlith report`, the portal, a
//! schedule and `semlith_report` produce the same document.
//!
//! From 0.42.0 every figure is typed (`Figure`: value, unit, currency, what it
//! is over, and the display string a person saw), every number is formatted by
//! [`num`], and every renderer draws the reports-v1 design: HTML is the
//! reference, the PDF is typeset natively to match its print rendering
//! (`pdf.rs`), Markdown is built to read on GitHub, and CSV and JSON are data.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

mod pdf;
mod render;

pub use render::REPORT_CSP;

// ------------------------------------------------------------------ numbers

/// A count as every surface prints it: `736,397`, `-130`. The CLI's ledger and
/// stats lines call this; it is [`num::int`].
pub fn number(n: i64) -> String {
    num::int(n)
}

/// The one number formatter: the portal (`app.js`'s `n` and `pct`), the CLI and
/// every export print a figure through these rules, so `711,763` is the same
/// digits on every surface. 0.41 printed `736 397` in reports and `736,397` in
/// the portal.
pub mod num {
    /// `711,763` — US grouping, which is what the portal's
    /// `Intl.NumberFormat("en-US")` prints.
    pub fn int(value: i64) -> String {
        let digits = value.unsigned_abs().to_string();
        let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
        if value < 0 {
            out.push('-');
        }
        for (i, c) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        out
    }

    /// `$1.42`, two places, grouped: `$12,345.60`.
    pub fn usd(value: f64) -> String {
        let cents = (value * 100.0).round() as i64;
        let sign = if cents < 0 { "-" } else { "" };
        format!("{sign}${}.{:02}", int(cents.abs() / 100), cents.abs() % 100)
    }

    /// `80%` for a share between 0 and 1, by `app.js`'s `pct`: one decimal
    /// under 10 % so a small share is not printed as zero, whole above.
    pub fn pct(share: f64) -> String {
        let value = share * 100.0;
        if value > 0.0 && value < 10.0 {
            format!("{value:.1}%")
        } else {
            format!("{}%", value.round() as i64)
        }
    }

    /// `325.3×`.
    pub fn times(value: f64) -> String {
        format!("{}×", grouped_decimal(value, 1))
    }

    /// `67 ms`, from microseconds.
    pub fn ms(micros: i64) -> String {
        format!("{} ms", int((micros as f64 / 1000.0).round() as i64))
    }

    /// `12.3 MB`, binary units, as the portal's `bytes`.
    pub fn bytes(value: i64) -> String {
        let units = ["B", "KB", "MB", "GB", "TB"];
        let mut size = value.max(0) as f64;
        let mut unit = 0;
        while size >= 1024.0 && unit < units.len() - 1 {
            size /= 1024.0;
            unit += 1;
        }
        if unit == 0 {
            format!("{} B", int(size as i64))
        } else {
            format!("{size:.1} {}", units[unit])
        }
    }

    fn grouped_decimal(value: f64, places: usize) -> String {
        let text = format!("{:.*}", places, value.abs());
        let (whole, frac) = text.split_once('.').unwrap_or((&text, ""));
        let whole: i64 = whole.parse().unwrap_or(0);
        let sign = if value < 0.0 { "-" } else { "" };
        if frac.is_empty() {
            format!("{sign}{}", int(whole))
        } else {
            format!("{sign}{}.{frac}", int(whole))
        }
    }
}

/// `2026-10-14T20:13:04+05:30` — the ledger's own local stamp as ISO 8601.
fn iso(at: i64) -> String {
    let stamp = crate::clock::local_stamp(at);
    let mut parts = stamp.split(' ');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(date), Some(time), Some(offset)) => format!("{date}T{time}{offset}"),
        _ => stamp,
    }
}

/// `2026-10-14`.
fn day(at: i64) -> String {
    crate::clock::local_stamp(at)
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_string()
}

// -------------------------------------------------------------- the window

/// How far back a report looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// Everything recorded.
    All,
    /// The last N days.
    Days(i64),
}

/// The window vocabulary, and the only spellings [`window_of`] accepts.
pub const WINDOWS: [(&str, Window); 5] = [
    ("all", Window::All),
    ("day", Window::Days(1)),
    ("week", Window::Days(7)),
    ("month", Window::Days(30)),
    ("quarter", Window::Days(90)),
];

/// The window every surface uses when none is named: the CLI, the portal's
/// Reports page, `/api/report`, `semlith_report` and a schedule. The portal
/// opened at 30 days and the CLI at everything in 0.41 (R11), so the same
/// command printed two documents.
pub const DEFAULT_WINDOW: &str = "month";

/// Resolve a window name, or say which there are. `None` is [`DEFAULT_WINDOW`].
pub fn window_of(name: Option<&str>) -> Result<Window> {
    let name = name.unwrap_or(DEFAULT_WINDOW);
    match WINDOWS.iter().find(|(n, _)| *n == name) {
        Some((_, window)) => Ok(*window),
        None => bail!(
            "unknown window {name:?}; the windows are {}",
            WINDOWS
                .iter()
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

impl Window {
    /// The epoch second a row must be at or after; 0 for everything.
    pub fn since(self, now: i64) -> i64 {
        match self {
            Window::All => 0,
            Window::Days(days) => now - days * 24 * 60 * 60,
        }
    }

    /// The name [`window_of`] takes back.
    pub fn name(self) -> &'static str {
        WINDOWS
            .iter()
            .find(|(_, w)| *w == self)
            .map_or("all", |(n, _)| *n)
    }

    /// `Last 7 days`.
    pub fn label(self) -> String {
        match self {
            Window::All => "Everything recorded".into(),
            Window::Days(1) => "Last 24 hours".into(),
            Window::Days(days) => format!("Last {days} days"),
        }
    }

    /// `in the last 7 days`, for a sentence.
    fn phrase(self) -> String {
        match self {
            Window::All => "since the ledger began".into(),
            Window::Days(1) => "in the last 24 hours".into(),
            Window::Days(days) => format!("in the last {days} days"),
        }
    }
}

/// The window a report was taken over, as every renderer prints it.
#[derive(Debug, Clone, Serialize)]
pub struct WindowInfo {
    /// `week`, as [`window_of`] takes it.
    pub name: String,
    /// `Last 7 days`.
    pub label: String,
    /// ISO 8601; absent for everything recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub to: String,
    /// Whether this report's figures carry a date to narrow by. The health
    /// report is the index as it stands, and says so instead of printing a
    /// span it did not apply.
    pub applies: bool,
}

// ------------------------------------------------------------- the kinds

/// The five reports: id, title, audience.
pub const KINDS: [(&str, &str); 5] = [
    ("savings", "Retrieval savings"),
    ("access", "AI access audit"),
    ("change", "Change brief"),
    ("health", "Index health"),
    ("gaps", "Knowledge gaps"),
];

fn audience(kind: &str) -> &'static str {
    match kind {
        "savings" => "for whoever approves the spend",
        "access" => "for security review",
        "change" => "for the reviewer",
        "health" => "for whoever owns the store",
        _ => "for whoever writes the docs",
    }
}

/// The formats [`Report::render`] returns as text. CSV and PDF are bytes: a
/// report's CSV is a zip of one file per table, unless one table is named
/// ([`Report::render_table`]).
pub const FORMATS: [&str; 3] = ["markdown", "json", "html"];

/// The PDF.
pub const PDF: &str = "pdf";

/// CSV: a zip of tables, or one table by name.
pub const CSV: &str = "csv";

/// The five formats, in the order a caller is offered them.
pub const ALL_FORMATS: [&str; 5] = ["markdown", CSV, "json", "html", PDF];

/// The schema the JSON format and `/api/report` carry, published in
/// `docs/reports.md`.
pub const SCHEMA: &str = "semlith.report/2";

/// The model a savings figure is priced at when nobody names one.
pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";

/// The models a savings figure is offered at, at least one per vendor the
/// supported clients reach, from the same models.dev table the ledger's usage
/// columns are priced by (`crate::prices`). Any model in that table can be
/// asked for by name; these are the ones a picker lists first.
pub const SAVINGS_MODELS: &[&str] = &[
    "claude-sonnet-5-5",
    "claude-opus-5-5",
    "claude-haiku-4-5",
    "claude-fable-5-1",
    "gpt-5.6",
    "gpt-5.5",
    "gpt-5.4-mini",
    "gpt-5.3-codex",
    "gemini-3.1-pro-preview",
    "gemini-3.5-flash",
    "grok-4.7",
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "kimi-k3",
    "glm-5.3",
    "mistral-large-latest",
];

/// The three names this binary priced by before 0.34.0, kept so a schedule or
/// a script that saved one still runs — now at that model's table price.
const ALIASES: [(&str, &str, &str); 3] = [
    ("sonnet5", "Sonnet 5", "claude-sonnet-5"),
    ("opus5", "Opus 5", "claude-opus-5"),
    ("haiku45", "Haiku 4.5", "claude-haiku-4-5"),
];

/// A model name reduced to what is worth comparing: letters and digits only,
/// folded to lower case. `Opus 5`, `opus-5` and `opus_5` are one model.
fn key_of(model: &str) -> String {
    model
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A model's name and what a million of its input tokens cost, if the price
/// table carries it. Input pricing, because a retrieval is something an agent
/// reads. A spelling nobody prices is `None`, never the default's figure.
pub fn price_named(model: &str) -> Option<(String, f64)> {
    let wanted = key_of(model);
    if wanted.is_empty() {
        return None;
    }
    let table = crate::prices::table();
    if let Some((_, label, id)) = ALIASES.iter().find(|(key, ..)| *key == wanted) {
        return crate::prices::lookup(&table, None, id)
            .map(|(_, p)| (label.to_string(), p.rates.input));
    }
    let (key, price) = crate::prices::lookup(&table, None, model)?;
    let name = key
        .split_once('/')
        .map_or(key.as_str(), |(_, m)| m)
        .to_string();
    Some((name, price.rates.input))
}

/// The price of a model by name, or the default's.
pub fn price_of(model: &str) -> (String, f64) {
    price_named(model)
        .or_else(|| price_named(DEFAULT_MODEL))
        .unwrap_or_else(|| (DEFAULT_MODEL.to_string(), 0.0))
}

/// What a refusal names as working instead.
pub fn price_names() -> String {
    format!(
        "any model in the price table (`semlith prices --model NAME`), such as {}",
        SAVINGS_MODELS[..6].join(", ")
    )
}

/// The picker's list: each of [`SAVINGS_MODELS`] the table carries, with its
/// input price.
pub fn savings_models() -> Vec<(String, f64)> {
    SAVINGS_MODELS
        .iter()
        .filter_map(|m| price_named(m))
        .collect()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Resolve a report name, or say which five there are — before a store is
/// opened, so a typo is a question about the argument.
pub fn kind_of(kind: &str) -> Result<(&'static str, &'static str)> {
    match KINDS.iter().find(|(k, _)| *k == kind) {
        Some((kind, title)) => Ok((kind, title)),
        None => bail!(
            "unknown report {kind:?}; the reports are {}",
            KINDS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Check a format a text-only surface can print: markdown, json or html.
pub fn check_format(format: &str) -> Result<()> {
    if FORMATS.contains(&format) || format == "md" {
        return Ok(());
    }
    bail!(
        "unknown format {format:?}; the formats are {}",
        FORMATS.join(", ")
    )
}

/// Check a format name against all five, CSV and PDF included.
pub fn check_any_format(format: &str) -> Result<()> {
    if ALL_FORMATS.contains(&format) || format == "md" {
        return Ok(());
    }
    bail!(
        "unknown format {format:?}; the formats are {}",
        ALL_FORMATS.join(", ")
    )
}

// ------------------------------------------------------------- the model

/// A number with what it means: the value as data, its unit, the currency
/// where it is money, what it is over, and the string a person saw.
#[derive(Debug, Clone, Serialize)]
pub struct Figure {
    pub value: Value,
    pub unit: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<&'static str>,
    pub over: String,
    pub display: String,
}

impl Figure {
    pub fn count(value: i64, unit: &'static str, over: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            unit,
            currency: None,
            over: over.into(),
            display: num::int(value),
        }
    }

    pub fn usd(value: f64, over: impl Into<String>) -> Self {
        Self {
            value: round(value, 6),
            unit: "money",
            currency: Some("USD"),
            over: over.into(),
            display: num::usd(value),
        }
    }

    /// A share: `part` of `whole`, as a ratio, displayed as a percentage.
    pub fn share(part: i64, whole: i64, over: impl Into<String>) -> Self {
        let ratio = if whole == 0 {
            0.0
        } else {
            part as f64 / whole as f64
        };
        Self {
            value: round(ratio, 4),
            unit: "ratio",
            currency: None,
            over: over.into(),
            display: num::pct(ratio),
        }
    }

    pub fn times(value: Option<f64>, over: impl Into<String>) -> Self {
        Self {
            value: value.map_or(Value::Null, |v| round(v, 1)),
            unit: "times",
            currency: None,
            over: over.into(),
            display: value.map_or("—".into(), num::times),
        }
    }

    pub fn ms(micros: i64, over: impl Into<String>) -> Self {
        Self {
            value: round(micros as f64 / 1000.0, 1),
            unit: "ms",
            currency: None,
            over: over.into(),
            display: num::ms(micros),
        }
    }

    pub fn bytes(value: i64, over: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            unit: "bytes",
            currency: None,
            over: over.into(),
            display: num::bytes(value),
        }
    }

    pub fn text(value: impl Into<String>, over: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            value: Value::String(value.clone()),
            unit: "text",
            currency: None,
            over: over.into(),
            display: value,
        }
    }

    /// The same figure with a display a person reads better, the value kept.
    fn shown(mut self, display: impl Into<String>) -> Self {
        self.display = display.into();
        self
    }
}

fn round(value: f64, places: i32) -> Value {
    let scale = 10f64.powi(places);
    let rounded = (value * scale).round() / scale;
    serde_json::Number::from_f64(rounded).map_or(Value::Null, Value::Number)
}

/// A table cell: the value as data and the display a person reads.
#[derive(Debug, Clone)]
pub struct Cell {
    pub value: Value,
    pub display: String,
}

impl Cell {
    pub fn int(value: i64) -> Self {
        Self {
            value: value.into(),
            display: num::int(value),
        }
    }
    pub fn text(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            value: Value::String(value.clone()),
            display: value,
        }
    }
    pub fn usd(value: f64) -> Self {
        Self {
            value: round(value, 6),
            display: num::usd(value),
        }
    }
    /// A share, 0 to 1, printed as a percentage.
    pub fn share(ratio: f64) -> Self {
        Self {
            value: round(ratio, 4),
            display: num::pct(ratio),
        }
    }
    /// A moment: ISO 8601 with its offset as data; `2026-10-14 19:59:41` for
    /// a person. The offset is the same on every row, so a table drops it and
    /// the footer's generated stamp carries it.
    pub fn stamp(at: i64) -> Self {
        let local = crate::clock::local_stamp(at);
        let display = local
            .rsplit_once(' ')
            .map_or(local.as_str(), |(stamp, _)| stamp)
            .to_string();
        Self {
            value: Value::String(iso(at)),
            display,
        }
    }
    pub fn empty() -> Self {
        Self {
            value: Value::Null,
            display: "—".into(),
        }
    }
}

impl Serialize for Cell {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(s)
    }
}

/// How a column is drawn. The renderers read this and nothing else to decide
/// alignment, face and wrapping, so every format lays a table out the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Prose that may wrap.
    Text,
    /// The row's name, in the heavier face.
    Strong,
    /// A description, in the muted ink.
    Muted,
    /// An id, a path, a version or a stamp: mono, never wrapped — a PDF
    /// ellipsizes one that does not fit rather than cutting it mid-token.
    Id,
    /// A number: mono, right-aligned, never wrapped.
    Num,
    /// A share drawn as a bar beside its percentage.
    Bar,
}

#[derive(Debug, Clone, Serialize)]
pub struct Column {
    /// The stable snake_case key a CSV header and a JSON reader use; carries
    /// its unit (`net_tokens`, `cost_usd`, `coverage_ratio`).
    pub key: &'static str,
    pub label: String,
    pub kind: Kind,
}

fn col(key: &'static str, label: &str, kind: Kind) -> Column {
    Column {
        key,
        label: label.to_string(),
        kind,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Table {
    /// What `--table` names it by and what its CSV file is called.
    pub id: &'static str,
    pub title: String,
    /// The small line at the right of the card's header.
    pub meta: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<Vec<Cell>>,
    /// Rows the table does not list, said rather than implied.
    #[serde(skip_serializing_if = "is_zero")]
    pub more: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// What it says when there are no rows.
    pub empty: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize)]
pub struct Fact {
    pub label: String,
    #[serde(flatten)]
    pub figure: Figure,
}

#[derive(Debug, Clone, Serialize)]
pub struct Kpi {
    pub label: String,
    #[serde(flatten)]
    pub figure: Figure,
    /// The headline figure, drawn with the accent edge.
    pub lead: bool,
}

/// A pill: a store name, a chain state, a redaction state.
#[derive(Debug, Clone, Serialize)]
pub struct Chip {
    pub text: String,
    /// `green`, `amber`, `blue` or empty for the neutral pill.
    #[serde(skip_serializing_if = "str::is_empty")]
    pub tone: &'static str,
}

/// One cell of the meta strip.
#[derive(Debug, Clone, Serialize)]
pub struct Meta {
    pub label: String,
    /// The first line in ink, the rest muted.
    pub lines: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chips: Vec<Chip>,
}

/// One block of a report, in the order it is drawn.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "block", rename_all = "lowercase")]
pub enum Block {
    Table(Table),
    /// Secondary figures, three to a row.
    Facts {
        title: String,
        meta: String,
        facts: Vec<Fact>,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// A sentence that explains a rule.
    Note {
        text: String,
    },
    /// The exact commands that reproduce the document.
    Code {
        title: String,
        meta: String,
        comment: String,
        lines: Vec<String>,
    },
}

/// A run of the lede, bold or not.
#[derive(Debug, Clone)]
pub struct Run {
    pub text: String,
    pub bold: bool,
}

fn runs(parts: &[(&str, bool)]) -> Vec<Run> {
    parts
        .iter()
        .map(|(text, bold)| Run {
            text: text.to_string(),
            bold: *bold,
        })
        .collect()
}

fn lede_text<S: serde::Serializer>(lede: &[Run], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&lede.iter().map(|r| r.text.as_str()).collect::<String>())
}

/// The ledger's hash chain, as a report states it.
#[derive(Debug, Clone, Serialize)]
pub struct Chain {
    /// `verifies`, `broken` or `empty`.
    pub state: &'static str,
    pub rows: i64,
    /// The newest row's hash, of the store holding the newest row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}

/// One generated report.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub kind: &'static str,
    pub title: String,
    pub audience: String,
    #[serde(serialize_with = "lede_text")]
    pub lede: Vec<Run>,
    /// The local stamp a person reads, and its ISO form.
    pub generated: String,
    pub generated_at: String,
    pub version: String,
    pub window: WindowInfo,
    /// The stores this was generated over, after narrowing.
    pub stores: Vec<String>,
    pub redacted: bool,
    pub meta: Vec<Meta>,
    pub kpis: Vec<Kpi>,
    pub blocks: Vec<Block>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<Chain>,
    /// Where it was generated, when not on this machine. Absent when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl Report {
    /// A report with a title and nothing in it: what a renderer test, or a
    /// program embedding the library, starts from.
    pub fn new(kind: &'static str, title: &str) -> Self {
        let now = unix_now();
        Self {
            schema: SCHEMA,
            kind,
            title: title.to_string(),
            audience: audience(kind).to_string(),
            lede: Vec::new(),
            generated: crate::clock::local_stamp(now),
            generated_at: iso(now),
            version: env!("CARGO_PKG_VERSION").to_string(),
            window: WindowInfo {
                name: "all".into(),
                label: Window::All.label(),
                from: None,
                to: iso(now),
                applies: false,
            },
            stores: Vec::new(),
            redacted: false,
            meta: Vec::new(),
            kpis: Vec::new(),
            blocks: Vec::new(),
            chain: None,
            origin: None,
        }
    }

    /// Every table, in order, as `--table` names them.
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.blocks.iter().filter_map(|b| match b {
            Block::Table(t) => Some(t),
            _ => None,
        })
    }

    /// The lede as plain text.
    pub fn lede_plain(&self) -> String {
        self.lede.iter().map(|r| r.text.as_str()).collect()
    }
}

/// The builder's two content toggles.
///
/// Not three: the design draws a `Sign the report` toggle as well, and signing
/// needs a key this product does not have. A toggle drawn with no wire behind
/// it is worse than one not drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Attach the per-retrieval rows behind the session totals: who asked,
    /// when, with which tool, how many hits and what they cost. The ledger
    /// keeps counts, not the lines an agent was shown, so this attaches rows
    /// and never lines.
    pub excerpts: bool,
    /// Replace every query text with a keyed digest of it.
    pub redact: bool,
    /// The key the digest is made with; `None` is this machine's
    /// (`home::report_salt`). A test names its own rather than writing one
    /// into a real home.
    pub salt: Option<[u8; 32]>,
}

// ------------------------------------------------------------ generating

/// The stores one report is over: the scope, already narrowed.
type Scope<'a> = [(&'a str, &'a crate::Semlith)];

/// Generate one report over every open store, in the default window.
pub fn generate(fleet: &crate::fleet::Fleet, kind: &str, model: &str) -> Result<Report> {
    generate_over(fleet, kind, model, window_of(None)?, &[])
}

/// Generate one report over a window and a scope.
pub fn generate_over(
    fleet: &crate::fleet::Fleet,
    kind: &str,
    model: &str,
    window: Window,
    only: &[String],
) -> Result<Report> {
    generate_with(fleet, kind, model, window, only, Options::default())
}

/// What every report is generated from: the scope, the window, the toggles.
struct Ctx<'a> {
    scope: &'a Scope<'a>,
    window: Window,
    now: i64,
    since: i64,
    options: Options,
    model: String,
    short: crate::Shortener,
    salt: [u8; 32],
    /// Whether the caller named stores, so the recompute command does too.
    narrowed: bool,
}

impl Ctx<'_> {
    /// A query text as it reaches paper: itself, or a keyed digest of it.
    ///
    /// Keyed with a per-machine salt (R7): an unkeyed digest of a symbol name
    /// is a dictionary lookup away from the name, so `detect_binary` and
    /// `blueprint registration` were reversed by guessing. Report-wide rather
    /// than per block: a toggle that hid a query in one table and printed it
    /// in another would be a promise broken by the document that made it.
    fn said(&self, query: &str) -> String {
        if !self.options.redact {
            return query.to_string();
        }
        format!(
            "hash:{}",
            &blake3::keyed_hash(&self.salt, query.as_bytes()).to_hex()[..16]
        )
    }

    fn path(&self, path: &str) -> String {
        crate::plain(&self.short.short(path))
    }

    /// `--window month --scope a --scope b`, as the recompute card prints it:
    /// only the flags this report reads.
    fn flags(&self, kind: &str) -> String {
        let mut out = format!("semlith report {kind}");
        if kind != "health" {
            out.push_str(&format!(" --window {}", self.window.name()));
        }
        if self.narrowed {
            for (label, _) in self.scope {
                out.push_str(&format!(" --scope {label}"));
            }
        }
        if kind == "savings" {
            out.push_str(&format!(" --model {}", self.model));
        }
        if self.options.redact && matches!(kind, "access" | "gaps") {
            out.push_str(" --redact");
        }
        if self.options.excerpts && kind == "access" {
            out.push_str(" --excerpts");
        }
        out
    }
}

/// Generate one report with the builder's toggles applied.
///
/// `only` is store labels; empty is every open store. A label no open store
/// answers to is refused rather than dropped.
pub fn generate_with(
    fleet: &crate::fleet::Fleet,
    kind: &str,
    model: &str,
    window: Window,
    only: &[String],
    options: Options,
) -> Result<Report> {
    let (kind, title) = kind_of(kind)?;
    let open: Vec<&str> = fleet.each().map(|(label, _)| label).collect();
    for name in only {
        if !open.contains(&name.as_str()) {
            bail!(
                "no open store is named {name:?}; the open stores are {}",
                if open.is_empty() {
                    "none".to_string()
                } else {
                    open.join(", ")
                }
            );
        }
    }
    let scope: Vec<(&str, &crate::Semlith)> = fleet
        .each()
        .filter(|(label, _)| only.is_empty() || only.iter().any(|n| n == label))
        .collect();
    let salt = match (options.redact, options.salt) {
        (_, Some(salt)) => salt,
        (true, None) => {
            crate::home::report_salt().context("making this machine's key for query digests")?
        }
        (false, None) => [0; 32],
    };
    // Paths print relative to the root that holds them: the registry's roots,
    // and for a store the registry does not name, the directory every one of
    // its files sits under.
    let mut roots = fleet.roots();
    for (_, store) in &scope {
        if let Some((first, last)) = crate::store::path_bounds(store.db())? {
            let common: std::path::PathBuf = std::path::Path::new(&first)
                .components()
                .zip(std::path::Path::new(&last).components())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect();
            let common = if common.as_os_str() == first.as_str() {
                common
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or(common)
            } else {
                common
            };
            if common.components().count() > 1 {
                roots.push(common);
            }
        }
    }
    let now = unix_now();
    let ctx = Ctx {
        scope: &scope,
        window,
        now,
        since: window.since(now),
        options,
        model: price_of(model).0,
        short: crate::Shortener::new(roots),
        salt,
        narrowed: !only.is_empty(),
    };
    let mut report = Report::new(kind, title);
    report.stores = scope.iter().map(|(l, _)| l.to_string()).collect();
    report.redacted = options.redact && matches!(kind, "access" | "gaps");
    report.window = WindowInfo {
        name: window.name().into(),
        label: window.label(),
        from: (window != Window::All).then(|| iso(ctx.since)),
        to: iso(now),
        applies: kind != "health",
    };
    match kind {
        "savings" => savings(&ctx, &mut report)?,
        "access" => access(&ctx, &mut report)?,
        "change" => change(&ctx, &mut report)?,
        "health" => health(&ctx, &mut report)?,
        _ => gaps(&ctx, &mut report)?,
    }
    Ok(report)
}

// ----------------------------------------------------- shared meta cells

fn window_meta(ctx: &Ctx<'_>, note: Option<&str>) -> Meta {
    if note.is_some_and(|n| n.starts_with("the index as it stands")) {
        return Meta {
            label: "Window".into(),
            lines: vec![
                "The index as it stands".into(),
                format!("on {} · no date to narrow by", day(ctx.now)),
            ],
            chips: Vec::new(),
        };
    }
    let mut lines = vec![ctx.window.label()];
    match (ctx.window, note) {
        (_, Some(note)) => lines.push(note.to_string()),
        (Window::All, None) => lines.push(format!("to {}", day(ctx.now))),
        (Window::Days(_), None) => lines.push(format!("{} → {}", day(ctx.since), day(ctx.now))),
    }
    Meta {
        label: "Window".into(),
        lines,
        chips: Vec::new(),
    }
}

fn stores_meta(ctx: &Ctx<'_>) -> Meta {
    Meta {
        label: "Stores".into(),
        lines: if ctx.scope.is_empty() {
            vec!["no open store".into()]
        } else {
            Vec::new()
        },
        chips: ctx
            .scope
            .iter()
            .map(|(label, _)| Chip {
                text: label.to_string(),
                tone: "",
            })
            .collect(),
    }
}

/// The chain over every store in scope: it verifies only if each store's does.
fn chain_of(ctx: &Ctx<'_>) -> Result<Chain> {
    let (mut rows, mut intact, mut newest) = (0i64, true, None::<(i64, String)>);
    for (_, store) in ctx.scope {
        let db = store.db();
        rows += crate::store::ledger_rows(db)?;
        intact = intact && crate::store::ledger_break(db)?.is_none();
        if let Some(head) = crate::store::ledger_head(db)? {
            let at: i64 = db
                .query_row("SELECT COALESCE(MAX(at), 0) FROM retrievals", [], |r| {
                    r.get(0)
                })
                .unwrap_or(0);
            if newest.as_ref().is_none_or(|(t, _)| at > *t) {
                newest = Some((at, head));
            }
        }
    }
    Ok(Chain {
        state: if rows == 0 {
            "empty"
        } else if intact {
            "verifies"
        } else {
            "broken"
        },
        rows,
        head: newest.map(|(_, h)| h),
    })
}

fn short_hash(hash: &str) -> String {
    let bare = hash.split_once(':').map_or(hash, |(_, h)| h);
    if bare.len() > 14 {
        format!("{}…{}", &bare[..8], &bare[bare.len() - 4..])
    } else {
        bare.to_string()
    }
}

fn chain_meta(chain: &Chain) -> Meta {
    let (tone, text) = match chain.state {
        "verifies" => ("green", "verifies"),
        "broken" => ("amber", "broken"),
        _ => ("", "no rows yet"),
    };
    let mut lines = Vec::new();
    lines.push(format!(
        "{} rows{}",
        num::int(chain.rows),
        chain
            .head
            .as_deref()
            .map(|h| format!(" · head {}", short_hash(h)))
            .unwrap_or_default()
    ));
    Meta {
        label: "Ledger chain".into(),
        lines,
        chips: vec![Chip {
            text: text.into(),
            tone,
        }],
    }
}

fn recompute(ctx: &Ctx<'_>, kind: &str, verify: bool) -> Block {
    let mut lines = vec![ctx.flags(kind)];
    if verify {
        lines.push("semlith ledger --verify".into());
    }
    Block::Code {
        title: "Recompute it".into(),
        meta: "the same document, without the browser".into(),
        comment: if verify {
            "every figure above comes from this machine's ledger rows".into()
        } else {
            "every figure above comes from this machine's index".into()
        },
        lines,
    }
}

// ------------------------------------------------------- the ledger tally

/// Tools whose rows are not queries: a refund (the hook saw a whole-file
/// read), a note, a refusal, and an acceptance or its revocation. They stay
/// out of every query count, coverage, the saving and the gaps; refunds are
/// counted on their own line.
const NOT_QUERIES: [&str; 5] = [
    crate::ledger::RAW_READ,
    "note",
    "refused",
    "accept",
    "revoke",
];

/// Clients that are a person rather than an agent: recorded, listed as human
/// searches, and kept out of what agents did not read.
const HUMAN: [&str; 2] = ["cli", "portal"];

/// Every figure a ledger report prints, from the rows inside the window.
///
/// The one place the reports count the ledger. ponytail: a report-local
/// aggregation over the rows; when `ledger.rs` grows the shared aggregation
/// every surface renders (0.42.0 item 5), this function is re-pointed at it
/// and nothing that renders a figure changes.
#[derive(Default)]
struct Tally {
    queries: BTreeSet<String>,
    credited: BTreeSet<String>,
    whole: i64,
    excerpt: i64,
    refunds: i64,
    human: BTreeSet<String>,
    micros: Vec<i64>,
    estimated: bool,
    clients: BTreeMap<String, ClientTally>,
    sessions: BTreeMap<(String, String), SessionTally>,
    /// Zero-hit agent queries by their text: times, last asked, stores.
    unanswered: BTreeMap<String, (i64, i64, BTreeSet<String>)>,
    /// Every agent row, newest first, for the attachment.
    rows: Vec<(String, crate::store::ReportRow)>,
}

#[derive(Default)]
struct ClientTally {
    version: String,
    queries: BTreeSet<String>,
    credited: BTreeSet<String>,
    net: i64,
}

#[derive(Default)]
struct SessionTally {
    last: i64,
    version: String,
    stores: BTreeSet<String>,
    queries: BTreeSet<String>,
    credited: BTreeSet<String>,
    net: i64,
}

fn tally(ctx: &Ctx<'_>) -> Result<Tally> {
    let mut t = Tally::default();
    for (label, store) in ctx.scope {
        for row in crate::store::report_rows_since(store.db(), label, ctx.since)? {
            if row.tool == crate::ledger::RAW_READ {
                t.refunds += 1;
                continue;
            }
            if NOT_QUERIES.contains(&row.tool.as_str()) {
                continue;
            }
            if HUMAN.contains(&row.client.as_str()) {
                t.human.insert(row.key.clone());
                continue;
            }
            t.queries.insert(row.key.clone());
            if row.micros > 0 {
                t.micros.push(row.micros);
            }
            let net = if row.hits > 0 {
                row.whole_file_tokens - row.excerpt_tokens
            } else {
                0
            };
            if row.hits > 0 {
                t.credited.insert(row.key.clone());
                t.whole += row.whole_file_tokens;
                t.excerpt += row.excerpt_tokens;
                t.estimated |= row.tokenizer != crate::ledger::MODEL;
            }
            let client = t.clients.entry(row.client.clone()).or_default();
            client.queries.insert(row.key.clone());
            if !row.client_version.is_empty() {
                client.version = row.client_version.clone();
            }
            if row.hits > 0 {
                client.credited.insert(row.key.clone());
            }
            client.net += net;
            let session = t
                .sessions
                .entry((row.session.clone(), row.client.clone()))
                .or_default();
            session.last = session.last.max(row.at);
            session.stores.insert(label.to_string());
            session.queries.insert(row.key.clone());
            if !row.client_version.is_empty() {
                session.version = row.client_version.clone();
            }
            if row.hits > 0 {
                session.credited.insert(row.key.clone());
            }
            session.net += net;
            t.rows.push((label.to_string(), row));
        }
    }
    // A query is answered if any store answered it; zero-hit is the rest,
    // counted once however many stores it asked.
    for (label, row) in &t.rows {
        if !t.credited.contains(&row.key) {
            let entry = t
                .unanswered
                .entry(row.query.clone())
                .or_insert((0, 0, BTreeSet::new()));
            entry.0 += 1;
            entry.1 = entry.1.max(row.at);
            entry.2.insert(label.clone());
        }
    }
    // A multi-store query is several rows and one ask: count times once per key.
    for (query, entry) in t.unanswered.iter_mut() {
        let keys: BTreeSet<&str> = t
            .rows
            .iter()
            .filter(|(_, r)| &r.query == query && !t.credited.contains(&r.key))
            .map(|(_, r)| r.key.as_str())
            .collect();
        entry.0 = keys.len() as i64;
    }
    t.micros.sort_unstable();
    t.rows
        .sort_by(|a, b| b.1.at.cmp(&a.1.at).then(b.1.id.cmp(&a.1.id)));
    Ok(t)
}

impl Tally {
    fn percentile(&self, q: f64) -> i64 {
        if self.micros.is_empty() {
            return 0;
        }
        self.micros[((self.micros.len() as f64 - 1.0) * q).round() as usize]
    }

    fn net(&self) -> i64 {
        self.whole - self.excerpt
    }

    fn zero_hit(&self) -> i64 {
        (self.queries.len() - self.credited.len()) as i64
    }

    fn tier(&self) -> &'static str {
        if self.credited.is_empty() || self.estimated {
            "modelled"
        } else {
            "measured"
        }
    }

    fn ratio(&self) -> Option<f64> {
        (self.excerpt > 0).then(|| self.whole as f64 / self.excerpt as f64)
    }
}

fn s(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", num::int(n as i64), if n == 1 { one } else { many })
}

// --------------------------------------------------------------- savings

fn savings(ctx: &Ctx<'_>, report: &mut Report) -> Result<()> {
    let (name, per_million) = price_of(&ctx.model);
    let table = crate::prices::table();
    let t = tally(ctx)?;
    let chain = chain_of(ctx)?;
    let cost = |tokens: i64| tokens as f64 / 1e6 * per_million;
    let (queries, credited) = (t.queries.len(), t.credited.len());
    let ratio = t.ratio();

    report.lede = if queries == 0 {
        runs(&[
            ("No agent asked semlith anything ", false),
            (&ctx.window.phrase(), false),
            (
                ". Every figure below is zero rather than missing, and still recomputable.",
                false,
            ),
        ])
    } else {
        let ratio_text = ratio.map_or("—".to_string(), num::times);
        let asked = s(queries, "question", "questions");
        let answered = num::int(credited as i64);
        let fewer = format!("{ratio_text} fewer tokens");
        let window = format!(" {}. Semlith answered ", ctx.window.phrase());
        runs(&[
            ("Agents on this machine asked ", false),
            (&asked, true),
            (&window, false),
            (&answered, true),
            (
                " of them with excerpts instead of whole files, and the excerpts cost ",
                false,
            ),
            (&fewer, true),
            (" than the files they came from.", false),
        ])
    };
    report.meta = vec![
        window_meta(ctx, None),
        stores_meta(ctx),
        Meta {
            label: "Priced at".into(),
            lines: vec![
                name.clone(),
                format!("{} / M input · {}", num::usd(per_million), table.label()),
            ],
            chips: Vec::new(),
        },
        chain_meta(&chain),
    ];
    report.kpis = vec![
        Kpi {
            label: "Net tokens not read".into(),
            figure: Figure::count(
                t.net(),
                "tokens",
                format!(
                    "whole-file less excerpt, {}",
                    s(credited, "credited query", "credited queries")
                ),
            ),
            lead: true,
        },
        Kpi {
            label: "Net cost avoided".into(),
            figure: Figure::usd(cost(t.net()), format!("at {name} input price")),
            lead: false,
        },
        Kpi {
            label: "Coverage".into(),
            figure: Figure::share(
                credited as i64,
                queries as i64,
                format!(
                    "{} of {} agent queries answered",
                    num::int(credited as i64),
                    num::int(queries as i64)
                ),
            ),
            lead: false,
        },
        Kpi {
            label: "Fewer tokens".into(),
            figure: Figure::times(ratio, "than reading those files whole"),
            lead: false,
        },
    ];

    let not_read = Table {
        id: "not_read",
        title: "What was not read".into(),
        meta: format!("tokens · cost at {name}"),
        columns: vec![
            col("line", "Line", Kind::Strong),
            col("tokens", "Tokens", Kind::Num),
            col("cost_usd", "Cost", Kind::Num),
            col("counts", "What it counts", Kind::Muted),
        ],
        rows: vec![
            vec![
                Cell::text("Whole files not read"),
                Cell::int(t.whole),
                Cell::usd(cost(t.whole)),
                Cell::text(
                    "reading every file a query was answered from, counted as the agent would read it",
                ),
            ],
            vec![
                Cell::text("Excerpts read instead"),
                Cell::int(t.excerpt),
                Cell::usd(cost(t.excerpt)),
                Cell::text("what the answers actually cost"),
            ],
        ],
        total: Some(vec![
            Cell::text("Net not read"),
            Cell::int(t.net()),
            Cell::usd(cost(t.net())),
            Cell::text(format!(
                "over {}",
                s(credited, "credited query", "credited queries")
            )),
        ]),
        more: 0,
        note: Some(format!(
            "Refunds ({}) and zero-hit queries ({}) are counted on their own lines below and never in the saving.",
            s(
                t.refunds as usize,
                "file an agent read whole anyway",
                "files an agent read whole anyway"
            ),
            num::int(t.zero_hit())
        )),
        empty: "Nothing recorded in this window.".into(),
    };

    let mut agents: Vec<(&String, &ClientTally)> = t.clients.iter().collect();
    agents.sort_by(|a, b| b.1.net.cmp(&a.1.net).then(a.0.cmp(b.0)));
    let total_net = t.net().max(1);
    let by_agent = Table {
        id: "by_agent",
        title: "By agent".into(),
        meta: "queries · net tokens".into(),
        columns: vec![
            col("agent", "Agent", Kind::Strong),
            col("version", "Version", Kind::Id),
            col("queries", "Queries", Kind::Num),
            col("coverage_ratio", "Coverage", Kind::Num),
            col("net_tokens", "Net tokens", Kind::Num),
            col("share_ratio", "Share", Kind::Bar),
        ],
        rows: agents
            .iter()
            .map(|(client, c)| {
                let q = c.queries.len() as f64;
                vec![
                    Cell::text(client.as_str()),
                    if c.version.is_empty() {
                        Cell::empty()
                    } else {
                        Cell::text(&c.version)
                    },
                    Cell::int(c.queries.len() as i64),
                    Cell::share(if q == 0.0 {
                        0.0
                    } else {
                        c.credited.len() as f64 / q
                    }),
                    Cell::int(c.net),
                    Cell::share(c.net.max(0) as f64 / total_net as f64),
                ]
            })
            .collect(),
        total: None,
        more: 0,
        note: None,
        empty: "No agent asked anything in this window.".into(),
    };

    let refunds_over = if t.refunds > 0 {
        "files read whole anyway · steering hook, measured"
    } else {
        "a floor: no steering hook reported a whole-file read here"
    };
    let over_queries = format!(
        "over {} in the window",
        s(t.micros.len(), "query", "queries")
    );
    let facts = Block::Facts {
        title: "Counted alongside".into(),
        meta: "never summed with the saving".into(),
        facts: vec![
            Fact {
                label: "Refunds".into(),
                figure: Figure::count(t.refunds, "files", refunds_over),
            },
            Fact {
                label: "Zero-hit".into(),
                figure: {
                    let share = if queries == 0 {
                        0.0
                    } else {
                        t.zero_hit() as f64 / queries as f64
                    };
                    let display = format!("{} · {}", num::int(t.zero_hit()), num::pct(share));
                    Figure::count(
                        t.zero_hit(),
                        "queries",
                        "queries the corpus could not answer",
                    )
                    .shown(display)
                },
            },
            Fact {
                label: "Tier".into(),
                figure: Figure::text(
                    t.tier(),
                    if t.tier() == "measured" {
                        "both sides counted with the store's tokenizer"
                    } else {
                        "some rows estimated at four characters a token"
                    },
                ),
            },
            Fact {
                label: "Latency p50".into(),
                figure: Figure::ms(t.percentile(0.50), &over_queries),
            },
            Fact {
                label: "Latency p95".into(),
                figure: Figure::ms(t.percentile(0.95), &over_queries),
            },
            Fact {
                label: "Human searches".into(),
                figure: Figure::count(
                    t.human.len() as i64,
                    "queries",
                    "portal and CLI, recorded, not counted as agent saving",
                ),
            },
        ],
        note: Some(
            "A refund is an agent that did not ask; a zero hit is semlith that did not reach \
             the answer. They call for opposite things, so they are shown apart."
                .into(),
        ),
    };
    report.blocks = vec![
        Block::Table(not_read),
        Block::Table(by_agent),
        facts,
        recompute(ctx, "savings", true),
    ];
    report.chain = Some(chain);
    Ok(())
}

// ---------------------------------------------------------------- access

/// The most individual retrievals one access report attaches. The sessions
/// table is the whole count either way; this bounds the detail.
const ATTACHED: usize = 500;

fn access(ctx: &Ctx<'_>, report: &mut Report) -> Result<()> {
    let t = tally(ctx)?;
    let chain = chain_of(ctx)?;
    let agents: BTreeSet<&str> = t.sessions.keys().map(|(_, c)| c.as_str()).collect();
    let sessions = t.sessions.len();

    report.lede = {
        let sessions_text = s(sessions, "session", "sessions");
        let agents_text = s(agents.len(), "agent", "agents");
        let tail = format!(
            " {}; nothing here has left the store it was written into.",
            ctx.window.phrase()
        );
        runs(&[
            (
                "Which agent asked what of this machine's stores, and when, from the hash-chained record. ",
                false,
            ),
            (&sessions_text, true),
            (" by ", false),
            (&agents_text, true),
            (&tail, false),
        ])
    };
    report.meta = vec![
        window_meta(ctx, Some("rows counted by their own time")),
        stores_meta(ctx),
        Meta {
            label: "Query text".into(),
            lines: vec![if ctx.options.redact {
                "keyed per machine · not reversible".into()
            } else {
                "as the agent asked it".into()
            }],
            chips: vec![Chip {
                text: if ctx.options.redact {
                    "hashed".into()
                } else {
                    "plain".into()
                },
                tone: if ctx.options.redact { "amber" } else { "" },
            }],
        },
        chain_meta(&chain),
    ];
    report.kpis = vec![
        Kpi {
            label: "Agent sessions".into(),
            figure: Figure::count(
                sessions as i64,
                "sessions",
                format!(
                    "{} · one row per conversation",
                    s(agents.len(), "agent", "agents")
                ),
            ),
            lead: true,
        },
        Kpi {
            label: "Queries".into(),
            figure: Figure::count(
                t.queries.len() as i64,
                "queries",
                format!(
                    "{} answered with excerpts",
                    num::int(t.credited.len() as i64)
                ),
            ),
            lead: false,
        },
        Kpi {
            label: "Read whole".into(),
            figure: Figure::count(t.refunds, "files", "refunds seen by the steering hook"),
            lead: false,
        },
        Kpi {
            label: "Human searches".into(),
            figure: Figure::count(
                t.human.len() as i64,
                "queries",
                "portal and CLI · not agent sessions",
            ),
            lead: false,
        },
    ];

    let mut rows: Vec<(&(String, String), &SessionTally)> = t.sessions.iter().collect();
    rows.sort_by(|a, b| b.1.last.cmp(&a.1.last));
    let sessions_table = Table {
        id: "sessions",
        title: "Sessions".into(),
        meta: "one row per conversation · newest first".into(),
        columns: vec![
            col("last_seen", "Last seen", Kind::Id),
            col("session", "Session", Kind::Id),
            col("agent", "Agent", Kind::Strong),
            col("version", "Version", Kind::Id),
            col("stores", "Stores", Kind::Text),
            col("queries", "Queries", Kind::Num),
            col("zero_hit", "Zero-hit", Kind::Num),
            col("net_tokens", "Net tokens", Kind::Num),
        ],
        rows: rows
            .iter()
            .map(|((session, client), st)| {
                vec![
                    Cell::stamp(st.last),
                    if session.is_empty() {
                        Cell::empty()
                    } else {
                        Cell::text(session.as_str())
                    },
                    Cell::text(client.as_str()),
                    if st.version.is_empty() {
                        Cell::empty()
                    } else {
                        Cell::text(&st.version)
                    },
                    Cell::text(st.stores.iter().cloned().collect::<Vec<_>>().join(" · ")),
                    Cell::int(st.queries.len() as i64),
                    Cell::int((st.queries.len() - st.credited.len()) as i64),
                    Cell::int(st.net),
                ]
            })
            .collect(),
        total: None,
        more: 0,
        note: None,
        empty: "No agent session in this window.".into(),
    };
    let mut blocks = vec![Block::Table(sessions_table)];
    if ctx.options.excerpts {
        let listed: Vec<&(String, crate::store::ReportRow)> =
            t.rows.iter().take(ATTACHED).collect();
        blocks.push(Block::Table(Table {
            id: "retrievals",
            title: "What each agent asked".into(),
            meta: "attached retrievals · counts, not lines".into(),
            columns: vec![
                col("at", "At", Kind::Id),
                col("agent", "Agent", Kind::Strong),
                col("store", "Store", Kind::Text),
                col("tool", "Tool", Kind::Id),
                col("query", "Query", if ctx.options.redact { Kind::Id } else { Kind::Text }),
                col("hits", "Hits", Kind::Num),
                col("excerpt_tokens", "Excerpt tokens", Kind::Num),
                col("whole_file_tokens", "Whole-file tokens", Kind::Num),
            ],
            rows: listed
                .iter()
                .map(|(label, row)| {
                    vec![
                        Cell::stamp(row.at),
                        Cell::text(&row.client),
                        Cell::text(label.as_str()),
                        if row.tool.is_empty() {
                            Cell::empty()
                        } else {
                            Cell::text(&row.tool)
                        },
                        Cell::text(ctx.said(&row.query)),
                        Cell::int(row.hits),
                        Cell::int(row.excerpt_tokens),
                        Cell::int(row.whole_file_tokens),
                    ]
                })
                .collect(),
            total: None,
            more: t.rows.len().saturating_sub(ATTACHED),
            note: Some(format!(
                "One row per retrieval as the ledger recorded it: who asked, when, with which tool, \
                 how many hits and what they cost. The ledger keeps counts, not the lines an agent \
                 was shown.{}",
                if ctx.options.redact {
                    " The query is a digest keyed to this machine, so this says who asked and \
                     what it cost, not what was asked."
                } else {
                    ""
                }
            )),
            empty: "No retrieval in this window.".into(),
        }));
    }
    let mut proof = Vec::new();
    for (label, store) in ctx.scope {
        let db = store.db();
        let rows = crate::store::ledger_rows(db)?;
        let broken = crate::store::ledger_break(db)?;
        proof.push(vec![
            Cell::text(*label),
            Cell::int(rows),
            Cell::text(match (rows, broken) {
                (0, _) => "no rows yet".to_string(),
                (_, None) => "verifies".to_string(),
                (_, Some(id)) => format!("breaks at row {id}"),
            }),
            crate::store::ledger_head(db)?.map_or(Cell::empty(), Cell::text),
        ]);
    }
    blocks.push(Block::Table(Table {
        id: "chain",
        title: "Chain proof".into(),
        meta: "each store's own hash chain".into(),
        columns: vec![
            col("store", "Store", Kind::Strong),
            col("rows", "Rows", Kind::Num),
            col("chain", "Chain", Kind::Text),
            col("head", "Head", Kind::Id),
        ],
        rows: proof,
        total: None,
        more: 0,
        note: Some(
            "Rows are hash-chained in each store's own database; editing or deleting one breaks \
             `semlith ledger --verify`."
                .into(),
        ),
        empty: "No store in scope.".into(),
    }));
    blocks.push(recompute(ctx, "access", true));
    report.blocks = blocks;
    report.chain = Some(chain);
    Ok(())
}

// ---------------------------------------------------------------- change

/// How many changed definitions per store the blast radius walks. The walk is
/// `graph::impact` per name; a re-index of a thousand files would otherwise
/// be a thousand walks.
const BLAST_NAMES: usize = 25;

/// Rows the changed-files table lists before saying how many more there are.
const LISTED: usize = 200;

fn change(ctx: &Ctx<'_>, report: &mut Report) -> Result<()> {
    struct Changed {
        store: String,
        path: String,
        at: i64,
        added: Vec<String>,
        removed: Vec<String>,
        resized: Vec<String>,
    }
    let mut changed: Vec<Changed> = Vec::new();
    let mut first = 0usize;
    let mut blast: Vec<Vec<Cell>> = Vec::new();
    let mut reached: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut stores_changed = BTreeSet::new();
    let mut walked = 0usize;
    for (label, store) in ctx.scope {
        let mut names: Vec<(String, String, &'static str)> = Vec::new();
        for file in crate::store::files_read_since(store.db(), ctx.since)? {
            if file.before.is_empty() {
                first += 1;
                continue;
            }
            let before: BTreeMap<&str, u32> = file
                .before
                .iter()
                .map(|(n, _, a, b)| (n.as_str(), b.saturating_sub(*a)))
                .collect();
            let now: BTreeMap<&str, u32> = file
                .now
                .iter()
                .map(|(n, _, a, b)| (n.as_str(), b.saturating_sub(*a)))
                .collect();
            let added: Vec<String> = now
                .keys()
                .filter(|n| !before.contains_key(*n))
                .map(|n| n.to_string())
                .collect();
            let removed: Vec<String> = before
                .keys()
                .filter(|n| !now.contains_key(*n))
                .map(|n| n.to_string())
                .collect();
            let resized: Vec<String> = now
                .iter()
                .filter(|(n, len)| before.get(*n).is_some_and(|b| b != *len))
                .map(|(n, _)| n.to_string())
                .collect();
            // A Markdown heading or a JSON key changes as text and is called by
            // nothing, so the walk is over code definitions only.
            let navigational: BTreeSet<&str> = file
                .before
                .iter()
                .chain(&file.now)
                .filter(|(_, kind, ..)| crate::graph::NAVIGATIONAL_KINDS.contains(&kind.as_str()))
                .map(|(n, ..)| n.as_str())
                .collect();
            for (list, delta) in [
                (&resized, "changed"),
                (&added, "added"),
                (&removed, "removed"),
            ] {
                for name in list {
                    if !navigational.contains(name.as_str()) {
                        names.push((name.clone(), file.path.clone(), delta));
                    }
                }
            }
            stores_changed.insert(label.to_string());
            changed.push(Changed {
                store: label.to_string(),
                path: file.path,
                at: file.indexed_at,
                added,
                removed,
                resized,
            });
        }
        walked += names.len().saturating_sub(BLAST_NAMES);
        for (name, path, delta) in names.into_iter().take(BLAST_NAMES) {
            let impact = crate::graph::impact(
                store.db(),
                &name,
                &[],
                2,
                false,
                &crate::graph::Scope::all(),
            )?;
            for r in &impact.reached {
                reached.insert((label.to_string(), r.path.clone(), r.name.clone()));
            }
            let mut top: Vec<&crate::graph::Reached> = impact.reached.iter().collect();
            top.sort_by_key(|r| r.hop);
            blast.push(vec![
                Cell::text(&name),
                Cell::text(ctx.path(&path)),
                Cell::text(delta),
                Cell::int(impact.reached.len() as i64 + impact.hidden as i64),
                Cell::int(impact.files.len() as i64),
                Cell::text(
                    top.iter()
                        .take(3)
                        .map(|r| r.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ]);
        }
    }
    changed.sort_by(|a, b| b.at.cmp(&a.at).then(a.path.cmp(&b.path)));
    blast.sort_by(|a, b| b[3].value.as_i64().cmp(&a[3].value.as_i64()));
    let definitions: usize = changed
        .iter()
        .map(|c| c.added.len() + c.removed.len() + c.resized.len())
        .sum();

    report.lede = {
        let files = s(changed.len(), "file", "files");
        let defs = s(definitions, "definition", "definitions");
        let callers = s(reached.len(), "caller", "callers");
        let tail = format!(
            " reach. {} read for the first time {} {} not counted as changes.",
            s(first, "file", "files"),
            ctx.window.phrase(),
            if first == 1 { "is" } else { "are" }
        );
        runs(&[
            (&files, true),
            (
                &format!(
                    " this machine had indexed before {} re-read {}, changing ",
                    if changed.len() == 1 { "was" } else { "were" },
                    ctx.window.phrase()
                ),
                false,
            ),
            (&defs, true),
            (" that ", false),
            (&callers, true),
            (&tail, false),
        ])
    };
    report.meta = vec![
        window_meta(ctx, Some("by when the store re-read each file")),
        stores_meta(ctx),
        Meta {
            label: "Blast radius".into(),
            lines: vec![
                "graph::impact, 2 hops".into(),
                "resolved edges only · ambiguous not crossed".into(),
            ],
            chips: Vec::new(),
        },
    ];
    report.kpis = vec![
        Kpi {
            label: "Files re-read".into(),
            figure: Figure::count(
                changed.len() as i64,
                "files",
                format!(
                    "held definitions before · {} first reads not counted",
                    num::int(first as i64)
                ),
            ),
            lead: true,
        },
        Kpi {
            label: "Definitions changed".into(),
            figure: Figure::count(
                definitions as i64,
                "definitions",
                "added, removed or resized",
            ),
            lead: false,
        },
        Kpi {
            label: "Callers reached".into(),
            figure: Figure::count(
                reached.len() as i64,
                "definitions",
                "within two hops of a changed definition",
            ),
            lead: false,
        },
        Kpi {
            label: "Stores".into(),
            figure: Figure::count(
                stores_changed.len() as i64,
                "stores",
                format!(
                    "of {} in scope with a change",
                    num::int(ctx.scope.len() as i64)
                ),
            ),
            lead: false,
        },
    ];
    let more = changed.len().saturating_sub(LISTED);
    let files_table = Table {
        id: "changed_files",
        title: "Changed files".into(),
        meta: "re-read, not rewritten · newest first".into(),
        columns: vec![
            col("file", "File", Kind::Id),
            col("store", "Store", Kind::Text),
            col("reread_at", "Re-read", Kind::Id),
            col("added", "Added", Kind::Num),
            col("removed", "Removed", Kind::Num),
            col("changed", "Changed", Kind::Num),
        ],
        rows: changed
            .iter()
            .take(LISTED)
            .map(|c| {
                vec![
                    Cell::text(ctx.path(&c.path)),
                    Cell::text(&c.store),
                    Cell::stamp(c.at),
                    Cell::int(c.added.len() as i64),
                    Cell::int(c.removed.len() as i64),
                    Cell::int(c.resized.len() as i64),
                ]
            })
            .collect(),
        total: None,
        more,
        note: Some(
            "A file is counted as re-read when a re-index inside the window retired the \
             definitions it held. The store records when it read a file, not when anybody \
             changed it, and it cannot tell a re-read file that held no definitions from a \
             first read; those are counted as first reads."
                .into(),
        ),
        empty: "No file this machine had indexed before was re-read in this window.".into(),
    };
    let blast_table = Table {
        id: "blast_radius",
        title: "Blast radius".into(),
        meta: "changed definition → what reaches it".into(),
        columns: vec![
            col("symbol", "Symbol", Kind::Strong),
            col("file", "File", Kind::Id),
            col("delta", "Delta", Kind::Text),
            col("callers", "Callers", Kind::Num),
            col("files", "Files", Kind::Num),
            col("top_callers", "Nearest callers", Kind::Muted),
        ],
        rows: blast,
        total: None,
        more: walked,
        note: Some(format!(
            "From the impact walk over each changed definition, two hops, resolved edges only; \
             the first {BLAST_NAMES} per store are walked. A removed definition is reached only \
             where another definition of its name remains."
        )),
        empty: "No changed definition to walk from.".into(),
    };
    report.blocks = vec![
        Block::Table(files_table),
        Block::Table(blast_table),
        recompute(ctx, "change", false),
    ];
    Ok(())
}

// ---------------------------------------------------------------- health

fn health(ctx: &Ctx<'_>, report: &mut Report) -> Result<()> {
    let (mut files, mut chunks, mut bytes) = (0i64, 0i64, 0i64);
    let (mut unresolved, mut several) = (0i64, 0i64);
    let (mut settled, mut answerable) = (0usize, 0usize);
    let mut rows = Vec::new();
    let mut refused: BTreeMap<(String, String), i64> = BTreeMap::new();
    for (label, store) in ctx.scope {
        let (f, c, b) = crate::store::stats(store.db())?;
        files += f;
        chunks += c;
        bytes += b;
        let (_, distinct) = crate::store::unresolved_targets(store.db(), 5)?;
        unresolved += distinct;
        several += crate::store::names_with_several_definitions(store.db())?;
        for row in crate::store::coverage_by_language(store.db())? {
            if row.files == 0 && row.definitions == 0 {
                continue;
            }
            settled += row.extracted + row.resolved;
            answerable += row.extracted + row.resolved + row.ambiguous;
            rows.push(vec![
                Cell::text(*label),
                Cell::text(&row.language),
                Cell::int(row.files as i64),
                Cell::int(row.parser_failed as i64),
                Cell::int(row.definitions as i64),
                Cell::int(row.ambiguous as i64),
                Cell::int(row.unresolved as i64),
                // No call edge to settle (prose, data) is not 0 % settled.
                if row.extracted + row.resolved + row.ambiguous == 0 {
                    Cell::empty()
                } else {
                    Cell::share(row.settled_share() as f64 / 100.0)
                },
            ]);
        }
        for r in crate::store::refusals(store.db())? {
            *refused
                .entry((label.to_string(), r.class.clone()))
                .or_default() += i64::from(r.files);
        }
    }
    let share = if answerable == 0 {
        0.0
    } else {
        settled as f64 / answerable as f64
    };
    report.lede = {
        let files_text = s(files as usize, "file", "files");
        let chunks_text = s(chunks as usize, "chunk", "chunks");
        let share_text = num::pct(share);
        runs(&[
            ("This machine's index holds ", false),
            (&files_text, true),
            (
                &format!(" in {}, cut into ", s(ctx.scope.len(), "store", "stores")),
                false,
            ),
            (&chunks_text, true),
            ("; the graph settles ", false),
            (&share_text, true),
            (" of the call edges it can answer on one definition.", false),
        ])
    };
    report.meta = vec![
        window_meta(ctx, Some("the index as it stands · no date to narrow by")),
        stores_meta(ctx),
    ];
    report.kpis = vec![
        Kpi {
            label: "Files".into(),
            figure: Figure::count(files, "files", "indexed"),
            lead: true,
        },
        Kpi {
            label: "Chunks".into(),
            figure: Figure::count(chunks, "chunks", "embedded"),
            lead: false,
        },
        Kpi {
            label: "Source read".into(),
            figure: Figure::bytes(bytes, "the bytes the index was built from"),
            lead: false,
        },
        Kpi {
            label: "Graph settled".into(),
            figure: Figure::share(
                settled as i64,
                answerable as i64,
                "answerable call edges on one definition",
            ),
            lead: false,
        },
    ];
    let languages = Table {
        id: "languages",
        title: "What the graph covers, per language".into(),
        meta: "`semlith stats` prints these rows".into(),
        columns: vec![
            col("store", "Store", Kind::Text),
            col("language", "Language", Kind::Strong),
            col("files", "Files", Kind::Num),
            col("unparsed", "Unparsed", Kind::Num),
            col("definitions", "Definitions", Kind::Num),
            col("ambiguous", "Ambiguous", Kind::Num),
            col("unresolved", "Unresolved", Kind::Num),
            col("settled_ratio", "Settled", Kind::Bar),
        ],
        rows,
        total: None,
        more: 0,
        note: Some(
            "An absent edge and a file the parser gave up on are different problems, which is \
             why they are different columns."
                .into(),
        ),
        empty: "No language with files or definitions.".into(),
    };
    let skipped = Table {
        id: "not_indexed",
        title: "Not indexed".into(),
        meta: "refused or skipped, by reason".into(),
        columns: vec![
            col("store", "Store", Kind::Text),
            col("reason", "Reason", Kind::Strong),
            col("files", "Files", Kind::Num),
        ],
        rows: refused
            .into_iter()
            .map(|((store, class), n)| vec![Cell::text(store), Cell::text(class), Cell::int(n)])
            .collect(),
        total: None,
        more: 0,
        note: Some("`semlith refused` lists each one and what a person decided about it.".into()),
        empty: "Nothing was refused or skipped.".into(),
    };
    let facts = Block::Facts {
        title: "The graph".into(),
        meta: "what an ambiguous or missing edge is made of".into(),
        facts: vec![
            Fact {
                label: "No definition here".into(),
                figure: Figure::count(unresolved, "names", "call targets the graph cannot find"),
            },
            Fact {
                label: "Several definitions".into(),
                figure: Figure::count(several, "names", "the population of every ambiguous edge"),
            },
            Fact {
                label: "Stores".into(),
                figure: Figure::count(ctx.scope.len() as i64, "stores", "in scope"),
            },
        ],
        note: None,
    };
    report.blocks = vec![
        Block::Table(languages),
        Block::Table(skipped),
        facts,
        recompute(ctx, "health", false),
    ];
    Ok(())
}

// ------------------------------------------------------------------ gaps

/// Rows each gaps table lists.
const GAPS_LISTED: usize = 50;

fn gaps(ctx: &Ctx<'_>, report: &mut Report) -> Result<()> {
    let t = tally(ctx)?;
    let mut asked: Vec<(&String, &(i64, i64, BTreeSet<String>))> = t.unanswered.iter().collect();
    asked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(b.1.1.cmp(&a.1.1)));
    let times: i64 = asked.iter().map(|(_, e)| e.0).sum();
    let mut missing = Vec::new();
    let (mut distinct, mut several) = (0i64, 0i64);
    for (label, store) in ctx.scope {
        let (targets, count) = crate::store::unresolved_targets(store.db(), 20)?;
        distinct += count;
        for (name, edges) in targets {
            missing.push(vec![Cell::text(name), Cell::int(edges), Cell::text(*label)]);
        }
        several += crate::store::names_with_several_definitions(store.db())?;
    }
    missing.sort_by(|a, b| b[1].value.as_i64().cmp(&a[1].value.as_i64()));
    report.lede = {
        let q = s(asked.len(), "question", "questions");
        let n = s(distinct as usize, "name", "names");
        runs(&[
            ("Agents asked ", false),
            (&q, true),
            (
                &format!(
                    " {} that semlith found nothing for, and the code calls ",
                    ctx.window.phrase()
                ),
                false,
            ),
            (&n, true),
            (
                " this corpus does not define. A docs to-do list and a dependency list, kept apart.",
                false,
            ),
        ])
    };
    report.meta = vec![
        window_meta(
            ctx,
            Some("questions by when asked · call targets as indexed now"),
        ),
        stores_meta(ctx),
        Meta {
            label: "Query text".into(),
            lines: vec![if ctx.options.redact {
                "keyed per machine · not reversible".into()
            } else {
                "as the agent asked it".into()
            }],
            chips: vec![Chip {
                text: if ctx.options.redact {
                    "hashed".into()
                } else {
                    "plain".into()
                },
                tone: if ctx.options.redact { "amber" } else { "" },
            }],
        },
    ];
    report.kpis = vec![
        Kpi {
            label: "Unanswered questions".into(),
            figure: Figure::count(
                asked.len() as i64,
                "queries",
                format!("asked {} times", num::int(times)),
            ),
            lead: true,
        },
        Kpi {
            label: "Names not found".into(),
            figure: Figure::count(distinct, "names", "called, never defined here"),
            lead: false,
        },
        Kpi {
            label: "Names that mislead".into(),
            figure: Figure::count(several, "names", "defined more than once"),
            lead: false,
        },
    ];
    let more = asked.len().saturating_sub(GAPS_LISTED);
    report.blocks =
        vec![
        Block::Table(Table {
            id: "asked",
            title: "Asked and not answered".into(),
            meta: "agent queries with no hit in any store".into(),
            columns: vec![
                col("query", "Query", if ctx.options.redact { Kind::Id } else { Kind::Strong }),
                col("times", "Times", Kind::Num),
                col("last_asked", "Last asked", Kind::Id),
                col("stores", "Stores", Kind::Text),
            ],
            rows: asked
                .iter()
                .take(GAPS_LISTED)
                .map(|(query, (times, last, stores))| {
                    vec![
                        Cell::text(ctx.said(query)),
                        Cell::int(*times),
                        Cell::stamp(*last),
                        Cell::text(stores.iter().cloned().collect::<Vec<_>>().join(" · ")),
                    ]
                })
                .collect(),
            total: None,
            more,
            note: Some(
                "A question that found nothing is a document this corpus does not hold, or words \
                 nobody wrote. Refunds and human searches are not questions agents asked semlith, \
                 and are not here."
                    .into(),
            ),
            empty: "Every agent question in this window found something.".into(),
        }),
        Block::Table(Table {
            id: "called",
            title: "Called and not found".into(),
            meta: "the 20 most-called per store".into(),
            columns: vec![
                col("name", "Name", Kind::Id),
                col("edges", "Edges", Kind::Num),
                col("store", "Store", Kind::Text),
            ],
            rows: missing,
            total: None,
            more: 0,
            note: Some(
                "A call target with no definition is code this corpus does not contain: the \
                 standard library, a dependency nobody indexed, or a typo. Standard-library names \
                 are not filtered out."
                    .into(),
            ),
            empty: "Every call target has a definition here.".into(),
        }),
        recompute(ctx, "gaps", true),
    ];
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_formatter_prints_what_the_portal_prints() {
        // app.js: `new Intl.NumberFormat("en-US")` for counts and `pct` for
        // shares. The same inputs, the same strings.
        assert_eq!(num::int(736_397), "736,397");
        assert_eq!(num::int(-1_234_567), "-1,234,567");
        assert_eq!(num::int(0), "0");
        assert_eq!(num::usd(1.4235), "$1.42");
        assert_eq!(num::usd(12_345.6), "$12,345.60");
        assert_eq!(num::pct(0.8), "80%");
        assert_eq!(num::pct(0.054), "5.4%");
        assert_eq!(num::times(325.27), "325.3×");
        assert_eq!(num::times(1234.0), "1,234.0×");
        assert_eq!(num::ms(67_400), "67 ms");
        let app = include_str!("../portal/app.js");
        assert!(
            app.contains("new Intl.NumberFormat(\"en-US\")"),
            "app.js left the one rule"
        );
        assert!(
            app.contains("value < 10 && value > 0 ? value.toFixed(1) : Math.round(value)"),
            "app.js's pct no longer matches num::pct"
        );
    }

    #[test]
    fn the_portal_opens_at_the_default_window() {
        let app = include_str!("../portal/app.js");
        assert!(
            app.contains(&format!("window: \"{DEFAULT_WINDOW}\"")),
            "the Reports page's starting window is not report::DEFAULT_WINDOW"
        );
    }

    #[test]
    fn a_window_name_resolves_or_says_which_there_are() {
        assert_eq!(window_of(None).unwrap(), Window::Days(30));
        assert_eq!(window_of(Some("week")).unwrap(), Window::Days(7));
        assert_eq!(window_of(Some("all")).unwrap(), Window::All);
        let e = window_of(Some("fortnight")).unwrap_err().to_string();
        for wanted in ["all", "day", "week", "month", "quarter"] {
            assert!(e.contains(wanted), "{e} does not name {wanted}");
        }
        for (name, window) in WINDOWS {
            assert_eq!(window.name(), name);
        }
    }

    #[test]
    fn a_model_is_priced_however_its_name_is_punctuated() {
        for spelling in ["Opus 5", "opus 5", "opus-5", "opus_5", "OPUS5"] {
            let (name, per_million) = price_of(spelling);
            assert_eq!(name, "Opus 5", "{spelling} priced as {name}");
            let table = crate::prices::built_in();
            let opus = crate::prices::lookup(&table, None, "claude-opus-5")
                .unwrap()
                .1;
            assert_eq!(per_million, opus.rates.input);
        }
        assert_eq!(price_of("gpt-5.5").0, "gpt-5.5");
        assert_eq!(
            price_of("Gemini-3.1-Pro-Preview").0,
            "gemini-3.1-pro-preview"
        );
    }

    #[test]
    fn the_picker_offers_every_vendor() {
        let names: Vec<String> = savings_models().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names.len(), SAVINGS_MODELS.len(), "{names:?}");
        for vendor in ["claude", "gpt", "gemini", "grok", "deepseek"] {
            assert!(names.iter().any(|n| n.starts_with(vendor)), "no {vendor}");
        }
    }

    #[test]
    fn a_model_nobody_prices_is_refused_rather_than_defaulted() {
        assert!(price_named("gpt-9").is_none());
        assert!(price_named("").is_none());
        assert_eq!(
            price_named("haiku 4.5").map(|(n, _)| n).as_deref(),
            Some("Haiku 4.5")
        );
        let names = price_names();
        for wanted in ["semlith prices --model", DEFAULT_MODEL] {
            assert!(names.contains(wanted), "{names} does not name {wanted}");
        }
    }

    #[test]
    fn a_figure_serializes_as_data() {
        let f = Figure::usd(1.4235, "at x");
        let json = serde_json::to_value(&f).unwrap();
        assert_eq!(json["value"], 1.4235);
        assert_eq!(json["unit"], "money");
        assert_eq!(json["currency"], "USD");
        assert_eq!(json["display"], "$1.42");
        let share = serde_json::to_value(Figure::share(8, 10, "of 10")).unwrap();
        assert_eq!(share["value"], 0.8);
        assert_eq!(share["display"], "80%");
    }
}
