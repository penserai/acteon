//! Real source and destination Acteon processes over HTTP with Redis restart.
#![cfg(feature = "redis")]
#![allow(clippy::too_many_lines)]

use std::{
    fs,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use acteon_core::{
    AgentCard, AgentCardInterface, PrincipalIdentity, PrincipalKind, ResourceKind, ResourceRef,
    Skill, TaskMessage, TaskRole,
};
use acteon_governance::{
    RootBudgetLimits,
    context::AcceptedEffect,
    federation::{
        FederationCapabilitySigner, FederationEnvelopeSigner, FederationGrantCeiling,
        FederationTrust,
    },
};
use acteon_server::{
    config::{
        ExecutionAuthorityConfig, ExecutionFederationConfig, FederationCapabilityPublisherConfig,
        FederationExportConfig, FederationExportEpochConfig, FederationPeerCapabilityConfig,
        FederationPeerConfig, ProviderConfig,
    },
    execution_authority::{ExecutionProviderRegistry, PreparedExecutionScope},
    provider_factory::StaticWebhook,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use axum::{Json, Router, routing::post};
use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Response, StatusCode, header},
    response::IntoResponse,
    routing::any,
};
use serde_json::{Value, json};

const NAMESPACE: &str = "prod";
const TENANT: &str = "acme";
const SOURCE_DOMAIN: &str = "acteon://source/prod";
const DESTINATION_DOMAIN: &str = "acteon://destination/prod";

struct BinaryServer {
    process: Child,
    directory: std::path::PathBuf,
    url: String,
    environment: Vec<(String, String)>,
}

impl BinaryServer {
    fn start(
        configuration: &Value,
        authentication: &str,
        environment: Vec<(String, String)>,
    ) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "acteon-federation-two-server-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join("acteon.toml"),
            toml::to_string(configuration).unwrap(),
        )
        .unwrap();
        fs::write(directory.join("auth.toml"), authentication).unwrap();
        let port = configuration["server"]["port"].as_u64().unwrap();
        let process = Self::launch(&directory, &environment);
        Self {
            process,
            directory,
            url: format!("http://127.0.0.1:{port}"),
            environment,
        }
    }

    fn launch(directory: &std::path::Path, environment: &[(String, String)]) -> Child {
        let log = fs::File::create(directory.join("server.log")).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_acteon-server"));
        command
            .arg("-c")
            .arg(directory.join("acteon.toml"))
            .env("ACTEON_AUTH_KEY", "11".repeat(32))
            .env(
                "ACTEON_AUTH_AUTHORITY_KEY",
                "federation-test-auth-authority-key",
            )
            .env(
                "ACTEON_EXECUTION_AUTHORITY_KEY",
                "federation-test-execution-authority-key",
            )
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log));
        for (name, value) in environment {
            command.env(name, value);
        }
        command.spawn().unwrap()
    }

    async fn ready(&mut self, client: &reqwest::Client) {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                assert!(self.process.try_wait().unwrap().is_none(), "{}", self.log());
                if client
                    .get(format!("{}/health", self.url))
                    .send()
                    .await
                    .is_ok_and(|response| response.status().is_success())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
    }

    fn restart(&mut self) {
        self.process.kill().unwrap();
        self.process.wait().unwrap();
        self.process = Self::launch(&self.directory, &self.environment);
    }

    fn log(&self) -> String {
        fs::read_to_string(self.directory.join("server.log")).unwrap_or_default()
    }
}

