//! Billing-grade ads measurement and alerting simulation.
//!
//! Run with: `cargo run -p acteon-simulation --example ads_measurement_pipeline`

use std::collections::{BTreeMap, HashMap, HashSet};

use acteon_core::{Action, ActionOutcome};
use acteon_simulation::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const ALERT_RULES: &str = r"
rules:
  - name: page-revenue-and-delivery-incidents
    priority: 1
    condition:
      all:
        - field: action.action_type
          eq: ads_pipeline_alert
        - field: action.payload.severity
          eq: critical
    action:
      type: reroute
      target_provider: pagerduty
  - name: deduplicate-operational-warnings
    priority: 10
    condition:
      field: action.action_type
      eq: ads_pipeline_alert
    action:
      type: deduplicate
      ttl_seconds: 900
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EventKind {
    AdDecision,
    Impression,
    Start,
    FirstQuartile,
    Midpoint,
    ThirdQuartile,
    Complete,
    Viewability,
    Click,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdEvent {
    schema_version: u8,
    event_id: String,
    beacon_id: String,
    kind: EventKind,
    occurred_at_ms: i64,
    received_at_ms: i64,
    campaign_id: String,
    creative_id: String,
    opportunity_id: String,
    break_id: String,
    device_token: String,
    privacy_signal: String,
    measurement_source: Option<String>,
    viewable_percent: Option<u8>,
    viewable_ms: Option<u32>,
    price_micros: Option<u64>,
}

impl AdEvent {
    fn digest(&self) -> String {
        hex::encode(Sha256::digest(
            serde_json::to_vec(self).expect("event serialization"),
        ))
    }

    fn beacon_key(&self) -> String {
        format!(
            "{}:{}:{:?}:{}",
            self.opportunity_id, self.creative_id, self.kind, self.beacon_id
        )
    }
}

#[derive(Debug, Default)]
struct IngestionReport {
    received: usize,
    accepted: Vec<AdEvent>,
    duplicate_event_ids: usize,
    duplicate_beacons: usize,
    quarantined: Vec<(String, String)>,
    out_of_order_arrivals: usize,
    shadow_v2_rejections: usize,
}

fn validate(event: &AdEvent) -> Result<(), &'static str> {
    if !matches!(event.schema_version, 1 | 2) {
        return Err("unsupported_schema_version");
    }
    if event.event_id.is_empty()
        || event.beacon_id.is_empty()
        || event.campaign_id.is_empty()
        || event.creative_id.is_empty()
        || event.opportunity_id.is_empty()
    {
        return Err("missing_required_identity");
    }
    let minimized_identity = event.device_token.strip_prefix("h:").is_some_and(|digest| {
        digest.len() == 32 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    if !minimized_identity {
        return Err("non_minimized_device_identity");
    }
    if event.privacy_signal.is_empty() || event.privacy_signal.len() > 64 {
        return Err("invalid_privacy_signal");
    }
    if event.kind == EventKind::Viewability
        && (event.viewable_percent.is_none_or(|value| value > 100) || event.viewable_ms.is_none())
    {
        return Err("invalid_viewability_measurement");
    }
    if event.received_at_ms < event.occurred_at_ms {
        return Err("negative_ingest_latency");
    }
    Ok(())
}

fn validate_shadow_v2(event: &AdEvent) -> Result<(), &'static str> {
    validate(event)?;
    if event
        .measurement_source
        .as_deref()
        .is_none_or(str::is_empty)
    {
        return Err("v2_requires_measurement_source");
    }
    Ok(())
}

fn ingest(events: Vec<AdEvent>) -> IngestionReport {
    let mut report = IngestionReport::default();
    let mut ids: HashMap<String, String> = HashMap::new();
    let mut beacons = HashSet::new();
    let mut maximum_event_time = i64::MIN;
    for event in events {
        report.received += 1;
        if event.occurred_at_ms < maximum_event_time {
            report.out_of_order_arrivals += 1;
        }
        maximum_event_time = maximum_event_time.max(event.occurred_at_ms);
        if let Err(reason) = validate(&event) {
            report.quarantined.push((event.event_id, reason.into()));
            continue;
        }
        if validate_shadow_v2(&event).is_err() {
            report.shadow_v2_rejections += 1;
        }
        let digest = event.digest();
        if let Some(existing) = ids.get(&event.event_id) {
            if existing == &digest {
                report.duplicate_event_ids += 1;
            } else {
                report
                    .quarantined
                    .push((event.event_id, "event_id_payload_conflict".into()));
            }
            continue;
        }
        ids.insert(event.event_id.clone(), digest);
        if !beacons.insert(event.beacon_key()) {
            report.duplicate_beacons += 1;
            continue;
        }
        report.accepted.push(event);
    }
    report
        .accepted
        .sort_by_key(|event| (event.occurred_at_ms, event.event_id.clone()));
    report
}

#[derive(Debug, Default, Serialize)]
struct Measurement {
    decisions: usize,
    impressions: usize,
    starts: usize,
    completes: usize,
    viewable_impressions: usize,
    clicks: usize,
    attributed_clicks: usize,
    ingest_p95_ms: i64,
    billable_micros: u64,
    completion_rate: f64,
    viewability_rate: f64,
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        count_as_f64(numerator) / count_as_f64(denominator)
    }
}

