use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ring::signature::KeyPair as _;
use serde_json::{Value, json};

use super::catalog_patch::apply_model_patches;
use super::keys::{KeyStatus, TrustedKey};
use super::overlay;
use super::provenance::{CloudFactsState, CloudFactsStatus, FactsOrigin};
use super::scope::{base_url_allowed, scoped_view};
use super::types::ModelOp;
use super::verify::{FactsRejection, parse_rfc3339_utc, signing_message, verify_envelope};
use crate::catalog::{CatalogCompiler, CatalogOffering, CatalogSource};

/// Cross-language fixture signed by `web/scripts/facts-publish.mjs` with the
/// TEST-ONLY key (`docs/cloud-facts/fixtures/test-only-signing-key.pem`).
const FIXTURE_V7: &str =
    include_str!("../../../../docs/cloud-facts/fixtures/envelope-stable-v7.json");
const FIXTURE_FUTURE_V8: &str =
    include_str!("../../../../docs/cloud-facts/fixtures/envelope-future-only-v8.json");
const TEST_ONLY_PUB_B64: &str = "8+FLDW4OorUETUVks0hpQAi5Lj4wg3kjKjfYFzLbJ7U=";
const NOW: u64 = 1_790_000_000; // 2026-09-21T...Z

fn test_only_key(status: KeyStatus) -> TrustedKey {
    let raw = BASE64.decode(TEST_ONLY_PUB_B64).expect("pub b64");
    let mut public_key = [0u8; 32];
    public_key.copy_from_slice(&raw);
    TrustedKey {
        key_id: "cwf-test-only",
        public_key,
        status,
    }
}

fn v(s: &str) -> semver::Version {
    semver::Version::parse(s).expect("semver")
}

/// Ephemeral signer for in-Rust variants (tamper, rotation, schema bumps).
struct Signer {
    key_id: &'static str,
    pair: ring::signature::Ed25519KeyPair,
}

impl Signer {
    fn new(key_id: &'static str) -> Self {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).expect("pkcs8");
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("pair");
        Self { key_id, pair }
    }

    fn trusted(&self, status: KeyStatus) -> TrustedKey {
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(self.pair.public_key().as_ref());
        TrustedKey {
            key_id: self.key_id,
            public_key,
            status,
        }
    }

    fn sign(&self, payload: &[u8]) -> String {
        BASE64.encode(self.pair.sign(&signing_message(self.key_id, payload)))
    }

    fn envelope(&self, payload: &Value) -> Value {
        let bytes = serde_json::to_vec(payload).expect("payload json");
        json!({
            "envelope": 1,
            "channel": payload["channel"],
            "facts_version": payload["facts_version"],
            "schema_version": payload["schema_version"],
            "key_id": self.key_id,
            "alg": "ed25519",
            "applies_to": payload["applies_to"],
            "payload_b64": BASE64.encode(&bytes),
            "sig_b64": self.sign(&bytes),
            "sigs": [],
        })
    }
}

fn payload(channel: &str, version: u64, applies_to: &str) -> Value {
    json!({
        "schema_version": 1,
        "channel": channel,
        "facts_version": version,
        "published_at": "2026-08-30T00:00:00Z",
        "applies_to": applies_to,
        "models": [],
        "provider_defaults": {},
        "release": null,
        "announcements": [],
    })
}

fn verify(bytes: &[u8], keys: &[TrustedKey]) -> Result<super::VerifiedFacts, FactsRejection> {
    verify_envelope(bytes, "stable", &v("0.9.11"), None, keys, NOW)
}

