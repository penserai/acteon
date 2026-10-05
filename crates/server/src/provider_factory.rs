//! Qualification from the same immutable inputs used to construct a provider.
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use acteon_core::{ResourceKind, ResourceRef};
use acteon_crypto::tls::LoadedTlsClientConfig;
use acteon_executor::governed::BoundProvider;
use acteon_http::{GuardedClient, OutboundPolicy};
use acteon_provider::{DynProvider, webhook::WebhookProvider};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::config::ProviderConfig;

/// Actual static webhook instance and private qualification inputs. No
/// deserializer or arbitrary client injection can establish this evidence.
pub struct StaticWebhook {
    provider: Arc<dyn DynProvider>,
    inputs: Zeroizing<Vec<u8>>,
    tls: Option<Arc<LoadedTlsClientConfig>>,
}

impl StaticWebhook {
    pub fn build(
        config: &ProviderConfig,
        tls: Option<Arc<LoadedTlsClientConfig>>,
    ) -> Result<Self, String> {
        if config.provider_type != "webhook" {
            return Err("static webhook factory requires webhook configuration".into());
        }
        let mut policy = OutboundPolicy {
            internal_hosts: config
                .internal_hosts
                .iter()
                .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
                .collect(),
        };
        policy.internal_hosts.sort();
        policy.internal_hosts.dedup();
        let destination = policy
            .validate_url(
                config
                    .url
                    .as_deref()
                    .ok_or("webhook requires a destination")?,
            )
            .map_err(|_| "webhook destination refused")?;
        let mut headers = BTreeMap::new();
        for (name, value) in &config.headers {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| "invalid webhook header name")?;
            reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| "invalid webhook header value")?;
            if headers
                .insert(name.as_str().to_owned(), value.clone())
                .is_some()
            {
                return Err("ambiguous webhook header names".into());
            }
        }
        let builder = match &tls {
            Some(loaded) => loaded
                .client_builder()
                .map_err(|_| "invalid webhook TLS material")?,
            None => reqwest::Client::builder().use_rustls_tls(),
        };
        let client = GuardedClient::from_builder(
            builder
                .timeout(Duration::from_secs(30))
                .retry(reqwest::retry::never()),
            policy.clone(),
            false,
        )
        .map_err(|_| "webhook client construction failed")?;
        let inputs = Zeroizing::new(
            serde_json::to_vec(&serde_json::json!({
                "format": "acteon.static_webhook.v1",
                "name": config.name,
                "destination": destination.as_str(),
                "headers": headers,
                "policy": policy,
                "timeout_ms": 30_000,
                "redirects": false,
                "proxies": false,
                "transport_retries": false,
                "adapter_contract": "post-action-json-bounded-response-v1",
            }))
            .map_err(|_| "invalid webhook qualification inputs")?,
        );
        let provider: Arc<dyn DynProvider> = Arc::new(
            WebhookProvider::new(&config.name, destination.as_str())
                .with_client(client)
                .with_headers(headers.into_iter().collect()),
        );
        Ok(Self {
            provider,
            inputs,
            tls,
        })
    }

    /// Share the exact constructed instance with the gateway registry.
    #[must_use]
    pub fn provider(&self) -> Arc<dyn DynProvider> {
        Arc::clone(&self.provider)
    }

    /// Qualify a declared route against this instance's actual immutable inputs.
    /// The host supplies a deployment key, never a request-provided revision.
    pub fn binding(
        &self,
        namespace: &str,
        tenant: &str,
        action_type: &str,
        key: &[u8],
    ) -> Result<BoundProvider, String> {
        if key.len() < 32 {
            return Err("webhook qualification requires a 32-byte deployment key".into());
        }
        let tls_revision = match &self.tls {
            Some(tls) => tls
                .qualification_fingerprint(key)
                .map_err(|_| "webhook TLS qualification failed")?,
            None => "rustls-default-roots-no-client-identity-v1".into(),
        };
        let mut mac =
            Hmac::<Sha256>::new_from_slice(key).map_err(|_| "invalid webhook qualification key")?;
        mac.update(b"acteon.static_webhook.binding.v1");
        for bytes in [self.inputs.as_slice(), tls_revision.as_bytes()] {
            mac.update(&(bytes.len() as u64).to_be_bytes());
            mac.update(bytes);
        }
        let revision = hex::encode(mac.finalize().into_bytes());
        let endpoint = ResourceRef::new(
            ResourceKind::Endpoint,
            namespace,
            tenant,
            format!("webhook/{}", self.provider.name()),
        )
        .map_err(|_| "invalid webhook endpoint identity")?;
        BoundProvider::new_trusted(
            Arc::clone(&self.provider),
            &endpoint,
            action_type,
            &revision,
            Vec::new(),
        )
        .and_then(BoundProvider::qualify_for_catalog)
        .map_err(|_| "invalid webhook route qualification".into())
    }
}