fn count_as_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).expect("simulation counts fit in u32"))
}

fn measure(events: &[AdEvent]) -> Measurement {
    let mut result = Measurement::default();
    let mut impressions = HashMap::new();
    let mut latencies = Vec::with_capacity(events.len());
    for event in events {
        latencies.push(event.received_at_ms - event.occurred_at_ms);
        match event.kind {
            EventKind::AdDecision => result.decisions += 1,
            EventKind::Impression => {
                result.impressions += 1;
                result.billable_micros += event.price_micros.unwrap_or_default();
                impressions.insert(event.opportunity_id.clone(), event.occurred_at_ms);
            }
            EventKind::Start => result.starts += 1,
            EventKind::Complete => result.completes += 1,
            EventKind::Viewability => {
                if event.viewable_percent.is_some_and(|value| value >= 50)
                    && event.viewable_ms.is_some_and(|value| value >= 2_000)
                {
                    result.viewable_impressions += 1;
                }
            }
            EventKind::Click => {
                result.clicks += 1;
                if impressions
                    .get(&event.opportunity_id)
                    .is_some_and(|impression| {
                        event.occurred_at_ms >= *impression
                            && event.occurred_at_ms - *impression <= 30 * 60 * 1_000
                    })
                {
                    result.attributed_clicks += 1;
                }
            }
            EventKind::FirstQuartile | EventKind::Midpoint | EventKind::ThirdQuartile => {}
        }
    }
    latencies.sort_unstable();
    let index = latencies
        .len()
        .saturating_mul(95)
        .div_ceil(100)
        .saturating_sub(1);
    result.ingest_p95_ms = latencies.get(index).copied().unwrap_or_default();
    result.completion_rate = ratio(result.completes, result.starts);
    result.viewability_rate = ratio(result.viewable_impressions, result.impressions);
    result
}

#[derive(Debug, Default)]
struct BillingLedger {
    postings: HashMap<String, u64>,
}

impl BillingLedger {
    fn commit(&mut self, events: &[AdEvent]) -> usize {
        let mut inserted = 0;
        for event in events
            .iter()
            .filter(|event| event.kind == EventKind::Impression)
        {
            let amount = event.price_micros.unwrap_or_default();
            match self.postings.entry(event.event_id.clone()) {
                std::collections::hash_map::Entry::Vacant(posting) => {
                    posting.insert(amount);
                    inserted += 1;
                }
                std::collections::hash_map::Entry::Occupied(posting) => {
                    assert_eq!(
                        *posting.get(),
                        amount,
                        "a billing idempotency key cannot change value"
                    );
                }
            }
        }
        inserted
    }

    fn total_micros(&self) -> u64 {
        self.postings.values().sum()
    }
}

#[derive(Debug, Clone, Serialize)]
struct PipelineAlert {
    kind: &'static str,
    severity: &'static str,
    observed: f64,
    threshold: f64,
    unit: &'static str,
}

