//! Durable secret-reference lifecycle simulation.
//!
//! Run with:
//! `cargo run -p acteon-simulation --example secret_custody_simulation`

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use acteon_core::{PushAuthentication, TaskPushNotificationConfig};
use acteon_crypto::{ExposeSecret, SecretString};
use acteon_secrets::{SecretError, SecretProvider, SecretRef, SecretResolver, SecretUsage};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;
use async_trait::async_trait;
use sha2::{Digest, Sha256};

#[derive(Default)]
struct RotatingProvider {
    values: RwLock<HashMap<(String, String), SecretString>>,
}

impl RotatingProvider {
    fn install(&self, key: &str, version: &str, value: &str) {
        self.values.write().unwrap().insert(
            (key.into(), version.into()),
            SecretString::new(value.to_owned().into()),
        );
    }

    fn revoke(&self, key: &str, version: &str) {
        self.values
            .write()
            .unwrap()
            .remove(&(key.into(), version.into()));
    }
}

#[async_trait]
impl SecretProvider for RotatingProvider {
    async fn resolve(
        &self,
        reference: &SecretRef,
        usage: &SecretUsage,
    ) -> Result<SecretString, SecretError> {
        if usage.namespace != "observability"
            || usage.tenant != "acme"
            || usage.purpose != "a2a.callback.authorization"
            || usage.audience != "https://incident-router.example/a2a"
        {
            return Err(SecretError::Refused);
        }
        let version = reference.version.as_deref().ok_or(SecretError::Refused)?;
        self.values
            .read()
            .unwrap()
            .get(&(reference.key.clone(), version.into()))
            .cloned()
            .ok_or(SecretError::Refused)
    }
}

fn fingerprint(secret: &SecretString) -> String {
    hex::encode(Sha256::digest(secret.expose_secret().as_bytes()))[..12].to_owned()
}

#[tokio::main]
async fn main() {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let provider = Arc::new(RotatingProvider::default());
    provider.install("callbacks/incident-router", "v1", "credential-one");

    let mut resolver = SecretResolver::new();
    resolver.register("demo-vault", provider.clone()).unwrap();

    let mut callback = TaskPushNotificationConfig::new(
        "incident-callback",
        "task-0199",
        "observability",
        "acme",
        "https://incident-router.example/a2a",
    );
    callback.authentication = Some(PushAuthentication {
        schemes: vec!["Bearer".into()],
        credentials: None,
        credentials_ref: Some(
            SecretRef::new("demo-vault", "callbacks/incident-router").with_version("v1"),
        ),
    });
    callback.validate().unwrap();

    let key = StateKey::new(
        "observability",
        "acme",
        KeyKind::A2aTaskPushConfig,
        callback.storage_id(),
    );
    let encoded = serde_json::to_string(&callback).unwrap();
    assert!(!encoded.contains("credential-one"));
    store.set(&key, &encoded, None).await.unwrap();
    println!("✓ persisted callback contains a versioned reference and no credential bytes");

    let restored: TaskPushNotificationConfig =
        serde_json::from_str(&store.get(&key).await.unwrap().unwrap()).unwrap();
    let v1 = restored
        .authentication
        .as_ref()
        .unwrap()
        .credentials_ref
        .as_ref()
        .unwrap();
    let usage = SecretUsage::new(
        "observability",
        "acme",
        "a2a.callback.authorization",
        "https://incident-router.example/a2a",
    );
    println!(
        "✓ worker restart resolved v1 at the effect boundary (sha256={})",
        fingerprint(&resolver.resolve(v1, &usage).await.unwrap())
    );

    provider.install("callbacks/incident-router", "v2", "credential-two");
    callback.authentication.as_mut().unwrap().credentials_ref =
        Some(SecretRef::new("demo-vault", "callbacks/incident-router").with_version("v2"));
    store
        .set(&key, &serde_json::to_string(&callback).unwrap(), None)
        .await
        .unwrap();
    let v2 = callback
        .authentication
        .as_ref()
        .unwrap()
        .credentials_ref
        .as_ref()
        .unwrap();
    println!(
        "✓ reference rotation selected v2 (sha256={})",
        fingerprint(&resolver.resolve(v2, &usage).await.unwrap())
    );

    provider.revoke("callbacks/incident-router", "v1");
    assert!(matches!(
        resolver.resolve(v1, &usage).await,
        Err(SecretError::Refused)
    ));
    assert!(resolver.resolve(v2, &usage).await.is_ok());
    println!("✓ revoked v1 fails closed while v2 remains deliverable");

    let redirected = SecretUsage::new(
        "observability",
        "acme",
        "a2a.callback.authorization",
        "https://attacker.example/collect",
    );
    assert!(matches!(
        resolver.resolve(v2, &redirected).await,
        Err(SecretError::Refused)
    ));
    println!("✓ redirecting the callback audience cannot exfiltrate the credential");
    println!("\nSECRET CUSTODY SIMULATION PASSED");
}
