//! Use the public Acteon HTTP API for all three telemetry consumers.
use super::{AnyError, NAMESPACE, TENANT, error, windowing::SignalSource};
use acteon_bus::{BusBackend, BusMessage};
use acteon_client::{ActeonClient, CreateSubscription, ReceiveRequest, SessionDelivery};
use acteon_gateway::GatewayBuilder;
use acteon_server::{
    api::{self, AppState},
    bus_sessions::BusSessionRegistry,
    config::ConfigSnapshot,
};
use acteon_state::StateStore;
use acteon_state_memory::MemoryDistributedLock;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::sync::RwLock;

pub struct HttpServer {
    pub client: ActeonClient,
    registry: Arc<BusSessionRegistry>,
    task: tokio::task::JoinHandle<()>,
}
impl HttpServer {
    pub async fn start(
        backend: Arc<dyn BusBackend>,
        store: Arc<dyn StateStore>,
    ) -> Result<Self, AnyError> {
        let gateway = GatewayBuilder::new()
            .state(store)
            .lock(Arc::new(MemoryDistributedLock::new()))
            .build()?;
        let metrics = gateway.metrics_arc();
        let registry = Arc::new(BusSessionRegistry::default());
        let state = AppState {
            gateway: Arc::new(RwLock::new(gateway)),
            metrics,
            audit: None,
            analytics: None,
            auth: None,
            rate_limiter: None,
            embedding: None,
            embedding_metrics: None,
            connection_registry: None,
            a2a_discovery_cache: Arc::new(api::a2a_discovery_cache::DiscoveryCache::new()),
            dispatch_semaphore: Arc::new(tokio::sync::Semaphore::new(100)),
            config: ConfigSnapshot::default(),
            static_quotas: None,
            static_templates: None,
            ui_path: None,
            ui_enabled: false,
            cors_allowed_origins: vec![],
            signature_verifier: None,
            replay_protection: None,
            #[cfg(feature = "swarm")]
            swarm_registry: None,
            bus_backend: Some(backend),
            bus_schema_validator: acteon_bus::SchemaValidator::new(),
            bus_sessions: Arc::clone(&registry),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = ActeonClient::new(format!("http://{}", listener.local_addr()?));
        let task = tokio::spawn(async move {
            axum::serve(listener, api::router(state))
                .await
                .expect("HTTP simulation server");
        });
        Ok(Self {
            client,
            registry,
            task,
        })
    }
    pub async fn stop(self) {
        self.registry.shutdown().await;
        self.task.abort();
        let _ = self.task.await;
    }
}

pub struct HttpSources {
    pub ids: BTreeMap<SignalSource, String>,
    pub sessions: BTreeMap<SignalSource, String>,
    pub groups: BTreeMap<SignalSource, String>,
    seen: BTreeMap<SignalSource, BTreeSet<String>>,
}
impl HttpSources {
    pub async fn create(
        client: &ActeonClient,
        topics: &BTreeMap<SignalSource, acteon_core::Topic>,
        run: &str,
    ) -> Result<Self, AnyError> {
        let mut ids = BTreeMap::new();
        let mut groups = BTreeMap::new();
        for source in SignalSource::ALL {
            let id = format!("neural-{}-{run}", source.as_str());
            let sub = client
                .create_bus_subscription(&CreateSubscription {
                    id: id.clone(),
                    topic: topics[&source].kafka_topic_name(),
                    namespace: NAMESPACE.into(),
                    tenant: TENANT.into(),
                    starting_offset: Some("earliest".into()),
                    receipt_required: true,
                    ack_timeout_ms: Some(60_000),
                    ..Default::default()
                })
                .await?;
            groups.insert(source, sub.consumer_group);
            ids.insert(source, id);
        }
        let mut sources = Self {
            ids,
            sessions: BTreeMap::new(),
            groups,
            seen: BTreeMap::new(),
        };
        sources.open(client).await?;
        Ok(sources)
    }
    pub async fn open(&mut self, client: &ActeonClient) -> Result<(), AnyError> {
        self.sessions.clear();
        self.seen.clear();
        for source in SignalSource::ALL {
            let session = client
                .open_bus_session(
                    NAMESPACE,
                    TENANT,
                    &self.ids[&source],
                    &uuid::Uuid::new_v4().to_string(),
                )
                .await?;
            self.sessions.insert(source, session.session_id);
            self.seen.insert(source, BTreeSet::new());
        }
        Ok(())
    }
    pub async fn next(&mut self, client: &ActeonClient) -> Result<(BusMessage, String), AnyError> {
        let polls = SignalSource::ALL
            .into_iter()
            .map(|source| {
                let id = &self.ids[&source];
                let session = &self.sessions[&source];
                let seen = &self.seen[&source];
                Box::pin(async move {
                    loop {
                        let batch = client
                            .receive_bus_session(
                                NAMESPACE,
                                TENANT,
                                id,
                                session,
                                &ReceiveRequest {
                                    max_messages: 256,
                                    wait_ms: 1000,
                                },
                            )
                            .await?;
                        if let Some(delivery) = batch
                            .deliveries
                            .into_iter()
                            .find(|d| !seen.contains(&d.receipt_id))
                        {
                            return Ok::<_, AnyError>((source, delivery));
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
            })
            .collect::<Vec<_>>();
        let (source, delivery): (_, SessionDelivery) = futures::future::select_all(polls).await.0?;
        self.seen
            .get_mut(&source)
            .ok_or_else(|| error("missing HTTP source"))?
            .insert(delivery.receipt_id.clone());
        Ok((
            serde_json::from_value(delivery.message)?,
            delivery.receipt_id,
        ))
    }
    pub async fn validate(
        &self,
        client: &ActeonClient,
        receipts: &BTreeMap<SignalSource, Vec<String>>,
        topics: &BTreeMap<SignalSource, acteon_core::Topic>,
    ) -> Result<Vec<acteon_bus::StreamPosition>, AnyError> {
        let mut positions = Vec::new();
        for source in SignalSource::ALL {
            let response = client
                .validate_bus_receipts(
                    NAMESPACE,
                    TENANT,
                    &self.ids[&source],
                    &self.sessions[&source],
                    &receipts[&source],
                )
                .await?;
            for p in response.positions {
                positions.push(acteon_bus::StreamPosition {
                    lane: acteon_bus::StreamPositionLane {
                        source: source.as_str().into(),
                        consumer_group: response.consumer_group.clone(),
                        topic: topics[&source].kafka_topic_name(),
                        partition: p.partition,
                    },
                    offset: p.offset,
                });
            }
        }
        Ok(positions)
    }
    pub async fn acknowledge(
        &self,
        client: &ActeonClient,
        receipts: &BTreeMap<SignalSource, Vec<String>>,
    ) -> Result<(), AnyError> {
        for source in SignalSource::ALL {
            client
                .acknowledge_bus_receipts(
                    NAMESPACE,
                    TENANT,
                    &self.ids[&source],
                    &self.sessions[&source],
                    &receipts[&source],
                )
                .await?;
        }
        Ok(())
    }
}
