//! Checking a remote worker's attestation before anything is sent to it.
//!
//! The worker hands over two signed tokens (JWTs): one for the CPU's
//! confidential VM (on Google Cloud, Google Cloud Attestation's token over the
//! vTPM's evidence) and one for the GPU (NVIDIA's attestation result). A policy
//! says, for each, which keys may sign it, who must have issued it, which
//! claims it must carry, and in which claim the nonce sits. The nonce must be
//! the binding `remote::binding` computed: the client's fresh nonce hashed
//! with the certificate the worker's TLS session was under. A token that
//! verifies but names another binding is a replay or belongs to another key,
//! and is refused like a forged one.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use serde_json::Value;
use std::collections::BTreeMap;

/// What the policy file says.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// No attestation at all: for a worker on a machine the user controls
    /// themselves (a test, a lab). Said wherever the lane is shown.
    #[serde(default)]
    pub off: bool,
    #[serde(default)]
    pub cpu: Option<Token>,
    #[serde(default)]
    pub gpu: Option<Token>,
}

/// How one token is checked.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Token {
    /// The `iss` the token must carry.
    pub issuer: String,
    /// Where the signing keys are: a JWKS URL, fetched over TLS when the lane
    /// starts.
    #[serde(default)]
    pub jwks_url: Option<String>,
    /// Or the keys themselves, pinned in the policy.
    #[serde(default)]
    pub jwks: Option<Value>,
    /// The claim holding the binding (`eat_nonce` for both Google and NVIDIA);
    /// a string, or a list that must contain it.
    pub nonce_claim: String,
    /// Claims that must equal these values; a dotted name reaches into an
    /// object (`submods.gce.project_id`).
    #[serde(default)]
    pub require: BTreeMap<String, Value>,
    /// The `aud` the token must carry, when the issuer sets one.
    #[serde(default)]
    pub audience: Option<String>,
}

impl Policy {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the attestation policy {}", path.display()))?;
        let policy: Policy = serde_json::from_str(&text)
            .with_context(|| format!("the attestation policy {}", path.display()))?;
        if !policy.off && policy.cpu.is_none() && policy.gpu.is_none() {
            bail!(
                "the attestation policy {} checks nothing; say \"off\": true to run without attestation",
                path.display()
            );
        }
        Ok(policy)
    }
}

/// What a passed check found, for the lane's row.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Verified {
    /// `attested` or `off`.
    pub state: &'static str,
    /// One line: what was checked and what it said.
    pub summary: String,
}

/// Check the worker's evidence against `policy` and `binding`.
pub fn verify(policy: &Policy, cpu: &str, gpu: &str, binding: &str, now: u64) -> Result<Verified> {
    if policy.off {
        return Ok(Verified {
            state: "off",
            summary: "not attested: the policy turns attestation off".into(),
        });
    }
    let mut said = Vec::new();
    for (name, rule, token) in [("CPU", &policy.cpu, cpu), ("GPU", &policy.gpu, gpu)] {
        let Some(rule) = rule else { continue };
        if token.is_empty() {
            bail!("the worker sent no {name} evidence, and the policy requires it");
        }
        let claims =
            check(rule, token, binding, now).with_context(|| format!("the {name} evidence"))?;
        let shown: Vec<String> = rule
            .require
            .keys()
            .map(|key| format!("{key}={}", plain(claim(&claims, key))))
            .collect();
        said.push(format!("{name}: {} ({})", rule.issuer, shown.join(", ")));
    }
    Ok(Verified {
        state: "attested",
        summary: said.join("; "),
    })
}

fn plain(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "absent".into(),
    }
}

fn claim<'a>(claims: &'a Value, dotted: &str) -> Option<&'a Value> {
    // A claim's own name may hold dots or dashes (NVIDIA's
    // `x-nvidia-overall-att-result`): the whole name first, then a path.
    if let Some(value) = claims.get(dotted) {
        return Some(value);
    }
    dotted.split('.').try_fold(claims, |at, part| at.get(part))
}

/// One token: signature, issuer, time, audience, required claims, binding.
/// Returns its claims.
fn check(rule: &Token, token: &str, binding: &str, now: u64) -> Result<Value> {
    let parts: Vec<&str> = token.trim().split('.').collect();
    let [head, body, signature] = parts.as_slice() else {
        bail!("not a signed token (expected three dot-separated parts)");
    };
    let header: Value = serde_json::from_slice(&b64(head)?).context("the token's header")?;
    let claims: Value = serde_json::from_slice(&b64(body)?).context("the token's claims")?;
    let signature = b64(signature)?;
    let signed = format!("{head}.{body}");
    let alg = header["alg"].as_str().unwrap_or_default();
    let kid = header["kid"].as_str();
    let keys = keys(rule)?;
    let candidates: Vec<&Value> = keys
        .iter()
        .filter(|key| kid.is_none() || key["kid"].as_str() == kid)
        .collect();
    if candidates.is_empty() {
        bail!(
            "no key {} in the issuer's key set",
            kid.unwrap_or("(the token names none)")
        );
    }
    let good = candidates
        .iter()
        .any(|key| signature_ok(alg, key, signed.as_bytes(), &signature));
    if !good {
        bail!("the signature does not verify against the issuer's keys ({alg})");
    }
    if claims["iss"].as_str() != Some(rule.issuer.as_str()) {
        bail!(
            "issued by {}, not {}",
            plain(claims.get("iss")),
            rule.issuer
        );
    }
    let leeway = 60;
    if let Some(exp) = claims["exp"].as_u64()
        && now > exp + leeway
    {
        bail!("expired {} s ago", now - exp);
    }
    if let Some(nbf) = claims["nbf"].as_u64()
        && now + leeway < nbf
    {
        bail!("not valid for another {} s", nbf - now);
    }
    if let Some(audience) = &rule.audience {
        let ok = match &claims["aud"] {
            Value::String(aud) => aud == audience,
            Value::Array(list) => list.iter().any(|aud| aud.as_str() == Some(audience)),
            _ => false,
        };
        if !ok {
            bail!("not for the audience {audience}");
        }
    }
    for (key, want) in &rule.require {
        let got = claim(&claims, key);
        if got != Some(want) {
            bail!(
                "{key} is {}, the policy requires {}",
                plain(got),
                plain(Some(want))
            );
        }
    }
    let bound = match claim(&claims, &rule.nonce_claim) {
        Some(Value::String(nonce)) => nonce.eq_ignore_ascii_case(binding),
        Some(Value::Array(list)) => list.iter().any(|nonce| {
            nonce
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(binding))
        }),
        _ => false,
    };
    if !bound {
        bail!(
            "its {} is not this connection's binding: a replayed report, or one made for another key",
            rule.nonce_claim
        );
    }
    Ok(claims)
}

