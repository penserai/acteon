//! Synthetic bank source -> managed suspicious-activity filter -> outbox -> timeline.
//!
//! Run without external services:
//! `cargo run -p acteon-simulation --features bus --example bank_transaction_pipeline`
//! These illustrative rules produce review candidates, not findings of fraud.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

use acteon_bus::{
    ManagedStreamStage, StreamCheckpointConfig, StreamCheckpointCoordinator, StreamDeliveryError,
    StreamOutboxConfig, StreamOutboxDelivery, StreamOutboxDispatchResult, StreamOutboxDispatcher,
    StreamOutboxEntry, StreamStageConfig, StreamStageInput, StreamStageProcessError,
    StreamStageProcessor, StreamStageResult, StreamStageSource, StreamStageTransition,
    stream_checkpoint_key,
};
use acteon_core::timeline::{
    ChangeDisposition, ChangeOperation, EventBody, TimelineChange, TimelineEvent, TimelineQuery,
    TimelineScope,
};
use acteon_state_memory::MemoryStateStore;
use acteon_timeline::{
    TimelineService, config::TimelineConfig, ledger::DiskLedger, memory::MemoryTimelineStore,
};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[path = "bank_transaction_pipeline/source.rs"]
mod source;
use source::TransactionSource;

type DemoResult<T> = Result<T, Box<dyn std::error::Error>>;
const DETECTOR: &str = "bank-suspicion-v1";
const LARGE_TRANSFER_CENTS: u64 = 1_000_000;
const WINDOW_SECONDS: i64 = 120;
const VELOCITY_COUNT: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PaymentKind {
    Transfer,
    Card,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Transaction {
    id: String,
    reference: String,
    account_id: String,
    counterparty_id: String,
    amount_cents: u64,
    currency: String,
    kind: PaymentKind,
    occurred_at: DateTime<Utc>,
    home_country: String,
    origin_country: String,
    new_payee: bool,
    known_device: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AccountWindow {
    latest: Option<DateTime<Utc>>,
    transfers: VecDeque<DateTime<Utc>>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DetectorState {
    received: u64,
    duplicates: u64,
    suspicious: u64,
    // This finite demo keeps every transaction for conflicting-duplicate detection.
    // A long-lived deployment needs a bounded deduplication retention policy.
    seen: BTreeMap<String, Transaction>,
    accounts: BTreeMap<String, AccountWindow>,
}
struct SuspiciousActivityFilter;
impl SuspiciousActivityFilter {
    fn detect(
        state: &mut DetectorState,
        tx: &Transaction,
    ) -> Result<Option<TimelineChange>, String> {
        state.received += 1;
        if let Some(previous) = state.seen.get(&tx.id) {
            if previous != tx {
                return Err(format!("conflicting transaction ID {}", tx.id));
            }
            state.duplicates += 1;
            return Ok(None);
        }
        if tx.currency != "USD" || tx.amount_cents == 0 {
            return Err("demo requires positive USD amounts in integer cents".into());
        }
        let window = state.accounts.entry(tx.account_id.clone()).or_default();
        if window.latest.is_some_and(|last| tx.occurred_at < last) {
            return Err("demo requires event-time order within each account".into());
        }
        window.latest = Some(tx.occurred_at);
        let lower = tx.occurred_at - TimeDelta::seconds(WINDOW_SECONDS);
        while window.transfers.front().is_some_and(|time| *time < lower) {
            window.transfers.pop_front();
        }
        if tx.kind == PaymentKind::Transfer {
            window.transfers.push_back(tx.occurred_at);
        }
        let mut reasons = Vec::new();
        let mut descriptions = Vec::new();
        if tx.kind == PaymentKind::Transfer
            && tx.amount_cents >= LARGE_TRANSFER_CENTS
            && tx.new_payee
        {
            reasons.push("large_new_payee");
            descriptions.push("Large transfer to a new payee");
        }
        if tx.origin_country != tx.home_country && !tx.known_device {
            reasons.push("new_country_device");
            descriptions.push("New device used from another country");
        }
        if tx.kind == PaymentKind::Transfer && window.transfers.len() >= VELOCITY_COUNT {
            reasons.push("rapid_transfers");
            descriptions.push("Rapid transfer burst within two minutes");
        }
        state.seen.insert(tx.id.clone(), tx.clone());
        if reasons.is_empty() {
            return Ok(None);
        }
        state.suspicious += 1;
        let severity = if reasons.len() > 1 {
            "critical"
        } else if reasons.contains(&"large_new_payee") || reasons.contains(&"rapid_transfers") {
            "high"
        } else {
            "medium"
        };
        let group = format!("review:{}:{}", tx.account_id, tx.occurred_at.date_naive());
        Ok(Some(TimelineChange {
            schema_version: 1,
            change_id: format!("{DETECTOR}:{}", tx.id),
            source: DETECTOR.into(),
            event_id: tx.id.clone(),
            revision: 1,
            operation: ChangeOperation::Create,
            before: None,
            after: Some(EventBody {
                event_group: group,
                occurred_at: tx.occurred_at,
                event_type: "bank.transaction.suspicious".into(),
                message: Some(descriptions.join("; ")),
                attributes: json!({
                    "transaction": {"id": tx.id, "reference": tx.reference, "amountCents": tx.amount_cents, "currency": tx.currency, "kind": tx.kind},
                    "account": {"id": tx.account_id}, "counterpartyId": tx.counterparty_id,
                    "originCountry": tx.origin_country, "homeCountry": tx.home_country,
                    "newPayee": tx.new_payee, "knownDevice": tx.known_device,
                    "amountBucket": if tx.amount_cents >= LARGE_TRANSFER_CENTS { "large" } else { "standard" },
                    "severity": severity, "reasons": reasons,
                    "transferCountInWindow": window.transfers.len(), "windowSeconds": WINDOW_SECONDS,
                    "detectorVersion": DETECTOR,
                }),
            }),
        }))
    }
}
#[async_trait]
impl StreamStageProcessor<Transaction, DetectorState, TimelineChange> for SuspiciousActivityFilter {
    async fn process(
        &self,
        mut state: DetectorState,
        inputs: &[StreamStageInput<Transaction>],
    ) -> Result<StreamStageTransition<DetectorState, TimelineChange>, StreamStageProcessError> {
        let mut outputs = Vec::new();
        for input in inputs {
            if let Some(change) = Self::detect(&mut state, &input.payload)
                .map_err(StreamStageProcessError::Permanent)?
            {
                outputs.push(StreamOutboxEntry {
                    idempotency_key: change.change_id.clone(),
                    created_at: Utc::now(),
                    payload: change,
                });
            }
        }
        // The stage atomically saves this state, output outbox and source position
        // before acknowledging the original receipt. No timeline side effects here.
        Ok(StreamStageTransition { state, outputs })
    }
}
struct TimelineSink {
    service: Arc<TimelineService>,
    scope: TimelineScope,
}
#[async_trait]
impl StreamOutboxDelivery<TimelineChange> for TimelineSink {
    async fn deliver(
        &self,
        entry: &StreamOutboxEntry<TimelineChange>,
    ) -> Result<(), StreamDeliveryError> {
        if entry.idempotency_key != entry.payload.change_id {
            return Err(StreamDeliveryError::Permanent(
                "outbox/change identity mismatch".into(),
            ));
        }
        let receipt = self
            .service
            .ingest(&self.scope, entry.payload.clone())
            .await
            .map_err(|error| match error {
                acteon_timeline::Error::Invalid(_)
                | acteon_timeline::Error::Conflict
                | acteon_timeline::Error::Json(_) => {
                    StreamDeliveryError::Permanent(error.to_string())
                }
                _ => StreamDeliveryError::Retryable(error.to_string()),
            })?;
        match receipt.disposition {
            ChangeDisposition::Applied | ChangeDisposition::Duplicate => Ok(()),
            other => Err(StreamDeliveryError::Permanent(format!(
                "unexpected timeline disposition: {other:?}"
            ))),
        }
    }
}
fn transaction_id(n: u32) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}
#[allow(clippy::too_many_lines)] // Keep the finite input fixture together.
fn transactions() -> Vec<Transaction> {
    let base = DateTime::parse_from_rfc3339("2026-10-24T09:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    // seconds, account, cents, kind, payee, origin, new payee, known device
    let rows = [
        (
            0,
            "alex",
            10_000,
            PaymentKind::Transfer,
            "utilities",
            "US",
            false,
            true,
        ),
        (
            10,
            "blair",
            4_500,
            PaymentKind::Card,
            "grocer",
            "US",
            false,
            true,
        ),
        (
            20,
            "alex",
            1_500_000,
            PaymentKind::Transfer,
            "orion",
            "US",
            true,
            true,
        ),
        (
            30,
            "casey",
            30_000,
            PaymentKind::Card,
            "hotel",
            "FR",
            false,
            false,
        ),
        (
            40,
            "alex",
            90_000,
            PaymentKind::Transfer,
            "rent",
            "US",
            false,
            true,
        ),
        (
            50,
            "blair",
            80_000,
            PaymentKind::Transfer,
            "utilities",
            "US",
            false,
            true,
        ),
        (
            60,
            "casey",
            2_000_000,
            PaymentKind::Transfer,
            "nova",
            "FR",
            true,
            false,
        ),
        (
            180,
            "alex",
            25_000,
            PaymentKind::Transfer,
            "utilities",
            "US",
            false,
            true,
        ),
        (
            190,
            "blair",
            1_200_000,
            PaymentKind::Transfer,
            "orion",
            "US",
            true,
            true,
        ),
        (
            200,
            "casey",
            65_000,
            PaymentKind::Transfer,
            "rent",
            "US",
            false,
            true,
        ),
        (
            210,
            "blair",
            10_000,
            PaymentKind::Transfer,
            "utilities",
            "US",
            false,
            true,
        ),
        (
            220,
            "blair",
            20_000,
            PaymentKind::Transfer,
            "rent",
            "US",
            false,
            true,
        ),
    ];
    rows.into_iter()
        .enumerate()
        .map(
            |(i, (seconds, account, cents, kind, payee, country, new_payee, known_device))| {
                let n = u32::try_from(i + 1).unwrap();
                Transaction {
                    id: transaction_id(n),
                    reference: format!("TX-{n:03}"),
                    account_id: format!("acct-{account}"),
                    counterparty_id: format!("payee-{payee}"),
                    amount_cents: cents,
                    currency: "USD".into(),
                    kind,
                    occurred_at: base + TimeDelta::seconds(seconds),
                    home_country: "US".into(),
                    origin_country: country.into(),
                    new_payee,
                    known_device,
                }
            },
        )
        .collect()
}
async fn query_all(
    service: &TimelineService,
    scope: &TimelineScope,
    mut query: TimelineQuery,
) -> DemoResult<(Vec<TimelineEvent>, usize)> {
    let mut events = Vec::new();
    let mut pages = 0;
    loop {
        let page = service.query(scope, &query).await?;
        pages += 1;
        events.extend(page.events);
        let Some(cursor) = page.next_cursor else {
            break;
        };
        // Preserve every predicate and ordering option when advancing the cursor.
        query.cursor = Some(cursor);
    }
    Ok((events, pages))
}
async fn searches(service: &TimelineService, scope: &TimelineScope) -> DemoResult<()> {
    let uuid = transaction_id(7);
    let cases = [
        (
            "Exact transaction UUID",
            format!(r#"attributes.transaction.id == "{uuid}""#),
            vec!["TX-007"],
        ),
        (
            "Transfer bursts",
            r#""rapid_transfers" in attributes.reasons"#.into(),
            vec!["TX-005", "TX-012"],
        ),
        (
            "High severity or critical, using a new device",
            r#"attributes.severity in ["high", "critical"] && attributes.knownDevice == false"#
                .into(),
            vec!["TX-007"],
        ),
        (
            "Shared counterparty across accounts",
            r#"attributes.counterpartyId == "payee-orion""#.into(),
            vec!["TX-003", "TX-009"],
        ),
        (
            "Full-text investigation",
            r#"textMatch("large transfer")"#.into(),
            vec!["TX-003", "TX-007", "TX-009"],
        ),
        (
            "Large-amount bucket and new payee",
            r#"attributes.amountBucket == "large" && attributes.newPayee == true"#.into(),
            vec!["TX-003", "TX-007", "TX-009"],
        ),
        (
            "Later activity",
            r#"occurredAt >= timestamp("2026-10-24T09:02:30Z")"#.into(),
            vec!["TX-009", "TX-012"],
        ),
    ];
    println!("\nInteresting searches (every result below is asserted):");
    for (label, filter, expected) in cases {
        let query = TimelineQuery {
            filter: Some(filter.clone()),
            ascending: Some(true),
            limit: Some(2),
            ..Default::default()
        };
        let (events, _) = query_all(service, scope, query).await?;
        let references: Vec<_> = events
            .iter()
            .map(|e| {
                e.event.attributes["transaction"]["reference"]
                    .as_str()
                    .unwrap()
            })
            .collect();
        assert_eq!(references, expected, "{label}");
        println!(
            "\n{label}\n  CEL: {filter}\n  Matches: {}",
            references.join(", ")
        );
    }
    let group_query = TimelineQuery {
        event_group: Some("review:acct-alex:2026-10-24".into()),
        limit: Some(1),
        ..Default::default()
    };
    let (group, pages) = query_all(service, scope, group_query.clone()).await?;
    assert_eq!(
        group.iter().map(|e| e.event_id.clone()).collect::<Vec<_>>(),
        vec![transaction_id(3), transaction_id(5)]
    );
    assert_eq!(pages, 2);
    println!(
        "\nAccount review timeline: TX-003, TX-005 (two pages)\n  query: {}",
        serde_json::to_string(&group_query)?
    );
    let other = TimelineScope {
        tenant: "another-bank".into(),
        ..scope.clone()
    };
    assert_eq!(
        service
            .query(
                &other,
                &TimelineQuery {
                    filter: Some("true".into()),
                    ..Default::default()
                }
            )
            .await?
            .events,
        Vec::<TimelineEvent>::new()
    );
    println!("\nTenant isolation verified: another-bank sees no events.");
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn simulate(directory: &std::path::Path) -> DemoResult<()> {
    let checkpoint_store = Arc::new(MemoryStateStore::new());
    let scope = TimelineScope {
        namespace: "banking".into(),
        tenant: "demo-bank".into(),
    };
    let service = Arc::new(TimelineService::new(
        Arc::new(MemoryTimelineStore::default()),
        Arc::new(DiskLedger::open(directory, 16 * 1024 * 1024)?),
        checkpoint_store.clone(),
        TimelineConfig::default(),
        None,
    )?);
    let sink = TimelineSink {
        service: service.clone(),
        scope: scope.clone(),
    };
    let topic_name = format!("{}.{}.transactions", scope.namespace, scope.tenant);
    let mut inputs = transactions();
    inputs.push(inputs[2].clone()); // Same ID/content at a different source offset.
    println!(
        "Synthetic bank transaction pipeline\nSource -> suspicious-activity filter -> checkpoint outbox -> timeline\n"
    );
    println!(
        "Rules: new-payee transfer >= USD 10000; foreign-origin new device; >= 3 transfers/account in 120s."
    );
    println!("Source: 12 unique transactions + 1 duplicate; one ordered partition.\n");
    let mut source = TransactionSource::new(&topic_name, &inputs)?;
    let key = stream_checkpoint_key(scope.namespace.as_str(), scope.tenant.as_str(), DETECTOR);
    let coordinator = StreamCheckpointCoordinator::initialize(
        checkpoint_store.clone(),
        key.clone(),
        DetectorState::default(),
        StreamCheckpointConfig::default(),
    )
    .await?;
    let mut stage = ManagedStreamStage::initialize(
        coordinator,
        "filter-worker",
        DETECTOR,
        source.identity(),
        StreamStageConfig {
            max_batch_records: 1,
            ..Default::default()
        },
    )
    .await?;
    let output_coordinator = StreamCheckpointCoordinator::initialize(
        checkpoint_store,
        key,
        DetectorState::default(),
        StreamCheckpointConfig::default(),
    )
    .await?;
    let mut dispatcher = StreamOutboxDispatcher::initialize(
        output_coordinator,
        "timeline-worker",
        StreamOutboxConfig::default(),
    )
    .await?;
    let cancel = CancellationToken::new();
    let mut retry_change = None;
    for tx in &inputs {
        match stage
            .process_once(&mut source, &SuspiciousActivityFilter, &cancel)
            .await?
        {
            StreamStageResult::Completed { processed: 1, .. } => (),
            other => return Err(format!("unexpected stage result: {other:?}").into()),
        }
        let output = stage.checkpoint().pending_outputs().first();
        if let Some(entry) = output {
            retry_change.get_or_insert_with(|| entry.payload.clone());
        }
        println!(
            "  {} -> {}",
            tx.reference,
            if output.is_some() {
                "suspicious event"
            } else {
                "filtered out / duplicate"
            }
        );
        loop {
            match dispatcher.dispatch_once(&sink).await? {
                StreamOutboxDispatchResult::Delivered { .. } => (),
                StreamOutboxDispatchResult::Idle => break,
                other => return Err(format!("unexpected output result: {other:?}").into()),
            }
        }
    }
    assert_eq!(source.acknowledged(), inputs.len());
    source.close().await;
    let counters = stage.checkpoint().state();
    assert_eq!(
        (counters.received, counters.duplicates, counters.suspicious),
        (13, 1, 6)
    );
    assert_eq!(counters.seen.len(), 12);
    let metrics = dispatcher.metrics().await?;
    assert_eq!((metrics.counters.delivered, metrics.pending), (6, 0));
    let retry = service.ingest(&scope, retry_change.unwrap()).await?;
    assert_eq!(retry.disposition, ChangeDisposition::Duplicate);
    assert_eq!(service.ledger.status(&scope).await?.records, 6);
    let (events, pages) = query_all(
        &service,
        &scope,
        TimelineQuery {
            ascending: Some(true),
            limit: Some(2),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        events
            .iter()
            .map(|e| e.event_id.clone())
            .collect::<Vec<_>>(),
        [3, 4, 5, 7, 9, 12].map(transaction_id)
    );
    assert_eq!(pages, 3);
    println!("\nSuspicious transactions (queried from the timeline, {pages} pages):");
    println!(
        "{:<8} {:<12} {:>12} {:<9} Reasons",
        "Ref", "Account", "USD", "Severity"
    );
    for event in &events {
        let a = &event.event.attributes;
        let cents = a["transaction"]["amountCents"].as_u64().unwrap();
        let amount = format!("{}.{:02}", cents / 100, cents % 100);
        let reasons = a["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{:<8} {:<12} {:>12} {:<9} {}",
            a["transaction"]["reference"].as_str().unwrap(),
            a["account"]["id"].as_str().unwrap(),
            amount,
            a["severity"].as_str().unwrap(),
            reasons
        );
    }
    println!(
        "\n12 unique inputs: 6 normal, 6 suspicious. One source duplicate and one output retry produced no extra event."
    );
    println!(
        "Six CDC records fsynced; outbox drained; default event retention is seven days from ingestion."
    );
    searches(&service, &scope).await?;
    println!(
        "\nAll assertions passed. In-memory source, checkpoints and projection are demo-only; the temporary disk ledger is removed on exit."
    );
    Ok(())
}
#[tokio::main]
async fn main() -> DemoResult<()> {
    let directory = std::env::temp_dir().join(format!("acteon-bank-demo-{}", uuid::Uuid::new_v4()));
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(30), simulate(&directory)).await;
    // simulate's handles are dropped before removing its uniquely owned directory.
    if directory.exists() {
        std::fs::remove_dir_all(directory)?;
    }
    result?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn threshold_and_device_rules_do_not_flag_large_known_payees_or_known_devices() {
        let mut state = DetectorState::default();
        let mut tx = transactions()[2].clone();
        tx.new_payee = false;
        tx.origin_country = "FR".into();
        assert!(
            SuspiciousActivityFilter::detect(&mut state, &tx)
                .unwrap()
                .is_none()
        );
        let mut below = transactions()[2].clone();
        below.amount_cents = LARGE_TRANSFER_CENTS - 1;
        assert!(
            SuspiciousActivityFilter::detect(&mut DetectorState::default(), &below)
                .unwrap()
                .is_none()
        );
        below.amount_cents += 1;
        assert!(
            SuspiciousActivityFilter::detect(&mut DetectorState::default(), &below)
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn velocity_window_is_inclusive_and_expires_old_transfers() {
        for (third_second, expected) in [(120, true), (121, false)] {
            let mut state = DetectorState::default();
            for (index, second) in [0, 60, third_second].into_iter().enumerate() {
                let mut tx = transactions()[0].clone();
                tx.id = transaction_id(u32::try_from(index + 1).unwrap());
                tx.occurred_at += TimeDelta::seconds(second);
                let event = SuspiciousActivityFilter::detect(&mut state, &tx).unwrap();
                assert_eq!(event.is_some(), index == 2 && expected);
            }
        }
    }
    #[test]
    fn duplicates_do_not_inflate_velocity_and_conflicting_ids_are_rejected() {
        let mut state = DetectorState::default();
        let mut tx = transactions()[0].clone();
        for _ in 0..4 {
            assert!(
                SuspiciousActivityFilter::detect(&mut state, &tx)
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(state.accounts[&tx.account_id].transfers.len(), 1);
        tx.amount_cents += 1;
        assert!(SuspiciousActivityFilter::detect(&mut state, &tx).is_err());
    }
    #[test]
    fn out_of_order_unique_transactions_are_rejected() {
        let mut state = DetectorState::default();
        let mut tx = transactions()[0].clone();
        SuspiciousActivityFilter::detect(&mut state, &tx).unwrap();
        tx.id = transaction_id(99);
        tx.occurred_at -= TimeDelta::seconds(1);
        assert!(SuspiciousActivityFilter::detect(&mut state, &tx).is_err());
    }
}
