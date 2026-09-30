//! What a model's tokens cost, for the ledger's usage columns.
//!
//! The table is models.dev's (`https://models.dev/api.json`, MIT), trimmed to
//! the providers the supported clients reach and to the four rates a request
//! is billed at — input, output, cache read, cache write — plus the context
//! tiers some models charge more above. A snapshot is built into the binary;
//! `semlith prices update` fetches a fresh one into `~/.semlith/prices.json`
//! when somebody runs it, and never otherwise. semlith contacts nothing on its
//! own, and a price table is not a reason to start.
//!
//! Every priced row names the table and its date, because a price is a fact
//! about a day and the ledger keeps rows for months.
//!
//! The built-in snapshot is refreshed each release by running
//! `semlith prices update` and copying `~/.semlith/prices.json` over
//! `src/prices.json`.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;

/// Where `semlith prices update` fetches from.
pub const SOURCE_URL: &str = "https://models.dev/api.json";

/// The snapshot this binary carries.
const BUILT_IN: &str = include_str!("prices.json");

/// The providers kept from models.dev: the vendors themselves first, then the
/// gateways the supported clients are pointed at. The order is also the order
/// a bare model id is looked up in, so a model sold both direct and through a
/// reseller is priced at the vendor's own rate.
pub const PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "xai",
    "deepseek",
    "mistral",
    "moonshotai",
    "zai",
    "alibaba",
    "github-copilot",
    "opencode",
    "google-vertex",
    "google-vertex-anthropic",
    "amazon-bedrock",
    "azure",
    "openrouter",
    "groq",
    "cerebras",
    "fireworks-ai",
    "togetherai",
];

/// A downloaded answer larger than this is refused. models.dev's whole API is
/// about 5 MiB today.
const MAX_FETCH: u64 = 64 * 1024 * 1024;

/// Dollars per million tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rates {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// A model's rates, and the higher rates it charges once a request's context
/// is larger than `above` tokens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Price {
    #[serde(flatten)]
    pub rates: Rates,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<Tier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tier {
    pub above: u64,
    #[serde(flatten)]
    pub rates: Rates,
}

/// The table: where it came from, the day it was fetched, and a price per
/// `provider/model`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Table {
    pub source: String,
    /// `YYYY-MM-DD`.
    pub fetched: String,
    pub models: BTreeMap<String, Price>,
}

impl Table {
    /// `models.dev 2026-09-30` — what a priced row's `cost_source` says.
    pub fn label(&self) -> String {
        format!("{} {}", self.source, self.fetched)
    }
}

/// Tokens of one model request, as a client's log records them after the
/// reader has put them in these terms: `input` is the uncached input only,
/// `output` includes any reasoning, `reasoning` is the part of `output` spent
/// thinking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Tokens {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
}

/// The table in use: the downloaded one when it parses and is not older
/// than the snapshot, the snapshot otherwise.
pub fn table() -> Table {
    let built_in = built_in();
    match downloaded() {
        Some(fresh) if fresh.fetched >= built_in.fetched => fresh,
        _ => built_in,
    }
}

/// The snapshot built into this binary.
pub fn built_in() -> Table {
    serde_json::from_str(BUILT_IN).expect("src/prices.json is a price table")
}

fn downloaded() -> Option<Table> {
    let path = crate::home::prices_path().ok()?;
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The price of a model as a client's log names it, with the key it was
/// found under.
///
/// `provider` is what the log says served it, when it says (OpenCode and
/// Codex do); without one the vendors are tried before the gateways. Names
/// are matched loosely in the ways logs differ from models.dev: case, a
/// `[1m]` context suffix, a `models/` prefix, a provider written into the
/// model id, and a date stamp on the end.
pub fn lookup<'a>(
    table: &'a Table,
    provider: Option<&str>,
    model: &str,
) -> Option<(String, &'a Price)> {
    let model = model.trim().to_ascii_lowercase();
    let model = model.split('[').next().unwrap_or("").trim();
    let model = model.strip_prefix("models/").unwrap_or(model);
    let find = |model: &str| -> Option<(String, &'a Price)> {
        let mut keys: Vec<String> = Vec::new();
        if let Some(p) = provider {
            keys.push(format!("{}/{model}", p.to_ascii_lowercase()));
        }
        // `anthropic/claude-sonnet-4.5` is also a provider and a model.
        if let Some((p, rest)) = model.split_once('/') {
            keys.push(format!("{p}/{rest}"));
        }
        keys.extend(PROVIDERS.iter().map(|p| format!("{p}/{model}")));
        for key in keys {
            if let Some(price) = table.models.get(&key) {
                return Some((key, price));
            }
        }
        None
    };
    find(model)
        .or_else(|| {
            let bare = model.rsplit('/').next().unwrap_or(model);
            (bare != model).then(|| find(bare)).flatten()
        })
        .or_else(|| {
            let bare = model.rsplit('/').next().unwrap_or(model);
            let undated = undated(bare)?;
            find(undated)
        })
}

