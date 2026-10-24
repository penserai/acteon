//! Timeline lifecycle, independent from action audit.
use crate::{config::ActeonConfig, error::ServerError};
use acteon_timeline::{
    TimelineService, TimelineStore,
    ledger::{DiskLedger, LedgerArchiveSink},
};
use std::sync::Arc;

pub async fn create(
    config: &ActeonConfig,
    state: Arc<dyn acteon_state::StateStore>,
    client: reqwest::Client,
) -> Result<Option<Arc<TimelineService>>, ServerError> {
    if !config.timeline.enabled {
        return Ok(None);
    }
    if config.state.backend == "memory" && !config.timeline.allow_memory {
        return Err(ServerError::Config(
            "timeline requires durable shared state; allow_memory is for development only".into(),
        ));
    }
    let mut settings = config.timeline.clone();
    if settings.backend == "audit" {
        settings.backend.clone_from(&config.audit.backend);
        if settings.url.is_none() {
            settings.url.clone_from(&config.audit.url);
        }
    }
    let fail = |e: acteon_timeline::Error| ServerError::Config(e.to_string());
    let store: Arc<dyn TimelineStore> = match settings.backend.as_str() {
        "memory" if settings.allow_memory => {
            Arc::new(acteon_timeline::memory::MemoryTimelineStore::default())
        }
        "elasticsearch" => Arc::new(
            acteon_timeline::elasticsearch::ElasticsearchTimelineStore::new(
                client,
                settings
                    .url
                    .as_deref()
                    .ok_or_else(|| ServerError::Config("timeline URL required".into()))?,
                &settings.collection,
            )
            .await
            .map_err(fail)?,
        ),
        "clickhouse" => Arc::new(
            acteon_timeline::clickhouse::ClickHouseTimelineStore::new(
                client,
                settings
                    .url
                    .as_deref()
                    .ok_or_else(|| ServerError::Config("timeline URL required".into()))?,
                &settings.collection,
            )
            .await
            .map_err(fail)?,
        ),
        other => {
            return Err(ServerError::Config(format!(
                "unsupported timeline backend: {other}"
            )));
        }
    };
    let ledger = Arc::new(
        DiskLedger::open(&settings.ledger.directory, settings.ledger.max_bytes).map_err(fail)?,
    );
    let archive: Option<Arc<dyn LedgerArchiveSink>> =
        if let Some(archive_config) = &settings.archive {
            #[cfg(feature = "timeline-archive")]
            {
                Some(Arc::new(
                    acteon_timeline::archive::ParquetArchive::new(archive_config).map_err(fail)?,
                ))
            }
            #[cfg(not(feature = "timeline-archive"))]
            {
                let _ = archive_config;
                return Err(ServerError::Config(
                    "archive requires the timeline-archive feature".into(),
                ));
            }
        } else {
            None
        };
    #[cfg(not(feature = "bus"))]
    if !settings.sources.is_empty() {
        return Err(ServerError::Config(
            "timeline sources require the bus feature".into(),
        ));
    }
    Ok(Some(Arc::new(
        TimelineService::new(store, ledger, state, settings, archive).map_err(fail)?,
    )))
}

pub fn spawn_maintenance(service: Arc<TimelineService>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(error) = recover_replays(&service).await {
                tracing::error!(%error, "timeline replay recovery failed");
            }
            if let Err(error) = service.cleanup_archive().await {
                tracing::error!(%error, "timeline archive maintenance failed");
            }
            if let Err(error) = service.recover_all().await {
                tracing::error!(%error,"timeline recovery failed");
            }
            if let Err(error) = service.store.cleanup(chrono::Utc::now()).await {
                tracing::error!(%error,"timeline projection cleanup failed");
            }
            if let Err(error) = service.ledger.cleanup(chrono::Utc::now()).await {
                tracing::error!(%error,"timeline ledger cleanup failed");
            }
        }
    })
}

async fn recover_replays(service: &TimelineService) -> Result<(), acteon_timeline::Error> {
    let mut cursor = None;
    loop {
        let page = service
            .state
            .scan_keys_by_kind_page(
                acteon_state::KeyKind::Custom("timeline_replay".into()),
                cursor.as_deref(),
                100,
            )
            .await?;
        for (_, value) in page.entries {
            let mut job: crate::api::timeline::ReplayStatus = serde_json::from_str(&value)?;
            if job.status != "running" {
                continue;
            }
            match service.replay_local(&job.scope).await {
                Ok(count) => {
                    job.status = "complete".into();
                    job.processed = count;
                }
                Err(error) => {
                    tracing::error!(%error, "timeline replay recovery failed");
                    job.status = "failed".into();
                    job.error = Some("replay failed; inspect server logs".into());
                }
            }
            let key = crate::api::timeline::replay_key(&job.scope, &job.id);
            service
                .state
                .set(&key, &serde_json::to_string(&job)?, None)
                .await?;
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            return Ok(());
        }
    }
}
