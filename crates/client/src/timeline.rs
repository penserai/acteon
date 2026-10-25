//! Typed helpers for the independent tenant timeline API.
use crate::{ActeonClient, Error, PlatformOperation};
use acteon_core::timeline::{
    ChangeReceipt, TimelineChange, TimelineEvent, TimelinePage, TimelineQuery, TimelineScope,
};
impl ActeonClient {
    pub async fn ingest_timeline_change(
        &self,
        scope: &TimelineScope,
        change: &TimelineChange,
    ) -> Result<ChangeReceipt, Error> {
        let body = serde_json::json!({"scope":scope,"change":change});
        let result = self
            .platform_request(PlatformOperation::TimelineIngest, &[], &[], Some(&body))
            .await?;
        serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))
    }
    pub async fn query_timeline(
        &self,
        scope: &TimelineScope,
        query: &TimelineQuery,
    ) -> Result<TimelinePage, Error> {
        let body = serde_json::json!({"scope":scope,"query":query});
        let result = self
            .platform_request(PlatformOperation::TimelineQuery, &[], &[], Some(&body))
            .await?;
        serde_json::from_value(result).map_err(|e| Error::Deserialization(e.to_string()))
    }
    pub async fn get_timeline_event(
        &self,
        scope: &TimelineScope,
        source: &str,
        event_id: &str,
    ) -> Result<Option<TimelineEvent>, Error> {
        match self
            .platform_request(
                PlatformOperation::TimelineGet,
                &[("source", source), ("eventId", event_id)],
                &[("namespace", &scope.namespace), ("tenant", &scope.tenant)],
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
