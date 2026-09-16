//! Executable conformance checks for [`AuditStore`].
//!
//! Backend tests can call [`run_audit_store_conformance_tests`] with a fresh store (or
//! a unique prefix when using a shared integration database). The contract
//! deliberately covers only portable, synchronous store behavior; retention
//! cleanup has backend-specific clock and deletion semantics.

use std::collections::HashSet;

use chrono::{Duration, Utc};
use serde_json::json;

use crate::{AuditError, AuditQuery, AuditRecord, store::AuditStore};

/// Run the portable audit-store conformance suite.
///
/// `prefix` must be unique when the store is shared with other tests. It is
/// incorporated into record IDs, action IDs, namespace, and tenant so this
/// suite neither observes nor mutates another test's records.
///
/// # Errors
///
/// Returns a backend error when a storage operation fails. Assertion failures
/// identify a violated `AuditStore` contract.
#[allow(clippy::too_many_lines)]
pub async fn run_audit_store_conformance_tests(
    store: &dyn AuditStore,
    prefix: &str,
) -> Result<(), AuditError> {
    let namespace = format!("audit-contract-{prefix}");
    let tenant = format!("tenant-{prefix}");
    let action_id = format!("action-{prefix}");
    let base = Utc::now() - Duration::minutes(1);

    assert!(
        store
            .get_by_id(&format!("missing-{prefix}"))
            .await?
            .is_none(),
        "missing audit IDs must return None"
    );
    assert!(
        store
            .get_by_action_id(&format!("missing-action-{prefix}"))
            .await?
            .is_none(),
        "missing action IDs must return None"
    );

    let first = record(prefix, "first", &action_id, &namespace, &tenant, base);
    let mut latest = record(
        prefix,
        "latest",
        &action_id,
        &namespace,
        &tenant,
        base + Duration::seconds(1),
    );
    latest.outcome = String::from("failed");
    let mut caller = record(
        prefix,
        "caller",
        &format!("caller-action-{prefix}"),
        &namespace,
        &tenant,
        base + Duration::seconds(2),
    );
    caller.caller_id = format!("caller-{prefix}");
    caller.chain_id = Some(format!("chain-{prefix}"));
    caller.signer_id = Some(format!("signer-{prefix}"));
    caller.kid = Some(format!("kid-{prefix}"));
    let same_timestamp = record(
        prefix,
        "same-timestamp",
        &format!("same-timestamp-action-{prefix}"),
        &namespace,
        &tenant,
        base + Duration::seconds(2),
    );
    let mut child = record(
        prefix,
        "child",
        &format!("child-action-{prefix}"),
        &namespace,
        &format!("{tenant}.child"),
        base + Duration::seconds(3),
    );
    child.outcome = String::from("suppressed");

    for entry in [&first, &latest, &caller, &same_timestamp, &child] {
        store.record(entry.clone()).await?;
    }

    let round_trip = store
        .get_by_id(&caller.id)
        .await?
        .expect("recorded audit ID must be retrievable");
    assert_eq!(round_trip.action_id, caller.action_id);
    assert_eq!(round_trip.caller_id, caller.caller_id);

    let newest = store
        .get_by_action_id(&action_id)
        .await?
        .expect("recorded action ID must be retrievable");
    assert_eq!(
        newest.id, latest.id,
        "action lookup must return the newest record"
    );

    let exact = AuditQuery {
        namespace: Some(namespace.clone()),
        tenant: Some(tenant.clone()),
        limit: Some(10),
        ..AuditQuery::default()
    };
    let exact_page = store.query(&exact).await?;
    let exact_ids: HashSet<&str> = exact_page
        .records
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(
        exact_ids.len(),
        4,
        "exact tenant query must exclude children"
    );
    for entry in [&first, &latest, &caller, &same_timestamp] {
        assert!(
            exact_ids.contains(entry.id.as_str()),
            "exact tenant query missed {}",
            entry.id
        );
    }

    let caller_page = store
        .query(&AuditQuery {
            namespace: Some(namespace.clone()),
            tenant: Some(tenant.clone()),
            caller_id: Some(caller.caller_id.clone()),
            limit: Some(10),
            ..AuditQuery::default()
        })
        .await?;
    assert_eq!(
        caller_page
            .records
            .iter()
            .map(|entry| &entry.id)
            .collect::<Vec<_>>(),
        vec![&caller.id],
        "caller_id must be an exact query filter"
    );

    let scoped_page = store
        .query(&AuditQuery {
            namespace: Some(namespace.clone()),
            tenant_scope: vec![tenant.clone()],
            limit: Some(10),
            ..AuditQuery::default()
        })
        .await?;
    let scoped_ids: HashSet<&str> = scoped_page
        .records
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(scoped_ids.len(), 5, "tenant scope must include descendants");
    assert!(scoped_ids.contains(child.id.as_str()));

    let mut cursor = None;
    let mut seen = HashSet::new();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(
            pages <= 3,
            "cursor pagination did not reach a terminal page"
        );
        let page = store
            .query(&AuditQuery {
                namespace: Some(namespace.clone()),
                tenant: Some(tenant.clone()),
                limit: Some(2),
                cursor: cursor.clone(),
                ..AuditQuery::default()
            })
            .await?;
        assert!(
            page.records.len() <= 2,
            "cursor page must honor its requested limit"
        );
        for entry in page.records {
            assert!(
                seen.insert(entry.id),
                "cursor pagination must not repeat a record"
            );
        }
        let Some(next) = page.next_cursor else {
            break;
        };
        cursor = Some(next);
    }
    assert_eq!(
        seen.len(),
        4,
        "cursor pagination must reach every exact match"
    );
    for entry in [&first, &latest, &caller, &same_timestamp] {
        assert!(seen.contains(&entry.id));
    }

    Ok(())
}

fn record(
    prefix: &str,
    suffix: &str,
    action_id: &str,
    namespace: &str,
    tenant: &str,
    dispatched_at: chrono::DateTime<Utc>,
) -> AuditRecord {
    AuditRecord {
        id: format!("{prefix}-{suffix}"),
        action_id: action_id.to_owned(),
        chain_id: None,
        namespace: namespace.to_owned(),
        tenant: tenant.to_owned(),
        provider: "conformance".to_owned(),
        action_type: "audit.conformance".to_owned(),
        verdict: "allow".to_owned(),
        matched_rule: None,
        outcome: "executed".to_owned(),
        action_payload: Some(json!({"contract": true})),
        verdict_details: json!({"source": "audit-store-conformance"}),
        outcome_details: json!({}),
        metadata: json!({}),
        dispatched_at,
        completed_at: dispatched_at + Duration::milliseconds(1),
        duration_ms: 1,
        expires_at: None,
        caller_id: String::new(),
        auth_method: "test".to_owned(),
        record_hash: None,
        previous_hash: None,
        sequence_number: None,
        attachment_metadata: Vec::new(),
        signature: None,
        signer_id: None,
        kid: None,
        canonical_hash: None,
    }
}
