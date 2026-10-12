//! Federation-finality startup validation without state or network access.

#![recursion_limit = "512"]

use acteon_server::config::{
    ExecutionAuthorityConfig, FederationFinalitySourceConfig, ReconciliationSourceConfig,
    prepare_federation_finality_sources, validate_federation_capability_key_separation,
    validate_finality_source_key_separation,
};
use serde_json::json;
use zeroize::Zeroizing;

fn execution() -> ExecutionAuthorityConfig {
    serde_json::from_value(json!({"scopes":[{
        "namespace":"prod", "tenant":"acme",
        "publisher":{"id":"publisher","kind":"system"},
        "subjects":[{"id":"agent/maya","kind":"agent"}],
        "routes":[{"provider":"incident","action_type":"execute"}],
        "valid_from_ms":0,
        "credential_limits":{"max_units":5,"max_concurrent":2,"deadline_ms":4_102_444_800_000_i64},
        "root_max_units":5,"root_max_concurrent":1,"root_lifetime_ms":60000,
        "federation":{
            "local_audience":"acteon://prod/acme", "trusts":[], "peers":[],
            "exports":[{
                "id":"remote-triage", "source_agent_id":"maya",
                "remote_origin":"https://remote.example", "remote_audience":"acteon://remote/prod",
                "active_epoch":7,
                "epochs":[{
                    "trust_id":"local-prod", "trust_revision":2, "key_id":"local-key-v2",
                    "verifying_key":"a".repeat(64), "signer_key_env":"ACTEON_SIGNER",
                    "issuer_epoch":7, "credential_env":"ACTEON_PEER_CREDENTIAL",
                    "peer_capability":{
                        "key_id":"peer-capability-v1", "verifying_key":"b".repeat(64),
                        "minimum_issuer_epoch":1, "max_clock_skew_ms":30000,
                        "protocol_revision":"acteon-federation-v1",
                        "authentication_scheme":"bearer", "require_peer_finality":true
                    },
                    "grant":{
                        "source":{"id":"agent/maya","kind":"agent"},
                        "target":{"id":"agent/remote","kind":"agent"},
                        "agent_resource":{"kind":"agent","namespace":"remote","tenant":"prod","id":"triage"},
                        "binding_digest":"b".repeat(64), "skill":"triage",
                        "ingress_effect":{"operation":"agent.invoke","resources":[{"kind":"agent","namespace":"remote","tenant":"prod","id":"triage"}]},
                        "effects":[{"operation":"incident.inspect","resources":[{"kind":"endpoint","namespace":"remote","tenant":"prod","id":"incident"}]}],
                        "limits":{"max_units":2,"max_concurrent":1,"deadline_ms":4_102_444_800_000_i64},
                        "max_depth":1
                    }
                }]
            }]
        }
    }]})).unwrap()
}

fn source(profile: &str, secret_env: &str) -> FederationFinalitySourceConfig {
    serde_json::from_value(json!({
        "namespace":"prod", "tenant":"acme", "export_profile_id":profile,
        "issuer_epoch":7, "source_id":"remote-journal",
        "qualification_ref":"contracts/remote-finality-v1",
        "verifier_revision":"remote-finality-v1",
        "keys":[{"id":"k1","secret_env":secret_env}]
    }))
    .unwrap()
}

