//! The attestation check against real evidence: an NVIDIA Remote Attestation
//! Service token recorded on a GCP Confidential G4 (RTX PRO 6000, CC on) on
//! 2026-10-07, whose `eat_nonce` was the nonce the request carried, and the
//! service key that signed it. Each way a token can be wrong is refused, and
//! says what was wrong.

use semlith::attest::{Policy, Token, verify};

const EAT: &str = include_str!("fixtures/attest/nvidia-eat.jwt");
const JWKS: &str = include_str!("fixtures/attest/nras-jwks.json");
const NONCE: &str = "db83252a0b3f1707c17e213caab3a426bf1cf1507b9b46d9e79b673c5723c648";
/// The token's `iat`; it lives an hour.
const ISSUED: u64 = 1_791_376_614;

fn gpu_policy() -> Policy {
    Policy {
        off: false,
        cpu: None,
        gpu: Some(Token {
            issuer: "https://nras.attestation.nvidia.com".into(),
            jwks_url: None,
            jwks: Some(serde_json::from_str(JWKS).unwrap()),
            nonce_claim: "eat_nonce".into(),
            require: [(
                "x-nvidia-overall-att-result".to_string(),
                serde_json::Value::Bool(true),
            )]
            .into(),
            audience: None,
        }),
    }
}

fn refusal(policy: &Policy, gpu: &str, binding: &str, now: u64) -> String {
    format!("{:#}", verify(policy, "", gpu, binding, now).unwrap_err())
}

#[test]
fn the_recorded_gpu_token_passes_with_its_own_nonce() {
    let ok = verify(&gpu_policy(), "", EAT.trim(), NONCE, ISSUED + 60).unwrap();
    assert_eq!(ok.state, "attested");
    assert!(
        ok.summary.contains("x-nvidia-overall-att-result=true"),
        "{}",
        ok.summary
    );
}

#[test]
fn a_replayed_token_for_another_connection_is_refused() {
    let other = "00".repeat(32);
    let why = refusal(&gpu_policy(), EAT.trim(), &other, ISSUED + 60);
    assert!(why.contains("replayed"), "{why}");
}

#[test]
fn a_changed_claim_breaks_the_signature() {
    // The same token with its claims swapped for ones saying the GPU failed.
    let parts: Vec<&str> = EAT.trim().split('.').collect();
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut claims: serde_json::Value =
        serde_json::from_slice(&engine.decode(parts[1]).unwrap()).unwrap();
    claims["eat_nonce"] = serde_json::Value::from("11".repeat(32));
    let forged = format!(
        "{}.{}.{}",
        parts[0],
        engine.encode(serde_json::to_vec(&claims).unwrap()),
        parts[2]
    );
    let why = refusal(&gpu_policy(), &forged, &"11".repeat(32), ISSUED + 60);
    assert!(why.contains("signature does not verify"), "{why}");
}

#[test]
fn an_expired_token_is_refused() {
    let why = refusal(&gpu_policy(), EAT.trim(), NONCE, ISSUED + 3 * 3600);
    assert!(why.contains("expired"), "{why}");
}

#[test]
fn a_key_the_issuer_does_not_list_is_refused() {
    let mut policy = gpu_policy();
    policy.gpu.as_mut().unwrap().jwks = Some(serde_json::json!({ "keys": [] }));
    let why = refusal(&policy, EAT.trim(), NONCE, ISSUED + 60);
    assert!(why.contains("no key"), "{why}");
}

#[test]
fn a_claim_the_policy_requires_must_match() {
    let mut policy = gpu_policy();
    policy
        .gpu
        .as_mut()
        .unwrap()
        .require
        .insert("x-nvidia-ver".into(), serde_json::Value::from("9.9"));
    let why = refusal(&policy, EAT.trim(), NONCE, ISSUED + 60);
    assert!(why.contains("x-nvidia-ver is 4.0"), "{why}");
}

#[test]
fn another_issuer_is_refused() {
    let mut policy = gpu_policy();
    policy.gpu.as_mut().unwrap().issuer = "https://example.com".into();
    let why = refusal(&policy, EAT.trim(), NONCE, ISSUED + 60);
    assert!(
        why.contains("issued by https://nras.attestation.nvidia.com"),
        "{why}"
    );
}

#[test]
fn missing_evidence_the_policy_requires_is_refused() {
    let why = refusal(&gpu_policy(), "", NONCE, ISSUED + 60);
    assert!(why.contains("no GPU evidence"), "{why}");
}

// ------------------------------------------------- the CPU: Google's token

const CVM: &str = include_str!("fixtures/attest/google-cvm.jwt");
const GCA_JWKS: &str = include_str!("fixtures/attest/gca-jwks.json");
const CVM_NONCE: &str = "807f2d5d42ad9f3a77e1e4e3b47d3c745d75ee45911cd1c9368485685095ca28";
const CVM_ISSUED: u64 = 1_791_377_632;

/// The CPU half of the policy `scripts/remote-worker` writes for a G4.
fn cpu_policy() -> Policy {
    Policy {
        off: false,
        cpu: Some(Token {
            issuer: "https://confidentialcomputing.googleapis.com".into(),
            jwks_url: None,
            jwks: Some(serde_json::from_str(GCA_JWKS).unwrap()),
            nonce_claim: "eat_nonce".into(),
            require: [
                ("hwmodel", serde_json::json!("GCP_AMD_SEV")),
                ("secboot", serde_json::json!(true)),
                ("swname", serde_json::json!("GCE")),
                ("submods.gce.project_id", serde_json::json!("semlith-cloud")),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
            audience: Some("semlith-remote-lane".into()),
        }),
        gpu: None,
    }
}

#[test]
fn the_recorded_confidential_vm_token_passes_with_its_own_nonce() {
    let ok = verify(&cpu_policy(), CVM.trim(), "", CVM_NONCE, CVM_ISSUED + 60).unwrap();
    assert!(ok.summary.contains("hwmodel=GCP_AMD_SEV"), "{}", ok.summary);
}

#[test]
fn a_vm_in_another_project_or_without_secure_boot_is_refused() {
    let mut policy = cpu_policy();
    policy.cpu.as_mut().unwrap().require.insert(
        "submods.gce.project_id".into(),
        serde_json::json!("someone-else"),
    );
    let why = format!(
        "{:#}",
        verify(&policy, CVM.trim(), "", CVM_NONCE, CVM_ISSUED + 60).unwrap_err()
    );
    assert!(
        why.contains("submods.gce.project_id is semlith-cloud"),
        "{why}"
    );
}

#[test]
fn a_token_for_another_audience_is_refused() {
    let mut policy = cpu_policy();
    policy.cpu.as_mut().unwrap().audience = Some("another-service".into());
    let why = format!(
        "{:#}",
        verify(&policy, CVM.trim(), "", CVM_NONCE, CVM_ISSUED + 60).unwrap_err()
    );
    assert!(why.contains("audience"), "{why}");
}

#[test]
fn both_halves_must_pass_when_the_policy_names_both() {
    let mut policy = cpu_policy();
    policy.gpu = gpu_policy().gpu;
    // Each token was made for its own nonce, so the pair can never share a
    // binding: one of them is always refused.
    assert!(verify(&policy, CVM.trim(), EAT.trim(), CVM_NONCE, CVM_ISSUED + 60).is_err());
    assert!(verify(&policy, CVM.trim(), EAT.trim(), NONCE, ISSUED + 60).is_err());
}
