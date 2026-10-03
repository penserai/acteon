//! Use the public Acteon HTTP API for all three telemetry consumers.
use super::AnyError;
use acteon_bus::BusBackend;
use acteon_client::ActeonClient;
use acteon_gateway::GatewayBuilder;
use acteon_server::{
    api::{self, AppState},
    bus_sessions::BusSessionRegistry,
    config::ConfigSnapshot,
};
use acteon_state::StateStore;
use acteon_state_memory::MemoryDistributedLock;
use std::sync::Arc;
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
