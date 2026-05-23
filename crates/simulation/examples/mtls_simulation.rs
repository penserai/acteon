//! Mutual TLS (mTLS) end-to-end simulation.
//!
//! This self-contained example demonstrates full mTLS between an Acteon client
//! and server. It generates a CA, server certificate, and client certificate at
//! runtime using `rcgen`, starts an HTTPS server with client certificate
//! verification, and dispatches actions over a mutually authenticated channel.
//!
//! No external processes or pre-existing certificate files are needed.
//!
//! Run with:
//!   cargo run -p acteon-simulation --example mtls_simulation

use std::net::SocketAddr;
use std::sync::Arc;

use acteon_client::ActeonClientBuilder;
use acteon_core::{Action, ActionOutcome};
use acteon_gateway::GatewayBuilder;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use rcgen::{CertificateParams, KeyPair};
use tokio::sync::RwLock;
use tracing::info;

// ---------------------------------------------------------------------------
// Certificate generation helpers
// ---------------------------------------------------------------------------

struct CertBundle {
    ca_cert_pem: String,
    server_cert_pem: String,
    server_key_pem: String,
    client_cert_pem: String,
    client_key_pem: String,
}

fn generate_certs() -> CertBundle {
    // --- CA ---
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Acteon Simulation CA");
    let ca_key = KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    // --- Server cert (signed by CA) ---
    let mut server_params =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    server_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "localhost");
    server_params
        .subject_alt_names
        .push(rcgen::SanType::IpAddress(
            "127.0.0.1".parse::<std::net::IpAddr>().unwrap(),
        ));
    let server_key = KeyPair::generate().unwrap();
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .unwrap();

    // --- Client cert (signed by CA) ---
    let mut client_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    client_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "acteon-client");
    let client_key = KeyPair::generate().unwrap();
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .unwrap();

    CertBundle {
        ca_cert_pem: ca_cert.pem(),
        server_cert_pem: server_cert.pem(),
        server_key_pem: server_key.serialize_pem(),
        client_cert_pem: client_cert.pem(),
        client_key_pem: client_key.serialize_pem(),
    }
}