#[test]
fn node_signed_fixture_verifies_under_the_test_only_key() {
    let keys = [test_only_key(KeyStatus::Active)];
    let verified = verify(FIXTURE_V7.as_bytes(), &keys).expect("fixture verifies");
    assert_eq!(verified.key_id, "cwf-test-only");
    assert_eq!(verified.facts.facts_version, 7);
    assert_eq!(verified.facts.channel, "stable");
    assert_eq!(verified.facts.models.len(), 6);
    assert!(!verified.stale);
    // sha256 in the envelope matches the recomputed digest of the signed bytes.
    let env: Value = serde_json::from_str(FIXTURE_V7).expect("json");
    assert_eq!(env["sha256"].as_str(), Some(verified.sha256.as_str()));
}

#[test]
fn fixture_is_rejected_with_no_trusted_keys() {
    let err = verify(FIXTURE_V7.as_bytes(), &[]).expect_err("no keys");
    assert!(matches!(err, FactsRejection::UnknownKey { .. }), "{err:?}");
}

#[test]
fn retired_key_is_rejected_distinctly() {
    let keys = [test_only_key(KeyStatus::Retired)];
    let err = verify(FIXTURE_V7.as_bytes(), &keys).expect_err("retired");
    assert_eq!(
        err,
        FactsRejection::RetiredKey {
            key_id: "cwf-test-only".into()
        }
    );
}

#[test]
fn flipping_payload_signature_or_key_id_bytes_breaks_verification() {
    let keys = [test_only_key(KeyStatus::Active)];
    let mut env: Value = serde_json::from_str(FIXTURE_V7).expect("json");

    // Payload tamper (decode → flip → encode keeps the envelope well-formed).
    let mut bytes = BASE64.decode(env["payload_b64"].as_str().unwrap()).unwrap();
    bytes[10] ^= 0x01;
    let mut tampered = env.clone();
    tampered["payload_b64"] = Value::String(BASE64.encode(&bytes));
    let err = verify(&serde_json::to_vec(&tampered).unwrap(), &keys).unwrap_err();
    assert_eq!(err, FactsRejection::BadSignature);

    // Signature tamper.
    let mut sig = BASE64.decode(env["sig_b64"].as_str().unwrap()).unwrap();
    sig[3] ^= 0x80;
    let mut tampered = env.clone();
    tampered["sig_b64"] = Value::String(BASE64.encode(&sig));
    let err = verify(&serde_json::to_vec(&tampered).unwrap(), &keys).unwrap_err();
    assert_eq!(err, FactsRejection::BadSignature);

    // Re-labelling under another pinned key id must fail: key_id is signed.
    let other = TrustedKey {
        key_id: "cwf-other",
        ..test_only_key(KeyStatus::Active)
    };
    env["key_id"] = Value::String("cwf-other".into());
    let err = verify(&serde_json::to_vec(&env).unwrap(), &[other]).unwrap_err();
    assert_eq!(err, FactsRejection::BadSignature);
}

#[test]
fn envelope_shape_errors_come_before_crypto() {
    let keys = [test_only_key(KeyStatus::Active)];
    let base: Value = serde_json::from_str(FIXTURE_V7).expect("json");

    let mut e = base.clone();
    e["alg"] = json!("rsa");
    assert!(matches!(
        verify(&serde_json::to_vec(&e).unwrap(), &keys).unwrap_err(),
        FactsRejection::BadEnvelope(_)
    ));

    let mut e = base.clone();
    e["envelope"] = json!(2);
    assert!(matches!(
        verify(&serde_json::to_vec(&e).unwrap(), &keys).unwrap_err(),
        FactsRejection::BadEnvelope(_)
    ));

    let big = vec![b' '; super::MAX_ENVELOPE_BYTES + 1];
    assert!(matches!(
        verify(&big, &keys).unwrap_err(),
        FactsRejection::TooLarge { .. }
    ));

    // Oversized payload_b64 is refused before base64 decode / crypto.
    let mut e = base.clone();
    e["payload_b64"] = Value::String("A".repeat(super::MAX_PAYLOAD_BYTES * 4 / 3 + 64));
    assert!(matches!(
        verify(&serde_json::to_vec(&e).unwrap(), &keys).unwrap_err(),
        FactsRejection::TooLarge { .. }
    ));
}

