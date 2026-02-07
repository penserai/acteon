//! Polyglot client simulation - tests all language clients against a running server.
//!
//! This simulation:
//! 1. Starts an Acteon HTTP server with recording providers
//! 2. Runs client tests for Python, Node.js, Go, and Java
//! 3. Verifies each client can correctly interact with the API
//!
//! Prerequisites:
//!   - Python 3.10+ with httpx installed
//!   - Node.js 18+ with npm
//!   - Go 1.22+
//!   - Java 21+ (optional, uses jbang if available)
//!
//! Run with:
//!   cargo run -p acteon-simulation --example polyglot_client_simulation

use std::net::SocketAddr;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use acteon_audit_memory::MemoryAuditStore;
use acteon_core::{Action, ActionOutcome};
use acteon_executor::ExecutorConfig;
use acteon_gateway::GatewayBuilder;
use acteon_provider::DynProvider;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use tokio::net::TcpListener;
use tokio::sync::RwLock;

/// Server handle that can be stopped
struct TestServer {
    addr: SocketAddr,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    /// Start a test server with the given recording provider
    async fn start(provider: Arc<RecordingProvider>) -> Result<Self, Box<dyn std::error::Error>> {
        // Create in-memory backends
        let state: Arc<dyn acteon_state::StateStore> = Arc::new(MemoryStateStore::new());
        let lock: Arc<dyn acteon_state::DistributedLock> = Arc::new(MemoryDistributedLock::new());
        let audit: Arc<dyn acteon_audit::AuditStore> = Arc::new(MemoryAuditStore::new());

        // Build gateway with recording provider
        let gateway = GatewayBuilder::new()
            .state(Arc::clone(&state))
            .lock(lock)
            .audit(Arc::clone(&audit))
            .provider(provider as Arc<dyn DynProvider>)
            .executor_config(ExecutorConfig::default())
            .build()?;

        let gateway = Arc::new(RwLock::new(gateway));

        // Create app state
        let app_state = acteon_server::api::AppState {
            gateway,
            audit: Some(audit),
            auth: None,
            rate_limiter: None,
            embedding: None,
            embedding_metrics: None,
        };

        // Build router
        let app = acteon_server::api::router(app_state);

        // Bind to random available port
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        // Spawn server
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        // Wait for server to be ready by polling /health
        let health_url = format!("http://{}/health", addr);
        let client = reqwest::Client::new();
        let max_attempts = 50;
        for attempt in 1..=max_attempts {
            match client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ if attempt == max_attempts => {
                    return Err("Server failed to start: health check timeout".into());
                }
                _ => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }

        Ok(Self {
            addr,
            shutdown_tx,
            handle,
        })
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn stop(self) {
        let _ = self.shutdown_tx.send(());
        let _ = self.handle.await;
    }
}

/// Result of running a client test
#[derive(Debug)]
struct ClientTestResult {
    language: String,
    success: bool,
    output: String,
}

/// Run the Python client test
fn run_python_client(base_url: &str, project_root: &str) -> ClientTestResult {
    let script = format!(
        "{}/acteon-simulation/scripts/test_python_client.py",
        project_root
    );

    let output = Command::new("python3")
        .arg(&script)
        .env("ACTEON_URL", base_url)
        .env("PYTHONPATH", format!("{}/clients/python", project_root))
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            ClientTestResult {
                language: "Python".to_string(),
                success: out.status.success(),
                output: format!("{}\n{}", stdout, stderr),
            }
        }
        Err(e) => ClientTestResult {
            language: "Python".to_string(),
            success: false,
            output: format!("Failed to run: {}", e),
        },
    }
}

/// Run the Node.js client test
fn run_nodejs_client(base_url: &str, project_root: &str) -> ClientTestResult {
    let client_dir = format!("{}/clients/nodejs", project_root);

    // Install dependencies if needed
    let install_output = Command::new("npm")
        .arg("install")
        .arg("--legacy-peer-deps")
        .current_dir(&client_dir)
        .output();

    if let Err(e) = install_output {
        return ClientTestResult {
            language: "Node.js".to_string(),
            success: false,
            output: format!("Failed to install dependencies: {}", e),
        };
    }

    // Build the TypeScript client
    let build_output = Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir(&client_dir)
        .output();

    match build_output {
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            return ClientTestResult {
                language: "Node.js".to_string(),
                success: false,
                output: format!("Build failed:\n{}\n{}", stdout, stderr),
            };
        }
        Err(e) => {
            return ClientTestResult {
                language: "Node.js".to_string(),
                success: false,
                output: format!("Failed to build: {}", e),
            };
        }
        _ => {}
    }

    let script = format!(
        "{}/acteon-simulation/scripts/test_nodejs_client.mjs",
        project_root
    );

    let output = Command::new("node")
        .arg(&script)
        .env("ACTEON_URL", base_url)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            ClientTestResult {
                language: "Node.js".to_string(),
                success: out.status.success(),
                output: format!("{}\n{}", stdout, stderr),
            }
        }
        Err(e) => ClientTestResult {
            language: "Node.js".to_string(),
            success: false,
            output: format!("Failed to run: {}", e),
        },
    }
}