impl Drop for BinaryServer {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn free_port() -> u16 {
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

async fn counted_webhook() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = Router::new().route(
        "/incident",
        post(move || {
            let observed = observed.clone();
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Json(json!({"delivered": true}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}/incident"), calls, task)
}

struct ResponseLossProxy {
    upstream: String,
    client: reqwest::Client,
    lose_next_invocation: std::sync::atomic::AtomicBool,
    invocations: AtomicUsize,
}

async fn response_loss_proxy(
    upstream: String,
) -> (String, Arc<ResponseLossProxy>, tokio::task::JoinHandle<()>) {
    let state = Arc::new(ResponseLossProxy {
        upstream,
        client: reqwest::Client::new(),
        lose_next_invocation: std::sync::atomic::AtomicBool::new(true),
        invocations: AtomicUsize::new(0),
    });
    let app = Router::new()
        .fallback(any(proxy_federation_request))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), state, task)
}

async fn proxy_federation_request(
    State(state): State<Arc<ResponseLossProxy>>,
    request: Request,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| "/".into(), ToString::to_string);
    let invocation = parts.method == axum::http::Method::POST
        && parts.uri.path().contains("/federation/agents/")
        && parts.uri.path().ends_with("/message:send");
    let Ok(body) = to_bytes(body, 2 * 1024 * 1024).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let mut forwarded = state
        .client
        .request(parts.method, format!("{}{path}", state.upstream));
    for (name, value) in &parts.headers {
        if !matches!(
            name,
            &header::HOST | &header::CONTENT_LENGTH | &header::CONNECTION
        ) {
            forwarded = forwarded.header(name, value);
        }
    }
    let Ok(upstream) = forwarded.body(body).send().await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let status = upstream.status();
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let Ok(body) = upstream.bytes().await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    if invocation {
        state.invocations.fetch_add(1, Ordering::SeqCst);
        if state.lose_next_invocation.swap(false, Ordering::SeqCst) {
            return StatusCode::BAD_GATEWAY.into_response();
        }
    }
    let mut response = Response::builder().status(status);
    if let Some(content_type) = content_type {
        response = response.header(header::CONTENT_TYPE, content_type);
    }
    response.body(Body::from(body)).unwrap()
}

fn provider_registry(webhook: &str) -> ExecutionProviderRegistry {
    let configuration: ProviderConfig = toml::from_str(&format!(
        "name='incident'\ntype='webhook'\nurl={webhook:?}\ninternal_hosts=['127.0.0.1']\n"
    ))
    .unwrap();
    let factory = StaticWebhook::build(&configuration, None).unwrap();
    let mut registry = ExecutionProviderRegistry::default();
    registry
        .register(factory.provider(), Some(factory))
        .unwrap();
    registry
}

fn destination_execution(
    registry: &ExecutionProviderRegistry,
) -> (ExecutionAuthorityConfig, FederationTrust) {
    let limits = json!({
        "max_units": 5,
        "max_concurrent": 2,
        "deadline_ms": 4_102_444_800_000_i64
    });
    let source = json!({"id": "agent/source", "kind": "agent"});
    let worker = json!({"id": "agent/worker", "kind": "agent"});
    let operator = json!({"id": "operator", "kind": "human"});
    let route = json!({"provider": "incident", "action_type": "execute"});
    let mut card = AgentCard::new("worker", NAMESPACE, TENANT, "Remote worker", "1");
    card.skills.push(Skill::new("triage"));
    card.interfaces.push(AgentCardInterface {
        kind: "rest".into(),
        url: "https://agents.example/worker".into(),
    });
    let mut execution: ExecutionAuthorityConfig = serde_json::from_value(json!({
        "agent_driver": {"enabled": true, "poll_interval_ms": 50, "max_parallel": 2, "scan_batch_size": 8},
        "scopes": [{
            "namespace": NAMESPACE, "tenant": TENANT, "bootstrap": true,
            "publisher": operator, "subjects": [operator, source, worker],
            "routes": [route], "valid_from_ms": 0, "credential_limits": limits,
            "root_max_units": 5, "root_max_concurrent": 1, "root_lifetime_ms": 60_000,
            "agent_services": [{
                "card": card, "principal": worker, "skill": "triage",
                "endpoint": "https://agents.example/worker", "endpoint_id": "worker-api",
                "route": route, "recipient_key_env": "ACTEON_DESTINATION_RECIPIENT",
                "recipient_permits": [{"id": "worker-provider", "accepted_revision": 1}],
                "grants": [{"id": "source-worker", "revision": 1, "source": source,
                    "source_permits": [{"id": "source-service", "accepted_revision": 1}],
                    "valid_from_ms": 0, "limits": limits, "max_depth": 4}]
            }],
            "permits": [
                {"id": "source-service", "revision": 1, "subject": source, "routes": [],
                    "agents": ["worker"], "valid_from_ms": 0, "limits": limits},
                {"id": "worker-provider", "revision": 1, "subject": worker, "routes": [route],
                    "valid_from_ms": 0, "limits": limits}
            ],
            "managers": []
        }]
    }))
    .unwrap();
    let prepared = registry
        .prepare(&execution, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let shape = destination_shape(&prepared[0]);
    let source_principal = PrincipalIdentity::new("agent/source", PrincipalKind::Agent).unwrap();
    let target_principal = PrincipalIdentity::new("agent/worker", PrincipalKind::Agent).unwrap();
    let envelope_signer = FederationEnvelopeSigner::from_secret([41; 32]);
    let trust = FederationTrust {
        id: "source-prod".into(),
        revision: 1,
        foreign_domain: SOURCE_DOMAIN.into(),
        local_audience: DESTINATION_DOMAIN.into(),
        key_id: "source-envelope-v1".into(),
        verifying_key: envelope_signer.verifying_key_hex(),
        minimum_issuer_epoch: 1,
        max_revocation_staleness_ms: 60_000,
        max_clock_skew_ms: 30_000,
        valid_from_ms: 0,
        deadline_ms: 4_102_444_800_000,
        approved: vec![FederationGrantCeiling {
            source: source_principal.clone(),
            target: target_principal,
            agent_resource: shape.agent_resource.clone(),
            binding_digest: shape.binding_digest.clone(),
            skill: "triage".into(),
            ingress_effect: shape.ingress_effect.clone(),
            effects: shape.effects.clone(),
            limits: RootBudgetLimits {
                max_units: 3,
                max_concurrent: 1,
                deadline_ms: 4_102_444_800_000,
            },
            max_depth: 2,
        }],
    };
    execution.scopes[0].federation = Some(ExecutionFederationConfig {
        local_audience: DESTINATION_DOMAIN.into(),
        capability_publisher: Some(FederationCapabilityPublisherConfig {
            key_id: "destination-capability-v1".into(),
            verifying_key: FederationCapabilitySigner::from_secret([42; 32]).verifying_key_hex(),
            signer_key_env: "ACTEON_DESTINATION_CAPABILITY_SIGNER".into(),
            issuer_epoch: 1,
            lifetime_ms: 300_000,
            protocol_revision: "acteon-federation-v1".into(),
            authentication_schemes: vec!["bearer".into()],
            peer_finality: false,
        }),
        trusts: vec![trust.clone()],
        peers: vec![FederationPeerConfig {
            foreign_domain: SOURCE_DOMAIN.into(),
            authentication_principal: source_principal,
            trust_ids: vec![trust.id.clone()],
        }],
        exports: vec![],
    });
    execution.scopes[0].managers.push(
        serde_json::from_value(json!({
            "principal": {"id": "operator", "kind": "human"},
            "subjects": [{"id": "agent/source", "kind": "agent"}],
            "routes": [], "valid_from_ms": 0,
            "limits": {"max_units": 5, "max_concurrent": 2,
                "deadline_ms": 4_102_444_800_000_i64},
            "can_manage_federation": true,
            "federation_trust_ids": ["source-prod"]
        }))
        .unwrap(),
    );
    registry
        .prepare(&execution, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    (execution, trust)
}

struct DestinationShape {
    agent_resource: ResourceRef,
    binding_digest: String,
    ingress_effect: AcceptedEffect,
    effects: Vec<AcceptedEffect>,
}

fn destination_shape(scope: &PreparedExecutionScope) -> DestinationShape {
    let ingress = scope
        .agent_service_ingress_effect("worker")
        .unwrap()
        .clone();
    DestinationShape {
        agent_resource: ingress
            .resources
            .iter()
            .find(|resource| resource.kind() == ResourceKind::Agent)
            .unwrap()
            .clone(),
        binding_digest: scope.agent_service_binding_digest("worker").unwrap().into(),
        ingress_effect: ingress,
        effects: scope
            .agent_service_direct_effects("worker")
            .unwrap()
            .to_vec(),
    }
}

fn source_execution(
    destination: &ExecutionAuthorityConfig,
    destination_origin: &str,
) -> ExecutionAuthorityConfig {
    let destination_scope = &destination.scopes[0];
    let destination_federation = destination_scope.federation.as_ref().unwrap();
    let grant = destination_federation.trusts[0].approved[0].clone();
    let limits = json!({
        "max_units": 5,
        "max_concurrent": 2,
        "deadline_ms": 4_102_444_800_000_i64
    });
    let source = json!({"id": "agent/source", "kind": "agent"});
    let controller = json!({"id": "agent/controller", "kind": "agent"});
    let route = json!({"provider": "incident", "action_type": "execute"});
    let mut card = AgentCard::new("source", NAMESPACE, TENANT, "Source agent", "1");
    card.skills.push(Skill::new("originate"));
    card.interfaces.push(AgentCardInterface {
        kind: "rest".into(),
        url: "https://agents.example/source".into(),
    });
    let mut execution: ExecutionAuthorityConfig = serde_json::from_value(json!({
        "agent_driver": {"enabled": false, "poll_interval_ms": 100, "max_parallel": 1, "scan_batch_size": 4},
        "scopes": [{
            "namespace": NAMESPACE, "tenant": TENANT, "bootstrap": true,
            "publisher": controller, "subjects": [controller, source], "routes": [route],
            "valid_from_ms": 0, "credential_limits": limits,
            "root_max_units": 5, "root_max_concurrent": 1, "root_lifetime_ms": 60_000,
            "agent_services": [{
                "card": card, "principal": source, "skill": "originate",
                "endpoint": "https://agents.example/source", "endpoint_id": "source-api",
                "route": route, "recipient_key_env": "ACTEON_SOURCE_RECIPIENT",
                "recipient_permits": [{"id": "source-worker", "accepted_revision": 1}],
                "grants": [{"id": "controller-source", "revision": 1, "source": controller,
                    "source_permits": [{"id": "controller-service", "accepted_revision": 1}],
                    "valid_from_ms": 0, "limits": limits, "max_depth": 4}]
            }],
            "permits": [
                {"id": "controller-service", "revision": 1, "subject": controller, "routes": [],
                    "agents": ["source"], "valid_from_ms": 0, "limits": limits},
                {"id": "source-worker", "revision": 1, "subject": source, "routes": [route],
                    "federation_exports": ["remote-worker"], "valid_from_ms": 0, "limits": limits}
            ]
        }]
    }))
    .unwrap();
    execution.scopes[0].federation = Some(ExecutionFederationConfig {
        local_audience: SOURCE_DOMAIN.into(),
        capability_publisher: None,
        trusts: vec![],
        peers: vec![],
        exports: vec![FederationExportConfig {
            id: "remote-worker".into(),
            source_agent_id: "source".into(),
            remote_origin: destination_origin.into(),
            remote_audience: DESTINATION_DOMAIN.into(),
            active_epoch: 1,
            epochs: vec![FederationExportEpochConfig {
                trust_id: "source-prod".into(),
                trust_revision: 1,
                key_id: "source-envelope-v1".into(),
                verifying_key: FederationEnvelopeSigner::from_secret([41; 32]).verifying_key_hex(),
                signer_key_env: "ACTEON_SOURCE_FEDERATION_SIGNER".into(),
                issuer_epoch: 1,
                credential_env: "ACTEON_SOURCE_PEER_CREDENTIAL".into(),
                credential_broker: None,
                peer_capability: FederationPeerCapabilityConfig {
                    key_id: "destination-capability-v1".into(),
                    verifying_key: FederationCapabilitySigner::from_secret([42; 32])
                        .verifying_key_hex(),
                    minimum_issuer_epoch: 1,
                    max_clock_skew_ms: 30_000,
                    protocol_revision: "acteon-federation-v1".into(),
                    authentication_scheme: "bearer".into(),
                    require_peer_finality: false,
                },
                grant,
            }],
            max_lifetime_ms: 60_000,
            timeout_ms: 5_000,
            internal_hosts: vec!["127.0.0.1".into()],
        }],
    });
    execution
}

fn server_configuration(
    port: u16,
    redis_url: &str,
    prefix: &str,
    webhook: &str,
    execution: &ExecutionAuthorityConfig,
) -> Value {
    json!({
        "server": {"host": "127.0.0.1", "port": port},
        "state": {"backend": "redis", "url": redis_url, "prefix": prefix},
        "ui": {"enabled": false},
        "auth": {"enabled": true, "config_path": "auth.toml", "watch": false,
            "authority": {"namespace": "auth-control", "tenant": "deployment",
                "source_id": format!("federation-{prefix}"), "bootstrap": true}},
        "providers": [{"name": "incident", "type": "webhook", "url": webhook,
            "internal_hosts": ["127.0.0.1"]}],
        "execution_authority": execution
    })
}

fn destination_authentication() -> String {
    format!(
        r#"
authority_revision = 1
[settings]
jwt_secret = "test-jwt-secret-at-least-32-bytes"
[[api_keys]]
name = "operator"
authority_id = "credential/operator"
principal = {{id="operator",kind="human"}}
key_hash = {:?}
role = "operator"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["audit"]
actions = ["read"]
[[api_keys]]
name = "source-peer"
authority_id = "credential/source-peer"
principal = {{id="agent/source",kind="agent"}}
key_hash = {:?}
role = "operator"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["incident"]
actions = ["execute"]
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["agent.worker"]
actions = ["invoke"]
[[api_keys]]
name = "worker"
authority_id = "credential/worker"
principal = {{id="agent/worker",kind="agent"}}
key_hash = {:?}
role = "executor"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["incident"]
actions = ["execute"]
"#,
        acteon_server::auth::api_key::hash_api_key("operator-secret"),
        acteon_server::auth::api_key::hash_api_key("source-peer-secret"),
        acteon_server::auth::api_key::hash_api_key("worker-secret")
    )
}

#[test]
fn source_and_destination_declarations_prepare_as_one_exact_contract() {
    let registry = provider_registry("https://webhook.example/incident");
    let (destination, trust) = destination_execution(&registry);
    let source = source_execution(&destination, "https://destination.example");
    let prepared_source = registry
        .prepare(&source, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let export = &source.scopes[0].federation.as_ref().unwrap().exports[0];
    let epoch = &export.epochs[0];

    assert_eq!(
        export.remote_audience,
        destination.scopes[0]
            .federation
            .as_ref()
            .unwrap()
            .local_audience
    );
    assert_eq!(epoch.trust_id, trust.id);
    assert_eq!(epoch.verifying_key, trust.verifying_key);
    assert_eq!(epoch.grant, trust.approved[0]);
    assert!(
        prepared_source[0]
            .agent_service_direct_effects("source")
            .unwrap()
            .iter()
            .any(|effect| effect.operation == "federation.export")
    );
}

fn source_authentication() -> String {
    format!(
        r#"
authority_revision = 1
[settings]
jwt_secret = "test-jwt-secret-at-least-32-bytes"
[[api_keys]]
name = "controller"
authority_id = "credential/controller"
principal = {{id="agent/controller",kind="agent"}}
key_hash = {:?}
role = "executor"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["agent.source"]
actions = ["invoke"]
[[api_keys]]
name = "source"
authority_id = "credential/source"
principal = {{id="agent/source",kind="agent"}}
key_hash = {:?}
role = "executor"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["federation.remote-worker"]
actions = ["export"]
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["incident"]
actions = ["execute"]
[[api_keys]]
name = "official-a2a-client"
authority_id = "credential/official-a2a-client"
principal = {{id="agent/controller",kind="agent"}}
key_hash = {:?}
role = "operator"
[[api_keys.grants]]
namespaces = ["prod"]
tenants = ["acme"]
providers = ["a2a"]
actions = ["rpc"]
"#,
        acteon_server::auth::api_key::hash_api_key("controller-secret"),
        acteon_server::auth::api_key::hash_api_key("source-secret"),
        acteon_server::auth::api_key::hash_api_key("official-a2a-secret")
    )
}

async fn publish_official_v1_card(redis_url: &str, prefix: &str, server_url: &str) {
    let config = acteon_state_redis::RedisConfig {
        url: redis_url.to_string(),
        prefix: prefix.to_string(),
        ..Default::default()
    };
    let store = acteon_state_redis::RedisStateStore::new(&config).unwrap();
    let mut card = AgentCard::new(
        "official-worker",
        NAMESPACE,
        TENANT,
        "Governed city worker",
        "1.0.0",
    );
    card.description = Some("A governed A2A task endpoint for conformance".into());
    card.capabilities.streaming = true;
    card.capabilities.push_notifications = true;
    card.skills.push(Skill::new("inspect-city"));
    card.interfaces.push(AgentCardInterface {
        kind: "json-rpc".into(),
        url: format!("{server_url}/a2a/{NAMESPACE}/{TENANT}"),
    });
    let key = StateKey::new(NAMESPACE, TENANT, KeyKind::BusAgentCard, "official-worker");
    store
        .set(&key, &serde_json::to_string(&card).unwrap(), None)
        .await
        .unwrap();
}

fn run_official_a2a_client(server_url: &str) {
    let python = std::env::var("ACTEON_A2A_PYTHON").unwrap_or_else(|_| "python3".into());
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/a2a_official_client_conformance.py");
    let output = Command::new(python)
        .arg(script)
        .arg("--base-url")
        .arg(format!("{server_url}/a2a/{NAMESPACE}/{TENANT}"))
        .arg("--token")
        .arg("official-a2a-secret")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "official a2a-sdk failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

async fn wait_for_calls(calls: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while calls.load(Ordering::SeqCst) != expected {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires ACTEON_GOVERNANCE_REDIS_URL and socket-enabled process execution"]
async fn two_acteon_servers_negotiate_export_restart_and_deliver_one_effect() {
    let redis_url = std::env::var("ACTEON_GOVERNANCE_REDIS_URL").unwrap();
    let (webhook, provider_calls, webhook_task) = counted_webhook().await;
    let registry = provider_registry(&webhook);
    let (destination_execution, trust) = destination_execution(&registry);
    let destination_prefix = format!("federation-destination-{}", uuid::Uuid::new_v4());
    let destination_config = server_configuration(
        free_port(),
        &redis_url,
        &destination_prefix,
        &webhook,
        &destination_execution,
    );
    let mut destination = BinaryServer::start(
        &destination_config,
        &destination_authentication(),
        vec![
            (
                "ACTEON_DESTINATION_RECIPIENT".into(),
                "worker-secret".into(),
            ),
            (
                "ACTEON_DESTINATION_CAPABILITY_SIGNER".into(),
                "2a".repeat(32),
            ),
        ],
    );
    let client = reqwest::Client::new();
    destination.ready(&client).await;

    let publication = json!({
        "namespace": NAMESPACE,
        "tenant": TENANT,
        "change_id": "publish-source-prod-v1",
        "expected_revision": 0,
        "trust": trust,
        "reason": "two-server conformance"
    });
    let response = client
        .post(format!(
            "{}/v1/governance/federation/trusts",
            destination.url
        ))
        .bearer_auth("operator-secret")
        .json(&publication)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());

    // Commit the first invocation at the destination, then discard its response. This
    // gives the source a real ambiguous delivery to recover across both restarts.
    let (proxy_url, proxy_state, proxy_task) = response_loss_proxy(destination.url.clone()).await;
    let source_execution = source_execution(&destination_execution, &proxy_url);
    registry
        .prepare(&source_execution, ("auth-control", "deployment"), &[8; 32])
        .unwrap();
    let source_prefix = format!("federation-source-{}", uuid::Uuid::new_v4());
    let source_config = server_configuration(
        free_port(),
        &redis_url,
        &source_prefix,
        &webhook,
        &source_execution,
    );
    let mut source = BinaryServer::start(
        &source_config,
        &source_authentication(),
        vec![
            ("ACTEON_SOURCE_RECIPIENT".into(), "source-secret".into()),
            ("ACTEON_SOURCE_FEDERATION_SIGNER".into(), "29".repeat(32)),
            (
                "ACTEON_SOURCE_PEER_CREDENTIAL".into(),
                "source-peer-secret".into(),
            ),
        ],
    );
    source.ready(&client).await;
    publish_official_v1_card(&redis_url, &source_prefix, &source.url).await;
    run_official_a2a_client(&source.url);

    let accepted = client
        .post(format!(
            "{}/a2a/{NAMESPACE}/{TENANT}/agents/source/v1/message:send",
            source.url
        ))
        .bearer_auth("controller-secret")
        .json(
            &json!({"message": {"role": "user", "messageId": "source-task-message",
            "parts": [{"kind": "text", "text": "delegate incident triage"}]}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 200, "{}", accepted.text().await.unwrap());
    let source_task: Value = accepted.json().await.unwrap();
    let source_task_id = source_task["id"].as_str().unwrap();
    uuid::Uuid::parse_str(source_task_id).unwrap();
    let outbound = TaskMessage::text(
        "federation-message-1",
        TaskRole::User,
        "triage the shared incident",
    );
    let export_url = format!(
        "{}/a2a/{NAMESPACE}/{TENANT}/agents/source/v1/tasks/{source_task_id}/federation/exports/remote-worker/message:send",
        source.url
    );
    let first = client
        .post(&export_url)
        .bearer_auth("source-secret")
        .header("a2a-version", "1.0")
        .json(&json!({"message": outbound}))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200, "{}", first.text().await.unwrap());
    let first: Value = first.json().await.unwrap();
    assert_eq!(first["attempt_status"], "uncertain");
    assert_eq!(first["delivery"]["status"], "invoke_uncertain");
    let delivery_id = first["delivery_id"].as_str().unwrap().to_owned();
    wait_for_calls(&provider_calls, 1).await;
    assert_eq!(proxy_state.invocations.load(Ordering::SeqCst), 1);

    let repeated = client
        .post(&export_url)
        .bearer_auth("source-secret")
        .header("a2a-version", "1.0")
        .json(&json!({"message": outbound}))
        .send()
        .await
        .unwrap();
    assert_eq!(repeated.status(), 200);
    let repeated: Value = repeated.json().await.unwrap();
    assert_eq!(repeated["delivery_id"], delivery_id);
    assert_eq!(repeated["attempt_status"], "uncertain");
    assert_eq!(repeated["delivery"]["status"], "invoke_uncertain");
    assert_eq!(proxy_state.invocations.load(Ordering::SeqCst), 1);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);

    source.restart();
    destination.restart();
    source.ready(&client).await;
    destination.ready(&client).await;
    let replayed = client
        .post(export_url.replace("message:send", "message:replay"))
        .bearer_auth("source-secret")
        .header("a2a-version", "1.0")
        .json(&json!({"message": outbound}))
        .send()
        .await
        .unwrap();
    assert_eq!(replayed.status(), 200, "{}", replayed.text().await.unwrap());
    let replayed: Value = replayed.json().await.unwrap();
    assert_eq!(replayed["delivery_id"], delivery_id);
    assert_eq!(replayed["attempt_status"], "settled");
    assert_eq!(replayed["delivery"]["status"], "accepted");
    let remote_task_id = replayed["delivery"]["task"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(proxy_state.invocations.load(Ordering::SeqCst), 2);
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);

    let observed = client
        .post(export_url.replace("message:send", "message:observe"))
        .bearer_auth("source-secret")
        .header("a2a-version", "1.0")
        .json(&json!({"message": outbound}))
        .send()
        .await
        .unwrap();
    assert_eq!(observed.status(), 200, "{}", observed.text().await.unwrap());
    let observed: Value = observed.json().await.unwrap();
    assert_eq!(observed["delivery_id"], delivery_id);
    assert_eq!(observed["delivery"]["task"]["id"], remote_task_id);
    assert_eq!(proxy_state.invocations.load(Ordering::SeqCst), 2);

    let remote = client
        .get(format!(
            "{}/a2a/{NAMESPACE}/{TENANT}/agents/worker/v1/tasks/{remote_task_id}",
            destination.url
        ))
        .bearer_auth("source-peer-secret")
        .send()
        .await
        .unwrap();
    assert_eq!(remote.status(), 200, "{}", remote.text().await.unwrap());
    assert_eq!(provider_calls.load(Ordering::SeqCst), 1);
    proxy_task.abort();
    webhook_task.abort();
}