#[test]
fn outer_inner_mismatch_wrong_channel_and_schema_too_new_are_rejected() {
    let signer = Signer::new("cwf-unit");
    let keys = [signer.trusted(KeyStatus::Active)];

    let mut env = signer.envelope(&payload("stable", 3, "*"));
    env["facts_version"] = json!(4);
    let err = verify(&serde_json::to_vec(&env).unwrap(), &keys).unwrap_err();
    assert!(matches!(err, FactsRejection::Mismatch(_)), "{err:?}");

    let env = signer.envelope(&payload("beta", 3, "*"));
    let err = verify(&serde_json::to_vec(&env).unwrap(), &keys).unwrap_err();
    assert_eq!(
        err,
        FactsRejection::WrongChannel {
            expected: "stable".into(),
            got: "beta".into()
        }
    );

    let mut p = payload("stable", 3, "*");
    p["schema_version"] = json!(2);
    let env = signer.envelope(&p);
    let err = verify(&serde_json::to_vec(&env).unwrap(), &keys).unwrap_err();
    assert_eq!(err, FactsRejection::SchemaTooNew { schema_version: 2 });
}

#[test]
fn applies_to_scopes_the_whole_payload_by_binary_version() {
    let keys = [test_only_key(KeyStatus::Active)];
    let err = verify(FIXTURE_FUTURE_V8.as_bytes(), &keys).unwrap_err();
    assert_eq!(
        err,
        FactsRejection::NotApplicable {
            applies_to: ">=99.0.0".into()
        }
    );
    // Same envelope is accepted by a binary inside the range.
    let ok = verify_envelope(
        FIXTURE_FUTURE_V8.as_bytes(),
        "stable",
        &v("99.1.0"),
        None,
        &keys,
        NOW,
    );
    assert!(ok.is_ok(), "{ok:?}");

    let signer = Signer::new("cwf-unit");
    let keys = [signer.trusted(KeyStatus::Active)];
    let env = signer.envelope(&payload("stable", 1, "not a range"));
    let err = verify(&serde_json::to_vec(&env).unwrap(), &keys).unwrap_err();
    assert!(matches!(err, FactsRejection::BadVersionReq(_)));

    // Prerelease binaries follow semver: a plain range does not match them.
    let env = signer.envelope(&payload("stable", 1, ">=0.9.0, <1.0.0"));
    let bytes = serde_json::to_vec(&env).unwrap();
    assert!(verify_envelope(&bytes, "stable", &v("0.9.12-beta.1"), None, &keys, NOW).is_err());
    assert!(verify_envelope(&bytes, "stable", &v("0.9.12"), None, &keys, NOW).is_ok());
}

#[test]
fn rollback_protection_accepts_equal_and_higher_only() {
    let signer = Signer::new("cwf-unit");
    let keys = [signer.trusted(KeyStatus::Active)];
    let env = serde_json::to_vec(&signer.envelope(&payload("stable", 5, "*"))).unwrap();
    assert!(verify_envelope(&env, "stable", &v("0.9.11"), Some(4), &keys, NOW).is_ok());
    assert!(verify_envelope(&env, "stable", &v("0.9.11"), Some(5), &keys, NOW).is_ok());
    let err = verify_envelope(&env, "stable", &v("0.9.11"), Some(6), &keys, NOW).unwrap_err();
    assert_eq!(
        err,
        FactsRejection::Rollback {
            got: 5,
            highest_seen: 6
        }
    );
}