#[test]
fn finality_source_is_optional_profile_bound_and_secret_safe() {
    assert!(
        prepare_federation_finality_sources(&[], None, |_| None)
            .unwrap()
            .is_empty()
    );
    let config = execution();
    let installed = prepare_federation_finality_sources(
        &[source(
            "remote-triage",
            "ACTEON_FEDERATION_FINALITY_REMOTE_K1",
        )],
        Some(&config),
        |name| match name {
            "ACTEON_FEDERATION_FINALITY_REMOTE_K1" => Some(Zeroizing::new("ab".repeat(32))),
            "ACTEON_SIGNER" => Some(Zeroizing::new("cd".repeat(32))),
            "ACTEON_PEER_CREDENTIAL" => Some(Zeroizing::new("peer-token".into())),
            _ => None,
        },
    )
    .unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].profile_id, "remote-triage");
    assert_eq!(installed[0].issuer_epoch, 7);
    assert_eq!(installed[0].verifier.revision(), "remote-finality-v1");

    assert!(
        prepare_federation_finality_sources(
            &[source("other", "ACTEON_FEDERATION_FINALITY_REMOTE_K1")],
            Some(&config),
            |_| Some(Zeroizing::new("ab".repeat(32))),
        )
        .is_err()
    );
    assert!(
        prepare_federation_finality_sources(
            &[source("remote-triage", "ACTEON_FINALITY_SHARED")],
            Some(&config),
            |_| Some(Zeroizing::new("ab".repeat(32))),
        )
        .is_err()
    );
    assert!(
        prepare_federation_finality_sources(
            &[source(
                "remote-triage",
                "ACTEON_FEDERATION_FINALITY_REMOTE_K1"
            )],
            Some(&config),
            |_| None,
        )
        .is_err()
    );
    assert!(
        prepare_federation_finality_sources(
            &[source(
                "remote-triage",
                "ACTEON_FEDERATION_FINALITY_REMOTE_K1"
            )],
            Some(&config),
            |name| match name {
                "ACTEON_FEDERATION_FINALITY_REMOTE_K1" | "ACTEON_SIGNER" => {
                    Some(Zeroizing::new("ab".repeat(32)))
                }
                "ACTEON_PEER_CREDENTIAL" => Some(Zeroizing::new("peer-token".into())),
                _ => None,
            },
        )
        .is_err()
    );
}

#[test]
fn provider_and_federation_finality_keys_cannot_alias() {
    let provider: ReconciliationSourceConfig = serde_json::from_value(json!({
        "namespace":"prod", "tenant":"acme", "binding_digest":"a".repeat(64),
        "source_id":"provider-journal", "qualification_ref":"provider-v1",
        "verifier_revision":"provider-v1",
        "keys":[{"id":"provider-k1","secret_env":"ACTEON_FINALITY_PROVIDER_K1"}]
    }))
    .unwrap();
    let federation = source("remote-triage", "ACTEON_FEDERATION_FINALITY_REMOTE_K1");
    assert!(
        validate_finality_source_key_separation(&[provider], &[federation], |name| {
            matches!(
                name,
                "ACTEON_FINALITY_PROVIDER_K1" | "ACTEON_FEDERATION_FINALITY_REMOTE_K1"
            )
            .then(|| Zeroizing::new("ab".repeat(32)))
        })
        .is_err()
    );
}

#[test]
fn capability_publisher_key_cannot_share_other_trust_material() {
    let mut config = execution();
    config.scopes[0]
        .federation
        .as_mut()
        .unwrap()
        .capability_publisher = Some(
        serde_json::from_value(json!({
            "key_id":"local-capability-v1", "verifying_key":"c".repeat(64),
            "signer_key_env":"ACTEON_FEDERATION_CAPABILITY_SIGNER",
            "issuer_epoch":1, "lifetime_ms":300000,
            "protocol_revision":"acteon-federation-v1",
            "authentication_schemes":["bearer"], "peer_finality":true
        }))
        .unwrap(),
    );
    let read = |shared: bool, name: &str| match name {
        "ACTEON_FEDERATION_CAPABILITY_SIGNER" => Some(Zeroizing::new(if shared {
            "ab".repeat(32)
        } else {
            "ef".repeat(32)
        })),
        "ACTEON_SIGNER" => Some(Zeroizing::new("ab".repeat(32))),
        "ACTEON_PEER_CREDENTIAL" => Some(Zeroizing::new("peer-token".into())),
        _ => None,
    };
    assert!(
        validate_federation_capability_key_separation(Some(&config), &[], &[], |name| {
            read(true, name)
        })
        .is_err()
    );
    validate_federation_capability_key_separation(Some(&config), &[], &[], |name| {
        read(false, name)
    })
    .unwrap();
}
