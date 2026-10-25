//! Bounded aggregate metrics; no tenant identities or payloads become metric labels.
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
#[derive(Default)]
pub struct TimelineMetrics {
    pub accepted: AtomicU64,
    pub quarantined: AtomicU64,
    pub duplicates: AtomicU64,
    pub stale: AtomicU64,
    pub failures: AtomicU64,
    pub query_count: AtomicU64,
    pub query_micros: AtomicU64,
    pub ingest_micros: AtomicU64,
}
impl TimelineMetrics {
    pub fn render(&self) -> String {
        let mut text = String::new();
        for (name, value) in [
            ("accepted_total", &self.accepted),
            ("quarantined_total", &self.quarantined),
            ("duplicates_total", &self.duplicates),
            ("stale_total", &self.stale),
            ("failures_total", &self.failures),
            ("queries_total", &self.query_count),
            ("query_duration_microseconds_total", &self.query_micros),
            ("ingest_duration_microseconds_total", &self.ingest_micros),
        ] {
            let _ = writeln!(
                text,
                "# TYPE acteon_timeline_{name} counter\nacteon_timeline_{name} {}",
                value.load(Ordering::Relaxed)
            );
        }
        text
    }
}