#[test]
fn not_after_only_downgrades_to_stale_after_grace() {
    let signer = Signer::new("cwf-unit");
    let keys = [signer.trusted(KeyStatus::Active)];
    let mut p = payload("stable", 5, "*");
    p["not_after"] = json!("2026-09-01T00:00:00Z");
    let env = serde_json::to_vec(&signer.envelope(&p)).unwrap();
    let not_after = parse_rfc3339_utc("2026-09-01T00:00:00Z").unwrap();
    let fresh = verify_envelope(&env, "stable", &v("0.9.11"), None, &keys, not_after - 1).unwrap();
    assert!(!fresh.stale);
    let in_grace =
        verify_envelope(&env, "stable", &v("0.9.11"), None, &keys, not_after + 3600).unwrap();
    assert!(!in_grace.stale);
    let stale = verify_envelope(
        &env,
        "stable",
        &v("0.9.11"),
        None,
        &keys,
        not_after + super::verify::NOT_AFTER_GRACE_SECS + 1,
    )
    .unwrap();
    assert!(stale.stale, "past grace must be stale, not rejected");
}

#[test]
fn rotation_accepts_any_pinned_active_signature() {
    let old = Signer::new("cwf-old");
    let new = Signer::new("cwf-new");
    let p = payload("stable", 9, "*");
    let bytes = serde_json::to_vec(&p).unwrap();
    // Primary signature by the old key, extra by the new key.
    let env = json!({
        "envelope": 1, "channel": "stable", "facts_version": 9, "key_id": "cwf-old",
        "alg": "ed25519", "payload_b64": BASE64.encode(&bytes), "sig_b64": old.sign(&bytes),
        "sigs": [{ "key_id": "cwf-new", "sig_b64": new.sign(&bytes) }],
    });
    let bytes = serde_json::to_vec(&env).unwrap();
    // A client that only pins the new key still accepts it.
    let ok = verify(&bytes, &[new.trusted(KeyStatus::Active)]).unwrap();
    assert_eq!(ok.key_id, "cwf-new");
    // A client that retired the old key and pins the new one accepts it too.
    let ok = verify(
        &bytes,
        &[
            old.trusted(KeyStatus::Retired),
            new.trusted(KeyStatus::Active),
        ],
    )
    .unwrap();
    assert_eq!(ok.key_id, "cwf-new");
    // A client pinning only the old key (still active) accepts via primary.
    let ok = verify(&bytes, &[old.trusted(KeyStatus::Active)]).unwrap();
    assert_eq!(ok.key_id, "cwf-old");
}

#[test]
fn unknown_payload_fields_are_preserved_not_acted_on() {
    let signer = Signer::new("cwf-unit");
    let keys = [signer.trusted(KeyStatus::Active)];
    let mut p = payload("stable", 5, "*");
    p["policy"] = json!({ "deny": ["everything"] });
    let env = serde_json::to_vec(&signer.envelope(&p)).unwrap();
    let ok = verify(&env, &keys).unwrap();
    assert!(ok.facts.unknown.contains_key("policy"));
    let scoped = scoped_view(&ok, &v("0.9.11"), NOW);
    assert!(scoped.models.is_empty() && scoped.provider_defaults.is_empty());
}

#[test]
fn scoped_view_filters_items_and_enforces_the_base_url_allowlist() {
    let keys = [test_only_key(KeyStatus::Active)];
    let verified = verify(FIXTURE_V7.as_bytes(), &keys).unwrap();
    let scoped = scoped_view(&verified, &v("0.9.11"), NOW);

    let ids: Vec<&str> = scoped.models.iter().map(|m| m.id.as_str()).collect();
    assert!(
        !ids.contains(&"future-only"),
        "per-item applies_to must filter: {ids:?}"
    );
    assert_eq!(scoped.models.len(), 5);

    assert_eq!(
        scoped.provider_defaults["deepseek"]
            .default_model
            .as_deref(),
        Some("deepseek-v4-pro")
    );
    assert_eq!(
        scoped.provider_defaults["deepseek"].base_url.as_deref(),
        Some("https://api.deepseek.com/beta")
    );
    assert!(
        !scoped.provider_defaults.contains_key("openai"),
        "off-family base_url must be dropped entirely"
    );
    assert!(
        !scoped.provider_defaults.contains_key("ollama"),
        "local providers accept no cloud base_url"
    );
    assert!(scoped.dropped.iter().any(|d| d.contains("evil.example")));

    let announcements: Vec<&str> = scoped.announcements.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(announcements, vec!["fixture-live"]);
    assert_eq!(scoped.release.as_ref().unwrap().yanked, vec!["0.9.10"]);
}