/// Run the Go client test
fn run_go_client(base_url: &str, project_root: &str) -> ClientTestResult {
    let script = format!(
        "{}/acteon-simulation/scripts/test_go_client.go",
        project_root
    );
    let go_client_dir = format!("{}/clients/go", project_root);

    let output = Command::new("go")
        .arg("run")
        .arg(&script)
        .env("ACTEON_URL", base_url)
        .current_dir(&go_client_dir)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            ClientTestResult {
                language: "Go".to_string(),
                success: out.status.success(),
                output: format!("{}\n{}", stdout, stderr),
            }
        }
        Err(e) => ClientTestResult {
            language: "Go".to_string(),
            success: false,
            output: format!("Failed to run: {}", e),
        },
    }
}

/// Run the Java client test (builds JAR with Gradle and runs with java)
fn run_java_client(base_url: &str, project_root: &str) -> ClientTestResult {
    let java_client_dir = format!("{}/clients/java", project_root);

    // Check if java is available
    let java_check = Command::new("java").arg("-version").output();
    if java_check.is_err() || !java_check.unwrap().status.success() {
        return ClientTestResult {
            language: "Java".to_string(),
            success: true,
            output: "Skipped (java not available)".to_string(),
        };
    }

    // Check if gradle is available
    let gradle_check = Command::new("gradle").arg("--version").output();
    if gradle_check.is_err() {
        return ClientTestResult {
            language: "Java".to_string(),
            success: true,
            output: "Skipped (gradle not available)".to_string(),
        };
    }

    // Build the JAR using the build script
    println!("  Building Java client JAR...");
    let build_output = Command::new("bash")
        .arg("build.sh")
        .current_dir(&java_client_dir)
        .output();

    match build_output {
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            return ClientTestResult {
                language: "Java".to_string(),
                success: false,
                output: format!("Build failed:\n{}\n{}", stdout, stderr),
            };
        }
        Err(e) => {
            return ClientTestResult {
                language: "Java".to_string(),
                success: false,
                output: format!("Failed to run build script: {}", e),
            };
        }
        _ => {}
    }

    // Run the test script using the JAR
    let script = format!(
        "{}/acteon-simulation/scripts/TestJavaClient.java",
        project_root
    );
    let jar_path = format!("{}/build/libs/acteon-client-0.1.0.jar", java_client_dir);

    // First try jbang (fastest), then fall back to java -cp
    let output = Command::new("jbang")
        .arg(&script)
        .env("ACTEON_URL", base_url)
        .output();

    let result = match output {
        Ok(out)
            if out.status.success()
                || !String::from_utf8_lossy(&out.stderr).contains("not found") =>
        {
            Some((
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
            ))
        }
        _ => None,
    };

    if let Some((success, stdout, stderr)) = result {
        return ClientTestResult {
            language: "Java".to_string(),
            success,
            output: format!("{}\n{}", stdout, stderr),
        };
    }

    // Fall back to running compiled test with java
    // Compile and run the test class using the JAR
    let compile_dir = format!("{}/acteon-simulation/scripts", project_root);

    // Compile TestJavaClient.java
    let compile_output = Command::new("javac")
        .arg("-cp")
        .arg(&jar_path)
        .arg("TestJavaClient.java")
        .current_dir(&compile_dir)
        .output();

    if let Err(e) = compile_output {
        return ClientTestResult {
            language: "Java".to_string(),
            success: true,
            output: format!("Skipped (javac not available: {})", e),
        };
    }

    let compile_out = compile_output.unwrap();
    if !compile_out.status.success() {
        let stderr = String::from_utf8_lossy(&compile_out.stderr);
        return ClientTestResult {
            language: "Java".to_string(),
            success: false,
            output: format!("Compilation failed:\n{}", stderr),
        };
    }

    // Run the test
    let output = Command::new("java")
        .arg("-cp")
        .arg(format!("{}:{}", jar_path, compile_dir))
        .arg("TestJavaClient")
        .env("ACTEON_URL", base_url)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            ClientTestResult {
                language: "Java".to_string(),
                success: out.status.success(),
                output: format!("{}\n{}", stdout, stderr),
            }
        }
        Err(e) => ClientTestResult {
            language: "Java".to_string(),
            success: false,
            output: format!("Failed to run: {}", e),
        },
    }
}

