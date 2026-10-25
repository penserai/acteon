use crate::{
    Result, TimelineEvent, TimelinePage, TimelineQuery, TimelineScope, TimelineStore,
    search::{PreparedQuery, SearchFields},
    store::{Projection, identity},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct MemoryTimelineStore {
    rows: RwLock<HashMap<String, Projection>>,
}
#[async_trait]
impl TimelineStore for MemoryTimelineStore {
    async fn apply(&self, p: &Projection) -> Result<()> {
        let mut rows = self.rows.write().await;
        let id = p.id();
        if rows.get(&id).is_none_or(|old| old.revision < p.revision) {
            rows.insert(id, p.clone());
        }
        Ok(())
    }
    async fn get(
        &self,
        scope: &TimelineScope,
        source: &str,
        event_id: &str,
    ) -> Result<Option<TimelineEvent>> {
        Ok(self
            .rows
            .read()
            .await
            .get(&identity(scope, source, event_id))
            .and_then(|p| p.event.clone())
            .filter(|e| e.expires_at > Utc::now()))
    }
    async fn query(&self, scope: &TimelineScope, query: &TimelineQuery) -> Result<TimelinePage> {
        let prepared = PreparedQuery::new(scope, query)?;
        let mut events = vec![];
        for p in self.rows.read().await.values() {
            if &p.scope != scope {
                continue;
            }
            if let Some(e) = &p.event {
                let fields = SearchFields::from_event(&e.event)?;
                if crate::search::matches(e, &fields, query, &prepared, Utc::now()) {
                    events.push(e.clone());
                }
            }
        }
        events.sort_by(|a, b| {
            (a.event.occurred_at, &a.source, &a.event_id).cmp(&(
                b.event.occurred_at,
                &b.source,
                &b.event_id,
            ))
        });
        if !prepared.ascending {
            events.reverse();
        }
        Ok(prepared.page(events))
    }
    async fn cleanup(&self, now: DateTime<Utc>) -> Result<u64> {
        let mut count = 0;
        for p in self.rows.write().await.values_mut() {
            if p.expires_at <= now && p.event.take().is_some() {
                count += 1;
            }
        }
        Ok(count)
    }
}
