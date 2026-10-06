//! Trust-root startup validation without environment mutation or state access.
use acteon_server::config::{
    ExecutionAuthorityConfig, ReconciliationSourceConfig, prepare_reconciliation_sources,
};
use serde_json::{Value, json};
use zeroize::Zeroizing;

fn execution() -> ExecutionAuthorityConfig {
    serde_json::from_value(json!({"scopes":[{
        "namespace":"prod", "tenant":"acme", "publisher":{"id":"publisher","kind":"system"},
        "subjects":[{"id":"agent/maya","kind":"agent"}],
        "routes":[{"provider":"incident","action_type":"execute"}],
        "valid_from_ms":0,"credential_limits":{"max_units":5,"max_concurrent":2,"deadline_ms":4_102_444_800_000_i64},
        "root_max_units":5,"root_max_concurrent":1,"root_lifetime_ms":60000
    }]})).unwrap()
}
fn declaration() -> Value {
    json!({"namespace":"prod","tenant":"acme","binding_digest":"a".repeat(64),
        "source_id":"incident-journal","qualification_ref":"contracts/incident-finality-v1",
        "verifier_revision":"journal-v1","keys":[{"id":"k1","secret_env":"ACTEON_FINALITY_INCIDENT_K1"}]})
}
fn source(value: Value) -> ReconciliationSourceConfig {
    serde_json::from_value(value).unwrap()
}
fn secret(name: &str) -> Option<Zeroizing<String>> {
    (name == "ACTEON_FINALITY_INCIDENT_K1").then(|| Zeroizing::new("ab".repeat(32)))
}
#[test]
fn source_configuration_is_optional_bounded_and_scope_exact() {
    assert!(
        prepare_reconciliation_sources(&[], None, |_| panic!("no secret read"))
            .unwrap()
            .is_empty()
    );
    let config = execution();
    let installed =
        prepare_reconciliation_sources(&[source(declaration())], Some(&config), secret).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].namespace, "prod");
    assert_eq!(installed[0].tenant, "acme");
    assert_eq!(
        installed[0].bindings[&"a".repeat(64)].revision(),
        "journal-v1"
    );
    assert!(prepare_reconciliation_sources(&[source(declaration())], None, secret).is_err());
    for (field, value) in [
        ("tenant", "other"),
        ("binding_digest", "A"),
        ("source_id", ""),
        ("qualification_ref", " "),
        ("verifier_revision", "*"),
    ] {
        let mut changed = declaration();
        changed[field] = json!(value);
        assert!(
            prepare_reconciliation_sources(&[source(changed)], Some(&config), secret).is_err(),
            "{field}"
        );
    }
    let excessive = (0..129).map(|_| source(declaration())).collect::<Vec<_>>();
    assert!(prepare_reconciliation_sources(&excessive, Some(&config), secret).is_err());
    let mut read_only = config;
    read_only.scopes[0].history_only = true;
    assert!(
        prepare_reconciliation_sources(&[source(declaration())], Some(&read_only), secret).is_err()
    );
}
#[test]
fn finality_keys_are_dedicated_and_errors_do_not_disclose_secrets() {
    let config = execution();
    let sources = [source(declaration())];
    assert!(prepare_reconciliation_sources(&sources, Some(&config), |_| None).is_err());
    for material in [
        "sensitive-not-hex".to_owned(),
        "ab".repeat(31),
        "ab".repeat(1025),
    ] {
        let error = prepare_reconciliation_sources(&sources, Some(&config), |_| {
            Some(Zeroizing::new(material.clone()))
        })
        .err()
        .unwrap();
        assert!(!error.contains(&material));
    }
    for authority in ["ab".repeat(32), "AB".repeat(32)] {
        let result = prepare_reconciliation_sources(&sources, Some(&config), |name| {
            if name == "ACTEON_AUTH_KEY" {
                Some(Zeroizing::new(authority.clone()))
            } else {
                secret(name)
            }
        });
        assert!(result.is_err(), "encoded authority alias");
    }
    let mut invalid = declaration();
    invalid["keys"][0]["secret_env"] = json!("ACTEON_EXECUTION_AUTHORITY_KEY");
    assert!(prepare_reconciliation_sources(&[source(invalid)], Some(&config), secret).is_err());
    let mut duplicate = declaration();
    duplicate["keys"]
        .as_array_mut()
        .unwrap()
        .push(declaration()["keys"][0].clone());
    assert!(prepare_reconciliation_sources(&[source(duplicate)], Some(&config), secret).is_err());
}
#[test]
fn duplicate_bindings_and_conflicting_source_revisions_are_refused() {
    let config = execution();
    let mut second = declaration();
    assert!(
        prepare_reconciliation_sources(
            &[source(declaration()), source(second.clone())],
            Some(&config),
            secret
        )
        .is_err()
    );
    second["binding_digest"] = json!("b".repeat(64));
    let installed = prepare_reconciliation_sources(
        &[source(declaration()), source(second.clone())],
        Some(&config),
        secret,
    )
    .unwrap();
    assert_eq!(installed[0].bindings.len(), 2);
    second["verifier_revision"] = json!("journal-v2");
    assert!(
        prepare_reconciliation_sources(
            &[source(declaration()), source(second.clone())],
            Some(&config),
            secret
        )
        .is_err()
    );
    second["verifier_revision"] = json!("journal-v1");
    second["keys"][0]["id"] = json!("k2");
    assert!(
        prepare_reconciliation_sources(
            &[source(declaration()), source(second)],
            Some(&config),
            secret
        )
        .is_err()
    );
    let mut unknown = declaration();
    unknown["secret"] = json!("inline-secret");
    assert!(serde_json::from_value::<ReconciliationSourceConfig>(unknown).is_err());
}

#[test]
fn server_toml_accepts_explicit_sources_and_preserves_default_denial() {
    let absent: acteon_server::config::ActeonConfig = toml::from_str("").unwrap();
    assert!(absent.reconciliation_sources.is_empty());
    let configured: acteon_server::config::ActeonConfig = toml::from_str(&format!(
        r#"
[[reconciliation_sources]]
namespace = "prod"
tenant = "acme"
binding_digest = "{}"
source_id = "incident-journal"
qualification_ref = "contracts/incident-finality-v1"
verifier_revision = "journal-v1"
keys = [{{ id = "k1", secret_env = "ACTEON_FINALITY_INCIDENT_K1" }}]
"#,
        "a".repeat(64)
    ))
    .unwrap();
    let installed = prepare_reconciliation_sources(
        &configured.reconciliation_sources,
        Some(&execution()),
        secret,
    )
    .unwrap();
    assert_eq!(installed.len(), 1);
    assert!(
        toml::from_str::<acteon_server::config::ActeonConfig>(
            "[[reconciliation_sources]]\nnamespace='prod'\nsecret='inline-secret'"
        )
        .is_err()
    );
}