/// `claude-sonnet-4-5-20250929` → `claude-sonnet-4-5`; also `-2025-09-29`.
fn undated(model: &str) -> Option<&str> {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some((head, tail)) = model.rsplit_once('-')
        && tail.len() == 8
        && digits(tail)
    {
        return Some(head);
    }
    let parts: Vec<&str> = model.rsplitn(4, '-').collect();
    if parts.len() == 4
        && parts[0].len() == 2
        && parts[1].len() == 2
        && parts[2].len() == 4
        && parts[..3].iter().all(|p| digits(p))
    {
        return Some(parts[3]);
    }
    None
}

/// Dollars for `tokens` at `price`.
///
/// The tier is chosen by the request's whole context — uncached input plus
/// both cache figures — as the vendors that tier their prices bill it. A cache
/// rate the table does not carry is billed at the input rate.
pub fn cost(price: &Price, tokens: &Tokens) -> f64 {
    let context = (tokens.input + tokens.cache_read + tokens.cache_write).max(0) as u64;
    let rates = price
        .tiers
        .iter()
        .filter(|t| context > t.above)
        .max_by_key(|t| t.above)
        .map_or(price.rates, |t| t.rates);
    let per = |n: i64, rate: f64| n.max(0) as f64 * rate / 1_000_000.0;
    per(tokens.input, rates.input)
        + per(tokens.output, rates.output)
        + per(tokens.cache_read, rates.cache_read.unwrap_or(rates.input))
        + per(tokens.cache_write, rates.cache_write.unwrap_or(rates.input))
}