#[test]
fn base_url_allowlist_is_https_and_official_host_family_only() {
    assert!(base_url_allowed(
        "deepseek",
        "https://api.deepseek.com/beta"
    ));
    assert!(base_url_allowed(
        "deepseek",
        "https://eu.api.deepseek.com/v1"
    ));
    assert!(!base_url_allowed(
        "deepseek",
        "http://api.deepseek.com/beta"
    ));
    assert!(!base_url_allowed(
        "deepseek",
        "https://api.deepseek.com.evil.example/"
    ));
    assert!(!base_url_allowed(
        "deepseek",
        "https://user@api.deepseek.com/"
    ));
    assert!(!base_url_allowed("openai", "https://evil.example/v1"));
    assert!(!base_url_allowed("ollama", "https://localhost:11434/v1"));
    assert!(!base_url_allowed("no-such-provider", "https://x.example/"));
}

#[test]
fn catalog_layer_15_patches_sit_between_models_dev_and_provider_live() {
    let keys = [test_only_key(KeyStatus::Active)];
    let verified = verify(FIXTURE_V7.as_bytes(), &keys).unwrap();
    let scoped = scoped_view(&verified, &v("0.9.11"), NOW);

    let row = |id: &str, source: CatalogSource| CatalogOffering {
        provider: "deepseek".into(),
        wire_model_id: id.into(),
        endpoint_key: "chat".into(),
        limit: Some(crate::models_dev::ModelsDevLimit {
            context: Some(1000),
            input: None,
            output: Some(10),
        }),
        reasoning: Some(false),
        source,
        ..CatalogOffering::default()
    };
    let snapshot = CatalogCompiler::new()
        .with_bundled(vec![
            row("deepseek-v4-pro", CatalogSource::Bundled),
            row("deepseek-chat", CatalogSource::Bundled),
            row("deepseek-reasoner", CatalogSource::Bundled),
        ])
        .with_cloud_facts(&scoped, NOW)
        .with_provider_live(vec![row(
            "deepseek-reasoner",
            CatalogSource::Live {
                base_url_fingerprint: "fp".into(),
                fetched_at: NOW,
            },
        )])
        .compile();

    let find = |id: &str| {
        snapshot
            .offerings
            .iter()
            .find(|o| o.wire_model_id == id)
            .cloned()
    };
    // Upsert patched only the fields it set; untouched fields survive.
    let pro = find("deepseek-v4-pro").expect("patched row");
    assert_eq!(pro.limit.as_ref().unwrap().context, Some(262_144));
    assert_eq!(pro.limit.as_ref().unwrap().output, Some(32_768));
    assert_eq!(pro.reasoning, Some(false), "unpatched field must survive");
    assert!(matches!(
        pro.source,
        CatalogSource::CloudFacts {
            facts_version: 7,
            ..
        }
    ));
    // New row materialized because it carried a context window.
    let new_row = find("fixture-new-model").expect("new row");
    assert_eq!(new_row.reasoning, Some(true));
    // Patch without context for a missing row is skipped.
    assert!(find("fixture-needs-context").is_none());
    // Deprecate annotates, never removes.
    let chat = find("deepseek-chat").expect("deprecated row stays");
    assert!(
        chat.reasoning_options
            .iter()
            .any(|v| v["cloud_facts"]["op"] == "deprecated")
    );
    // Hide removed the bundled row, but the provider-live row above layer 15
    // re-adds it: cloud can never hide a gateway's own live row.
    let reasoner = find("deepseek-reasoner").expect("provider-live row wins");
    assert!(matches!(reasoner.source, CatalogSource::Live { .. }));

    // Direct map application: hide on a higher-layer row is a receipt, not a removal.
    let mut rows: BTreeMap<(String, String), CatalogOffering> = BTreeMap::new();
    rows.insert(
        ("deepseek".into(), "deepseek-reasoner".into()),
        row("deepseek-reasoner", CatalogSource::UserOverride),
    );
    let skipped = apply_model_patches(&mut rows, &scoped, NOW);
    assert!(rows.contains_key(&("deepseek".into(), "deepseek-reasoner".into())));
    assert!(skipped.iter().any(|s| s.id == "deepseek-reasoner"));
    assert_eq!(scoped.models[0].op, ModelOp::Upsert);
}