fn detect_alerts(
    ingestion: &IngestionReport,
    measurement: &Measurement,
    vendor_impressions: usize,
) -> Vec<PipelineAlert> {
    let drop_rate = 1.0 - ratio(measurement.impressions, measurement.decisions);
    let duplicate_rate = ratio(
        ingestion.duplicate_event_ids + ingestion.duplicate_beacons,
        ingestion.received,
    );
    let quarantine_rate = ratio(ingestion.quarantined.len(), ingestion.received);
    let billing_drift = count_as_f64(measurement.impressions.abs_diff(vendor_impressions))
        / count_as_f64(measurement.impressions.max(1));
    [
        PipelineAlert {
            kind: "impression_drop",
            severity: "critical",
            observed: drop_rate,
            threshold: 0.05,
            unit: "ratio",
        },
        PipelineAlert {
            kind: "retry_storm_duplicates",
            severity: "warning",
            observed: duplicate_rate,
            threshold: 0.05,
            unit: "ratio",
        },
        PipelineAlert {
            kind: "ingest_latency_p95",
            severity: "warning",
            observed: f64::from(
                i32::try_from(measurement.ingest_p95_ms).expect("simulation latency fits in i32"),
            ),
            threshold: 2_000.0,
            unit: "ms",
        },
        PipelineAlert {
            kind: "schema_quarantine",
            severity: "warning",
            observed: quarantine_rate,
            threshold: 0.01,
            unit: "ratio",
        },
        PipelineAlert {
            kind: "completion_regression",
            severity: "warning",
            observed: measurement.completion_rate,
            threshold: 0.75,
            unit: "minimum_ratio",
        },
        PipelineAlert {
            kind: "billing_reconciliation_drift",
            severity: "critical",
            observed: billing_drift,
            threshold: 0.02,
            unit: "ratio",
        },
    ]
    .into_iter()
    .filter(|alert| match alert.unit {
        "minimum_ratio" => alert.observed < alert.threshold,
        _ => alert.observed > alert.threshold,
    })
    .collect()
}

fn event(opportunity: u32, kind: EventKind, sequence: u32, version: u8, delay_ms: i64) -> AdEvent {
    let occurred_at_ms =
        1_800_000_000_000 + i64::from(opportunity) * 10_000 + i64::from(sequence) * 500;
    AdEvent {
        schema_version: version,
        event_id: format!("evt-{opportunity}-{sequence}"),
        beacon_id: format!("beacon-{opportunity}-{sequence}"),
        kind,
        occurred_at_ms,
        received_at_ms: occurred_at_ms + delay_ms,
        campaign_id: "campaign-live-final".into(),
        creative_id: "creative-ctv-30s".into(),
        opportunity_id: format!("opportunity-{opportunity}"),
        break_id: format!("break-{}", opportunity / 4),
        device_token: format!("h:{opportunity:032x}"),
        privacy_signal: "gpp:example-consented".into(),
        measurement_source: (version == 2).then(|| "omid-1.5".into()),
        viewable_percent: None,
        viewable_ms: None,
        price_micros: (kind == EventKind::Impression).then_some(2_500),
    }
}