/// Write PEM strings to temp files and return their paths.
fn write_certs_to_temp(bundle: &CertBundle) -> (String, String, String, String, String) {
    let dir = std::env::temp_dir().join(format!("acteon-mtls-sim-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let ca = dir.join("ca.pem");
    let server_cert = dir.join("server.crt");
    let server_key = dir.join("server.key");
    let client_cert = dir.join("client.crt");
    let client_key = dir.join("client.key");

    std::fs::write(&ca, &bundle.ca_cert_pem).unwrap();
    std::fs::write(&server_cert, &bundle.server_cert_pem).unwrap();
    std::fs::write(&server_key, &bundle.server_key_pem).unwrap();
    std::fs::write(&client_cert, &bundle.client_cert_pem).unwrap();
    std::fs::write(&client_key, &bundle.client_key_pem).unwrap();

    (
        ca.display().to_string(),
        server_cert.display().to_string(),
        server_key.display().to_string(),
        client_cert.display().to_string(),
        client_key.display().to_string(),
    )
}

// ---------------------------------------------------------------------------
// Minimal HTTPS server backed by a Gateway
// ---------------------------------------------------------------------------

async fn run_tls_server(
    addr: SocketAddr,
    gateway: Arc<RwLock<acteon_gateway::Gateway>>,
    server_cert_path: &str,
    server_key_path: &str,
    client_ca_path: &str,
    ready_tx: tokio::sync::oneshot::Sender<()>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    use tower::ServiceExt;

    // Health handler
    async fn health() -> axum::http::StatusCode {
        axum::http::StatusCode::OK
    }

    // Dispatch handler
    async fn dispatch(
        axum::extract::State(gw): axum::extract::State<Arc<RwLock<acteon_gateway::Gateway>>>,
        axum::Json(action): axum::Json<Action>,
    ) -> axum::response::Result<axum::Json<ActionOutcome>, axum::http::StatusCode> {
        let gateway = gw.read().await;
        match gateway.dispatch(action, None).await {
            Ok(outcome) => Ok(axum::Json(outcome)),
            Err(_) => Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    let app = axum::Router::new()
        .route("/health", axum::routing::get(health))
        .route("/v1/dispatch", axum::routing::post(dispatch))
        .with_state(gateway);

    // Build TLS config with mTLS (client cert required)
    let tls_config = acteon_crypto::tls::build_server_config(
        server_cert_path,
        server_key_path,
        Some(client_ca_path),
        acteon_crypto::tls::MinTlsVersion::Tls12,
    )
    .expect("failed to build TLS server config");

    let tls_acceptor = tokio_rustls::TlsAcceptor::from(tls_config);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind TLS listener");

    // Signal that the server is ready
    let _ = ready_tx.send(());

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (tcp_stream, remote_addr) = match result {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let acceptor = tls_acceptor.clone();
                let app = app.clone();

                tokio::spawn(async move {
                    let tls_stream = match acceptor.accept(tcp_stream).await {
                        Ok(s) => s,
                        Err(_) => return,
                    };

                    let io = hyper_util::rt::TokioIo::new(tls_stream);
                    let tower_service = app;
                    let hyper_service = hyper::service::service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            tower_service.clone().oneshot(request)
                        },
                    );

                    let _ = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .serve_connection(io, hyper_service)
                    .await;

                    let _ = remote_addr;
                });
            }
            _ = &mut shutdown_rx => {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Main simulation
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Install the ring crypto provider globally — needed because both ring and
    // aws-lc-rs are in the dependency tree (from AWS SDK).
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install ring CryptoProvider");

    info!(
        "{}",
        "╔══════════════════════════════════════════════════════════════╗"
    );
    info!(
        "{}",
        "║          mTLS END-TO-END SIMULATION                         ║"
    );
    info!(
        "{}",
        "╚══════════════════════════════════════════════════════════════╝\n"
    );

    // =========================================================================
    // Step 1: Generate certificates
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  STEP 1: CERTIFICATE GENERATION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let bundle = generate_certs();
    let (ca_path, server_cert_path, server_key_path, client_cert_path, client_key_path) =
        write_certs_to_temp(&bundle);

    info!("  Generated at runtime (no pre-existing files needed):");
    info!("    CA certificate:     {ca_path}");
    info!("    Server certificate: {server_cert_path}");
    info!("    Server private key: {server_key_path}");
    info!("    Client certificate: {client_cert_path}");
    info!("    Client private key: {client_key_path}");
    info!("");

    // =========================================================================
    // Step 2: Build in-process Gateway (memory backend)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  STEP 2: GATEWAY SETUP (memory backend)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
    let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
    let audit: Arc<dyn acteon_audit::AuditStore> =
        Arc::new(acteon_audit_memory::MemoryAuditStore::new());

    let recording_provider = Arc::new(acteon_simulation::RecordingProvider::new("webhook"));

    // Parse a simple rule
    let rules_yaml = r#"
rules:
  - name: block-spam
    priority: 1
    condition:
      field: action.action_type
      eq: "spam"
    action:
      type: suppress
"#;
    use acteon_rules::RuleFrontend;
    let rules = acteon_rules_yaml::YamlFrontend
        .parse(rules_yaml)
        .expect("failed to parse rules");

    let gateway = GatewayBuilder::new()
        .state(state)
        .lock(lock)
        .audit(audit)
        .rules(rules)
        .provider(recording_provider.clone() as Arc<dyn acteon_provider::DynProvider>)
        .build()
        .expect("failed to build gateway");

    let gateway = Arc::new(RwLock::new(gateway));

    info!("  State backend:  memory");
    info!("  Audit backend:  memory");
    info!("  Provider:       webhook (recording)");
    info!("  Rules:          block-spam (suppress action_type='spam')");
    info!("");

    // =========================================================================
    // Step 3: Start HTTPS server with mTLS
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  STEP 3: START HTTPS SERVER (mTLS enabled)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    // Bind first to get the actual port
    let temp_listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_addr = temp_listener.local_addr()?;
    drop(temp_listener);

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    let server_cert = server_cert_path.clone();
    let server_key = server_key_path.clone();
    let ca = ca_path.clone();
    let gw = Arc::clone(&gateway);

    tokio::spawn(async move {
        run_tls_server(
            actual_addr,
            gw,
            &server_cert,
            &server_key,
            &ca,
            ready_tx,
            shutdown_rx,
        )
        .await;
    });

    // Wait for server to be ready
    ready_rx.await?;

    let base_url = format!("https://127.0.0.1:{}", actual_addr.port());
    info!("  Server listening at: {base_url}");
    info!("  TLS version:        1.2+");
    info!("  Client cert verify: REQUIRED (mTLS)");
    info!("");

    // =========================================================================
    // Demo 1: Successful mTLS connection
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 1: SUCCESSFUL mTLS CONNECTION");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let client = ActeonClientBuilder::new(&base_url)
        .ca_cert_path(&ca_path)
        .client_cert(&client_cert_path, &client_key_path)
        .build()?;

    info!("  Client configured with:");
    info!("    CA cert:     {ca_path}");
    info!("    Client cert: {client_cert_path}");
    info!("    Client key:  {client_key_path}\n");

    // Health check over mTLS
    match client.health().await {
        Ok(true) => info!("  Health check: PASSED (mTLS handshake successful)"),
        Ok(false) => info!("  Health check: server unhealthy"),
        Err(e) => info!("  Health check: FAILED - {e}"),
    }
    info!("");

    // Dispatch an action over mTLS
    let action = Action::new(
        "notifications",
        "tenant-mtls",
        "webhook",
        "send_alert",
        serde_json::json!({
            "to": "ops-team@example.com",
            "message": "Dispatched over mTLS!"
        }),
    );

    info!("  Dispatching action over mTLS...");
    info!("    Provider:    webhook");
    info!("    Action type: send_alert");

    match client.dispatch(&action).await {
        Ok(outcome) => {
            info!("    Outcome:     {outcome:?}");
            info!(
                "    Provider called: {} time(s)",
                recording_provider.call_count()
            );
        }
        Err(e) => info!("    Error: {e}"),
    }
    info!("");

    // =========================================================================
    // Demo 2: Rule enforcement over mTLS
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 2: RULE ENFORCEMENT OVER mTLS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let spam_action = Action::new(
        "notifications",
        "tenant-mtls",
        "webhook",
        "spam",
        serde_json::json!({"subject": "Buy now!!!"}),
    );

    info!("  Dispatching SPAM action (should be suppressed by rule)...");
    match client.dispatch(&spam_action).await {
        Ok(outcome) => {
            let suppressed = matches!(outcome, ActionOutcome::Suppressed { .. });
            info!("    Outcome:     {outcome:?}");
            info!(
                "    Suppressed:  {} (block-spam rule active)",
                if suppressed { "YES" } else { "NO" }
            );
        }
        Err(e) => info!("    Error: {e}"),
    }
    info!("");

    // =========================================================================
    // Demo 3: Batch dispatch over mTLS
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 3: BATCH DISPATCH OVER mTLS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let start = std::time::Instant::now();
    let mut success = 0;
    let mut errors = 0;
    let count = 50;

    for i in 0..count {
        let action = Action::new(
            "benchmark",
            "tenant-mtls",
            "webhook",
            "throughput_test",
            serde_json::json!({"seq": i}),
        );
        match client.dispatch(&action).await {
            Ok(_) => success += 1,
            Err(_) => errors += 1,
        }
    }

    let elapsed = start.elapsed();
    let throughput = count as f64 / elapsed.as_secs_f64();

    info!("  {count} sequential HTTPS+mTLS requests:");
    info!("    Success:    {success}");
    info!("    Errors:     {errors}");
    info!("    Duration:   {elapsed:?}");
    info!("    Throughput: {throughput:.0} req/sec");
    info!("");

    // =========================================================================
    // Demo 4: Connection WITHOUT client cert (should be rejected)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 4: REJECTED CONNECTION (no client certificate)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let no_cert_client = ActeonClientBuilder::new(&base_url)
        .ca_cert_path(&ca_path)
        // No client_cert() — server should reject
        .build()?;

    info!("  Client configured WITHOUT client certificate");
    info!("  Attempting health check...");

    match no_cert_client.health().await {
        Ok(true) => info!("    UNEXPECTED: connection accepted without client cert!"),
        Ok(false) => info!("    Server returned unhealthy (connection may have succeeded)"),
        Err(e) => {
            info!("    REJECTED: {e}");
            info!("    (Server correctly refused the TLS handshake)");
        }
    }
    info!("");

    // =========================================================================
    // Demo 5: Connection with invalid CA (should be rejected by client)
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO 5: REJECTED CONNECTION (wrong CA - untrusted server)");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Generate a different CA that didn't sign the server cert
    let mut rogue_ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    rogue_ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    rogue_ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Rogue CA");
    let rogue_key = KeyPair::generate().unwrap();
    let rogue_ca = rogue_ca_params.self_signed(&rogue_key).unwrap();

    let rogue_ca_path = std::env::temp_dir()
        .join(format!("acteon-mtls-sim-{}", std::process::id()))
        .join("rogue-ca.pem");
    std::fs::write(&rogue_ca_path, rogue_ca.pem()).unwrap();

    let wrong_ca_client = ActeonClientBuilder::new(&base_url)
        .ca_cert_path(rogue_ca_path.display().to_string())
        .client_cert(&client_cert_path, &client_key_path)
        .build()?;

    info!("  Client configured with WRONG CA (server cert not trusted)");
    info!("  Attempting health check...");

    match wrong_ca_client.health().await {
        Ok(true) => info!("    UNEXPECTED: connection accepted with wrong CA!"),
        Ok(false) => info!("    Server returned unhealthy"),
        Err(e) => {
            info!("    REJECTED: {e}");
            info!("    (Client correctly refused the untrusted server certificate)");
        }
    }
    info!("");

    // =========================================================================
    // Architecture recap
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  ARCHITECTURE");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("    ┌─────────────────┐  TLS 1.2+ (mTLS)  ┌──────────────────┐");
    info!("    │  ActeonClient   │ ═══════════════════│  HTTPS Server    │");
    info!("    │  (rustls)       │  client cert +     │  (tokio-rustls)  │");
    info!("    │                 │  server cert       │                  │");
    info!("    └─────────────────┘  verified by CA    └────────┬─────────┘");
    info!("                                                    │");
    info!("                                           ┌────────▼─────────┐");
    info!("                                           │    Gateway       │");
    info!("                                           │  ┌────────────┐  │");
    info!("                                           │  │   Rules    │  │");
    info!("                                           │  │ (in-mem)   │  │");
    info!("                                           │  └────────────┘  │");
    info!("                                           │  ┌────────────┐  │");
    info!("                                           │  │   State    │  │");
    info!("                                           │  │ (memory)   │  │");
    info!("                                           │  └────────────┘  │");
    info!("                                           └────────┬─────────┘");
    info!("                                                    │");
    info!("                                           ┌────────▼─────────┐");
    info!("                                           │    Provider      │");
    info!("                                           │  (recording)     │");
    info!("                                           └──────────────────┘");
    info!("");
    info!("  All certificates generated at runtime by rcgen (no openssl needed).");
    info!("  Server requires client certificates (mTLS enforced).");
    info!("  Memory backends used for state, audit, and locking.");
    info!("");

    // Shutdown the server
    let _ = shutdown_tx.send(());

    // Cleanup temp files
    let temp_dir = std::env::temp_dir().join(format!("acteon-mtls-sim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(temp_dir);

    info!(
        "{}",
        "╔══════════════════════════════════════════════════════════════╗"
    );
    info!(
        "{}",
        "║              mTLS SIMULATION COMPLETE                        ║"
    );
    info!(
        "{}",
        "╚══════════════════════════════════════════════════════════════╝"
    );

    Ok(())
}