/// models.dev's API, trimmed to [`PROVIDERS`] and to models with a price.
pub fn trim(api: &serde_json::Value, fetched: &str) -> Result<Table> {
    let mut models = BTreeMap::new();
    let rates = |c: &serde_json::Value| Rates {
        input: c.get("input").and_then(|v| v.as_f64()).unwrap_or(0.0),
        output: c.get("output").and_then(|v| v.as_f64()).unwrap_or(0.0),
        cache_read: c.get("cache_read").and_then(|v| v.as_f64()),
        cache_write: c.get("cache_write").and_then(|v| v.as_f64()),
    };
    for provider in PROVIDERS {
        let Some(listed) = api
            .get(provider)
            .and_then(|p| p.get("models"))
            .and_then(|m| m.as_object())
        else {
            continue;
        };
        for (id, model) in listed {
            let Some(c) = model.get("cost") else { continue };
            let base = rates(c);
            if base.input <= 0.0 && base.output <= 0.0 {
                continue;
            }
            let tiers = c
                .get("tiers")
                .and_then(|t| t.as_array())
                .map(|tiers| {
                    tiers
                        .iter()
                        .filter(|t| {
                            t.pointer("/tier/type").and_then(|v| v.as_str()) == Some("context")
                        })
                        .filter_map(|t| {
                            Some(Tier {
                                above: t.pointer("/tier/size")?.as_u64()?,
                                rates: rates(t),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            models.insert(
                format!("{provider}/{}", id.to_ascii_lowercase()),
                Price { rates: base, tiers },
            );
        }
    }
    if models.is_empty() {
        bail!("{SOURCE_URL} answered, but with no priced models from any provider semlith keeps");
    }
    Ok(Table {
        source: "models.dev".into(),
        fetched: fetched.into(),
        models,
    })
}

/// Fetch models.dev, trim it and write it where [`table`] looks first.
///
/// The one network request this module makes, and only because somebody ran
/// the command. Refused outright on an air-gapped machine, before a connection
/// is opened.
pub fn update() -> Result<Table> {
    if let Some(reason) = crate::upgrade::offline() {
        bail!("{}", reason.replace("Upgrade on a connected machine and copy the binary across.", "Run `semlith prices update` on a connected machine and copy ~/.semlith/prices.json across."));
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .build()
        .new_agent();
    let mut response = agent
        .get(SOURCE_URL)
        .call()
        .with_context(|| format!("fetching {SOURCE_URL}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_FETCH + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {SOURCE_URL}"))?;
    if bytes.len() as u64 > MAX_FETCH {
        bail!(
            "{SOURCE_URL} answered with more than {} MiB; nothing was written",
            MAX_FETCH >> 20
        );
    }
    let api: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("{SOURCE_URL} did not answer JSON"))?;
    let table = trim(&api, &today())?;
    let path = crate::home::prices_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&table)? + "\n")
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(table)
}

/// Today in UTC, `YYYY-MM-DD`, without a date crate: days since the epoch
/// through the civil-from-days algorithm.
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    civil(secs.div_euclid(86_400))
}

fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        let api = serde_json::json!({
            "anthropic": { "models": {
                "claude-opus-5-5": { "cost": { "input": 4, "output": 20, "cache_read": 0.2, "cache_write": 5 } },
                "claude-sonnet-4-5": { "cost": { "input": 3, "output": 15, "cache_read": 0.3, "cache_write": 3.75 } }
            }},
            "openai": { "models": {
                "gpt-5.5": { "cost": { "input": 5, "output": 30, "cache_read": 0.5,
                    "tiers": [ { "input": 10, "output": 45, "cache_read": 1, "tier": { "type": "context", "size": 272000 } } ] } },
                "free-model": { "cost": { "input": 0, "output": 0 } }
            }},
            "openrouter": { "models": {
                "anthropic/claude-sonnet-4-5": { "cost": { "input": 3.3, "output": 16.5 } }
            }},
            "somebody-else": { "models": { "gpt-5.5": { "cost": { "input": 1, "output": 1 } } } }
        });
        trim(&api, "2026-09-30").unwrap()
    }

    #[test]
    fn trimming_keeps_the_kept_providers_and_their_priced_models() {
        let t = table();
        assert_eq!(t.label(), "models.dev 2026-09-30");
        let keys: Vec<&str> = t.models.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "anthropic/claude-opus-5-5",
                "anthropic/claude-sonnet-4-5",
                "openai/gpt-5.5",
                "openrouter/anthropic/claude-sonnet-4-5"
            ]
        );
        assert_eq!(t.models["openai/gpt-5.5"].tiers[0].above, 272_000);
    }

    #[test]
    fn a_model_is_found_the_way_a_log_names_it() {
        let t = table();
        let key = |provider, model| lookup(&t, provider, model).map(|(k, _)| k);
        assert_eq!(
            key(None, "claude-opus-5-5").as_deref(),
            Some("anthropic/claude-opus-5-5")
        );
        assert_eq!(
            key(None, "claude-opus-5-5[1m]").as_deref(),
            Some("anthropic/claude-opus-5-5")
        );
        assert_eq!(
            key(None, "Claude-Sonnet-4-5-20250929").as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(
            key(None, "claude-sonnet-4-5-2025-09-29").as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(
            key(Some("openrouter"), "anthropic/claude-sonnet-4-5").as_deref(),
            Some("openrouter/anthropic/claude-sonnet-4-5")
        );
        // Without a provider, a vendor-prefixed id is the vendor's own price.
        assert_eq!(
            key(None, "anthropic/claude-sonnet-4-5").as_deref(),
            Some("anthropic/claude-sonnet-4-5")
        );
        assert_eq!(
            key(None, "models/gpt-5.5").as_deref(),
            Some("openai/gpt-5.5")
        );
        assert_eq!(key(None, "no-such-model"), None);
    }

    #[test]
    fn cost_is_tokens_times_rates_with_tiers_and_cache() {
        let t = table();
        let opus = &t.models["anthropic/claude-opus-5-5"];
        let tokens = Tokens {
            input: 1_000,
            output: 2_000,
            cache_read: 100_000,
            cache_write: 10_000,
            reasoning: 0,
        };
        // 1k x 4 + 2k x 20 + 100k x 0.2 + 10k x 5, per million.
        let expected = (4_000.0 + 40_000.0 + 20_000.0 + 50_000.0) / 1e6;
        assert!((cost(opus, &tokens) - expected).abs() < 1e-12);

        let gpt = &t.models["openai/gpt-5.5"];
        let small = Tokens {
            input: 100_000,
            output: 1_000,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        };
        assert!((cost(gpt, &small) - (500_000.0 + 30_000.0) / 1e6).abs() < 1e-12);
        let large = Tokens {
            input: 200_000,
            output: 1_000,
            cache_read: 100_000,
            cache_write: 0,
            reasoning: 0,
        };
        let tiered = (200_000.0 * 10.0 + 1_000.0 * 45.0 + 100_000.0 * 1.0) / 1e6;
        assert!((cost(gpt, &large) - tiered).abs() < 1e-12);
        // No cache-write rate listed: billed at the input rate.
        let write = Tokens {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 100_000,
            reasoning: 0,
        };
        assert!((cost(gpt, &write) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn the_built_in_snapshot_parses_and_prices_the_vendors() {
        let t = built_in();
        assert_eq!(t.source, "models.dev");
        for model in ["claude-opus-4-5", "gpt-5", "gemini-2.5-pro"] {
            assert!(
                lookup(&t, None, model).is_some(),
                "{model} is not in the snapshot"
            );
        }
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(20_726), "2026-09-30");
        assert_eq!(civil(11_016), "2000-02-29");
    }
}