/// Run the native Rust client test
async fn run_rust_client(base_url: &str) -> ClientTestResult {
    use acteon_client::{ActeonClient, AuditQuery};

    let client = ActeonClient::new(base_url);
    let mut passed = 0;
    let mut failed = 0;
    let mut output = String::new();

    output.push_str(&format!("Rust Client Test - connecting to {}\n", base_url));
    output.push_str(&"=".repeat(60));
    output.push('\n');

    // Test: Health
    match client.health().await {
        Ok(true) => {
            output.push_str("  [PASS] health()\n");
            passed += 1;
        }
        Ok(false) | Err(_) => {
            output.push_str("  [FAIL] health()\n");
            failed += 1;
        }
    }

    // Test: Dispatch
    let action = Action::new(
        "test",
        "rust-client",
        "email",
        "send_notification",
        serde_json::json!({"to": "test@example.com", "subject": "Rust test"}),
    );
    match client.dispatch(&action).await {
        Ok(outcome) => {
            if matches!(
                outcome,
                ActionOutcome::Executed(_)
                    | ActionOutcome::Deduplicated
                    | ActionOutcome::Suppressed { .. }
                    | ActionOutcome::Rerouted { .. }
                    | ActionOutcome::Throttled { .. }
                    | ActionOutcome::Failed(_)
            ) {
                output.push_str("  [PASS] dispatch()\n");
                passed += 1;
            } else {
                output.push_str("  [FAIL] dispatch(): unexpected outcome\n");
                failed += 1;
            }
        }
        Err(e) => {
            output.push_str(&format!("  [FAIL] dispatch(): {}\n", e));
            failed += 1;
        }
    }

    // Test: Batch dispatch
    let actions: Vec<Action> = (0..3)
        .map(|i| {
            Action::new(
                "test",
                "rust-client",
                "email",
                "batch_test",
                serde_json::json!({"seq": i}),
            )
        })
        .collect();
    match client.dispatch_batch(&actions).await {
        Ok(results) => {
            if results.len() == 3 {
                output.push_str("  [PASS] dispatch_batch()\n");
                passed += 1;
            } else {
                output.push_str(&format!(
                    "  [FAIL] dispatch_batch(): expected 3, got {}\n",
                    results.len()
                ));
                failed += 1;
            }
        }
        Err(e) => {
            output.push_str(&format!("  [FAIL] dispatch_batch(): {}\n", e));
            failed += 1;
        }
    }

    // Test: List rules
    match client.list_rules().await {
        Ok(_) => {
            output.push_str("  [PASS] list_rules()\n");
            passed += 1;
        }
        Err(e) => {
            output.push_str(&format!("  [FAIL] list_rules(): {}\n", e));
            failed += 1;
        }
    }

    // Test: Query audit
    let query = AuditQuery {
        tenant: Some("rust-client".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    match client.query_audit(&query).await {
        Ok(_) => {
            output.push_str("  [PASS] query_audit()\n");
            passed += 1;
        }
        Err(e) => {
            output.push_str(&format!("  [FAIL] query_audit(): {}\n", e));
            failed += 1;
        }
    }

    output.push_str(&"=".repeat(60));
    output.push('\n');
    output.push_str(&format!("Results: {}/{} passed\n", passed, passed + failed));

    ClientTestResult {
        language: "Rust".to_string(),
        success: failed == 0,
        output,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║           POLYGLOT CLIENT SIMULATION                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Get project root
    let project_root = std::env::current_dir()?.to_string_lossy().to_string();

    // Create recording provider
    let provider = Arc::new(RecordingProvider::new("email"));

    // Start test server
    println!("Starting test server...");
    let server = TestServer::start(Arc::clone(&provider)).await?;
    let base_url = server.url();
    println!("Server running at {}\n", base_url);

    let mut results: Vec<ClientTestResult> = Vec::new();

    // Run Rust client test (async)
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  RUST CLIENT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    let rust_result = run_rust_client(&base_url).await;
    println!("{}", rust_result.output);
    results.push(rust_result);

    // Reset provider call count
    provider.clear();

    // Run Python client test
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  PYTHON CLIENT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    let python_result = run_python_client(&base_url, &project_root);
    println!("{}", python_result.output);
    results.push(python_result);

    provider.clear();

    // Run Node.js client test
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  NODE.JS CLIENT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    let nodejs_result = run_nodejs_client(&base_url, &project_root);
    println!("{}", nodejs_result.output);
    results.push(nodejs_result);

    provider.clear();

    // Run Go client test
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  GO CLIENT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    let go_result = run_go_client(&base_url, &project_root);
    println!("{}", go_result.output);
    results.push(go_result);

    provider.clear();

    // Run Java client test (optional)
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  JAVA CLIENT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    let java_result = run_java_client(&base_url, &project_root);
    println!("{}", java_result.output);
    results.push(java_result);

    // Stop server
    println!("\nStopping test server...");
    server.stop().await;

    // Summary
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                      SUMMARY                                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let mut all_passed = true;
    for result in &results {
        let status = if result.success {
            "✓ PASS"
        } else {
            "✗ FAIL"
        };
        println!("  {} {}", status, result.language);
        if !result.success {
            all_passed = false;
        }
    }

    println!();
    if all_passed {
        println!("All client tests passed!");
    } else {
        println!("Some client tests failed.");
        std::process::exit(1);
    }

    Ok(())
}