#[test]
fn overlay_supplies_provider_defaults_only_from_the_scoped_view() {
    let keys = [test_only_key(KeyStatus::Active)];
    let verified = verify(FIXTURE_V7.as_bytes(), &keys).unwrap();
    let scoped = scoped_view(&verified, &v("0.9.11"), NOW);

    overlay::clear();
    assert!(overlay::cloud_default_model("deepseek").is_none());
    overlay::set_overlay(Some(scoped));
    let (model, source) = overlay::cloud_default_model("deepseek").expect("cloud default");
    assert_eq!(model, "deepseek-v4-pro");
    assert_eq!(
        source,
        overlay::DefaultSource::CloudFacts { facts_version: 7 }
    );
    assert!(overlay::cloud_default_base_url("openai").is_none());
    overlay::clear();
    assert!(overlay::cloud_default_model("deepseek").is_none());
}

#[test]
fn status_labels_cover_every_state() {
    let now = NOW;
    let label = |state: CloudFactsState| {
        CloudFactsStatus {
            state,
            ..CloudFactsStatus::default()
        }
        .label(now)
    };
    assert_eq!(label(CloudFactsState::Off), "off (bundled)");
    assert!(label(CloudFactsState::Inert).contains("inert"));
    assert!(label(CloudFactsState::BundledOnly).contains("bundled"));
    let verified = label(CloudFactsState::Verified {
        channel: "stable".into(),
        facts_version: 42,
        key_id: "cwf-2026-08".into(),
        fetched_at: now - 12 * 60,
        origin: FactsOrigin::Network,
        stale: false,
        patches: 3,
        defaults: 1,
        announcements: 0,
    });
    assert_eq!(
        verified,
        "stable v42 · verified cwf-2026-08 · fetched 12m ago (network) · 3 patches, 1 default, 0 notices"
    );
    assert!(
        label(CloudFactsState::Rejected {
            reason: "bad signature".into(),
            at: now
        })
        .contains("bundled in use")
    );
    assert!(
        label(CloudFactsState::NotApplicable {
            applies_to: ">=1.0".into()
        })
        .contains(">=1.0")
    );
    assert!(
        label(CloudFactsState::Failed {
            last_error: "HTTP 503".into(),
            at: now - 3 * 86_400,
            keeping: Some(42)
        })
        .contains("keeping v42")
    );
}

#[test]
fn rfc3339_utc_parser_matches_known_epochs() {
    assert_eq!(parse_rfc3339_utc("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        parse_rfc3339_utc("2026-08-30T00:00:00Z"),
        Some(1_788_048_000)
    );
    assert_eq!(
        parse_rfc3339_utc("2026-08-30T00:00:00.123Z"),
        Some(1_788_048_000)
    );
    assert_eq!(parse_rfc3339_utc("2026-08-30T00:00:00+02:00"), None);
    assert_eq!(parse_rfc3339_utc("garbage"), None);
}

#[test]
fn pinned_keys_are_well_formed() {
    for key in super::TRUSTED_KEYS {
        assert!(key.key_id.starts_with("cwf-"));
        assert_ne!(key.public_key, [0u8; 32]);
    }
}