fn b64(part: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part.trim_end_matches('='))
        .context("a token part that is not base64url")
}

/// The issuer's keys: pinned in the policy, or fetched from its URL once per
/// process.
fn keys(rule: &Token) -> Result<Vec<Value>> {
    let set = match (&rule.jwks, &rule.jwks_url) {
        (Some(set), _) => set.clone(),
        (None, Some(url)) => fetch_jwks(url)?,
        (None, None) => bail!("the policy names no keys for {}", rule.issuer),
    };
    Ok(set["keys"].as_array().cloned().unwrap_or_default())
}

fn fetch_jwks(url: &str) -> Result<Value> {
    static FETCHED: std::sync::Mutex<BTreeMap<String, Value>> =
        std::sync::Mutex::new(BTreeMap::new());
    if let Some(set) = FETCHED.lock().unwrap_or_else(|e| e.into_inner()).get(url) {
        return Ok(set.clone());
    }
    if !url.starts_with("https://") {
        bail!("the key set {url} is not served over https");
    }
    let mut response = ureq::get(url)
        // NVIDIA's service refuses a request with no agent named.
        .header("User-Agent", concat!("semlith/", env!("CARGO_PKG_VERSION")))
        .call()
        .with_context(|| format!("fetching the key set {url}"))?;
    let mut set: Value = response
        .body_mut()
        .read_json()
        .with_context(|| format!("the key set {url}"))?;
    // An OpenID configuration names its key set rather than holding it.
    if set.get("keys").is_none()
        && let Some(next) = set["jwks_uri"].as_str()
    {
        let next = next.to_string();
        set = ureq::get(&next)
            .header("User-Agent", concat!("semlith/", env!("CARGO_PKG_VERSION")))
            .call()
            .with_context(|| format!("fetching the key set {next}"))?
            .body_mut()
            .read_json()
            .with_context(|| format!("the key set {next}"))?;
    }
    FETCHED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(url.to_string(), set.clone());
    Ok(set)
}

fn signature_ok(alg: &str, key: &Value, message: &[u8], signature: &[u8]) -> bool {
    use ring::signature as sig;
    let field = |name: &str| key[name].as_str().and_then(|v| b64(v).ok());
    match (alg, key["kty"].as_str()) {
        ("RS256", Some("RSA")) => {
            let (Some(n), Some(e)) = (field("n"), field("e")) else {
                return false;
            };
            sig::RsaPublicKeyComponents { n, e }
                .verify(&sig::RSA_PKCS1_2048_8192_SHA256, message, signature)
                .is_ok()
        }
        ("ES384" | "ES256", Some("EC")) => {
            let (Some(x), Some(y)) = (field("x"), field("y")) else {
                return false;
            };
            let algorithm: &dyn sig::VerificationAlgorithm = if alg == "ES384" {
                &sig::ECDSA_P384_SHA384_FIXED
            } else {
                &sig::ECDSA_P256_SHA256_FIXED
            };
            let mut point = vec![4u8];
            point.extend(x);
            point.extend(y);
            sig::UnparsedPublicKey::new(algorithm, point)
                .verify(message, signature)
                .is_ok()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dotted_claim_reaches_into_objects_and_whole_names_win() {
        let claims = serde_json::json!({
            "submods": { "gce": { "project_id": "p" } },
            "x-nvidia-overall-att-result": true,
        });
        assert_eq!(
            claim(&claims, "submods.gce.project_id"),
            Some(&Value::from("p"))
        );
        assert_eq!(
            claim(&claims, "x-nvidia-overall-att-result"),
            Some(&Value::from(true))
        );
        assert_eq!(claim(&claims, "submods.gce.zone"), None);
    }

    #[test]
    fn a_policy_that_checks_nothing_is_refused() {
        let dir = std::env::temp_dir().join(format!("semlith-attest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("policy.json");
        std::fs::write(&path, "{}").unwrap();
        assert!(Policy::load(&path).is_err());
        std::fs::write(&path, r#"{"off": true}"#).unwrap();
        assert!(Policy::load(&path).unwrap().off);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
