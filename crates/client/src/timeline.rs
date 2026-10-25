//! Typed helpers for the independent tenant timeline API.
use crate::{ActeonClient, Error, PlatformOperation};
use acteon_core::timeline::{
    ChangeReceipt, TimelineChange, TimelineEvent, TimelinePage, TimelineQuery,
};
impl ActeonClient {
    pub async fn ingest_timeline_change(
        &self,
        change: &TimelineChange,
    ) -> Result<ChangeReceipt, Error> {
        let body = serde_json::json!({"change":change});
        let result = self
            .platform_request(PlatformOperation::TimelineIngest, &[], &[], Some(&body))
            .await?;
        serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))
    }
    pub async fn query_timeline(&self, query: &TimelineQuery) -> Result<TimelinePage, Error> {
        let body = serde_json::json!({"query":query});
        let result = self
            .platform_request(PlatformOperation::TimelineQuery, &[], &[], Some(&body))
            .await?;
        serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))
    }
    pub async fn get_timeline_event(
        &self,
        source: &str,
        event_id: &str,
    ) -> Result<Option<TimelineEvent>, Error> {
        match self
            .platform_request(
                PlatformOperation::TimelineGet,
                &[("source", source), ("eventId", event_id)],
                &[],
                None,
            )
            .await
        {
            Ok(value) => serde_json::from_value(value)
                .map(Some)
                .map_err(|e| Error::Deserialization(e.to_string())),
            Err(Error::Http { status: 404, .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