fn fixture() -> Vec<AdEvent> {
    let mut events = Vec::new();
    for opportunity in 0..12 {
        let version = if opportunity % 2 == 0 { 2 } else { 1 };
        let delay = if opportunity >= 8 { 6_000 } else { 150 };
        events.push(event(opportunity, EventKind::AdDecision, 0, version, delay));
        if opportunity < 10 {
            events.push(event(opportunity, EventKind::Impression, 1, version, delay));
        }
        if opportunity < 8 {
            events.push(event(opportunity, EventKind::Start, 2, version, delay));
        }
        if opportunity < 6 {
            events.push(event(
                opportunity,
                EventKind::FirstQuartile,
                3,
                version,
                delay,
            ));
            events.push(event(opportunity, EventKind::Midpoint, 4, version, delay));
        }
        if opportunity < 5 {
            events.push(event(
                opportunity,
                EventKind::ThirdQuartile,
                5,
                version,
                delay,
            ));
            events.push(event(opportunity, EventKind::Complete, 6, version, delay));
        }
        if opportunity < 6 {
            let mut viewability = event(opportunity, EventKind::Viewability, 7, version, delay);
            viewability.viewable_percent = Some(75);
            viewability.viewable_ms = Some(2_500);
            events.push(viewability);
        }
        if opportunity < 2 {
            events.push(event(opportunity, EventKind::Click, 8, version, delay));
        }
    }

    let exact_retry = events
        .iter()
        .find(|event| event.kind == EventKind::Impression)
        .unwrap()
        .clone();
    events.push(exact_retry);
    for opportunity in 1..=3 {
        let mut retry = events
            .iter()
            .find(|event| {
                event.kind == EventKind::Impression
                    && event.opportunity_id == format!("opportunity-{opportunity}")
            })
            .unwrap()
            .clone();
        retry.event_id = format!("retry-{opportunity}");
        events.push(retry);
    }
    let mut conflict = events[0].clone();
    conflict.campaign_id = "campaign-tampered".into();
    events.push(conflict);
    let mut unknown = event(20, EventKind::Impression, 1, 1, 100);
    unknown.schema_version = 99;
    events.push(unknown);
    let mut raw_device = event(21, EventKind::Impression, 1, 2, 100);
    raw_device.device_token = "raw-device-identifier".into();
    events.push(raw_device);
    let late = events.remove(2);
    events.push(late);
    events
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ingestion = ingest(fixture());
    let measurement = measure(&ingestion.accepted);
    let mut ledger = BillingLedger::default();
    let first_postings = ledger.commit(&ingestion.accepted);
    let replay_postings = ledger.commit(&ingestion.accepted);
    assert_eq!(first_postings, measurement.impressions);
    assert_eq!(replay_postings, 0);
    assert_eq!(ledger.total_micros(), measurement.billable_micros);

    let vendor_impressions = 8;
    let alerts = detect_alerts(&ingestion, &measurement, vendor_impressions);
    assert_eq!(alerts.len(), 6);
    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(2)
            .shared_state(true)
            .add_recording_providers(["slack", "pagerduty"])
            .add_rule_yaml(ALERT_RULES)
            .build(),
    )
    .await?;
    let mut outcomes = BTreeMap::new();
    let mut duplicate_warning_suppressed = false;
    for alert in &alerts {
        let action = Action::new(
            "ads-measurement",
            "streaming-tv",
            "slack",
            "ads_pipeline_alert",
            serde_json::to_value(alert)?,
        )
        .with_dedup_key(format!("live-final:{}", alert.kind));
        let outcome = harness.dispatch(&action).await?;
        let outcome_label = match (&outcome, alert.severity) {
            (ActionOutcome::Executed(_), "warning") => "executed:slack".to_owned(),
            (ActionOutcome::Rerouted { new_provider, .. }, "critical")
                if new_provider == "pagerduty" =>
            {
                "rerouted:pagerduty".to_owned()
            }
            _ => panic!(
                "unexpected dispatch outcome for {} ({}): {outcome:?}",
                alert.kind, alert.severity
            ),
        };
        outcomes.insert(alert.kind, outcome_label);
        if alert.kind == "completion_regression" {
            let duplicate = harness.dispatch_to(1, &action).await?;
            duplicate_warning_suppressed = matches!(duplicate, ActionOutcome::Deduplicated);
            assert!(duplicate_warning_suppressed);
        }
    }
    assert_eq!(harness.provider("pagerduty").unwrap().call_count(), 2);
    assert_eq!(harness.provider("slack").unwrap().call_count(), 4);
    let result = serde_json::json!({
        "ingestion": {
            "received": ingestion.received,
            "accepted": ingestion.accepted.len(),
            "duplicate_event_ids": ingestion.duplicate_event_ids,
            "duplicate_beacons": ingestion.duplicate_beacons,
            "quarantined": ingestion.quarantined,
            "out_of_order_arrivals": ingestion.out_of_order_arrivals,
            "shadow_v2_rejections": ingestion.shadow_v2_rejections,
        },
        "measurement": measurement,
        "billing": {
            "first_commit_postings": first_postings,
            "replay_postings": replay_postings,
            "ledger_total_micros": ledger.total_micros(),
            "vendor_impressions": vendor_impressions,
        },
        "alerts": alerts,
        "alert_outcomes": outcomes,
        "delivery": {
            "pagerduty_calls": harness.provider("pagerduty").unwrap().call_count(),
            "slack_calls": harness.provider("slack").unwrap().call_count(),
            "cross_node_duplicate_suppressed": duplicate_warning_suppressed,
        }
    });
    println!("{}", serde_json::to_string_pretty(&result)?);
    println!("\nADS MEASUREMENT PIPELINE SIMULATION PASSED");
    harness.teardown().await?;
    Ok(())
}
