use crate::{Result, TimelineEvent, TimelinePage, TimelineQuery, TimelineScope};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A versioned projection, including terminal markers. Retain markers to fence delayed writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Projection {
    pub scope: TimelineScope,
    pub source: String,
    pub event_id: String,
    pub revision: u64,
    pub expires_at: DateTime<Utc>,
    pub event: Option<TimelineEvent>,
}
impl Projection {
    pub fn id(&self) -> String {
        identity(&self.scope, &self.source, &self.event_id)
    }
}
pub fn identity(scope: &TimelineScope, source: &str, event_id: &str) -> String {
    crate::digest(&(scope, source, event_id))
}
#[async_trait]
pub trait TimelineStore: Send + Sync {
    /// Idempotent and version-fenced, including across concurrent writers.
    async fn apply(&self, projection: &Projection) -> Result<()>;
    async fn get(
        &self,
        scope: &TimelineScope,
        source: &str,
        event_id: &str,
    ) -> Result<Option<TimelineEvent>>;
    async fn query(&self, scope: &TimelineScope, query: &TimelineQuery) -> Result<TimelinePage>;
    /// Remove expired payloads while retaining compact version fences.
    async fn cleanup(&self, now: DateTime<Utc>) -> Result<u64>;
}
