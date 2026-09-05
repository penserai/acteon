//! Outbound HTTP with destination checks at URL parsing and connection time.
//! The connector consumes the addresses inspected by the resolver; it does not
//! perform a second lookup. Proxies are disabled so they cannot bypass this check.

pub mod destination;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use serde::{Deserialize, Serialize};

/// Operator-controlled exceptions. Exact hostnames only; never tenant input.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundPolicy {
    /// Hosts permitted to use loopback, private networks, and plain HTTP.
    /// Link-local metadata and other special addresses remain forbidden.
    #[serde(default)]
    pub internal_hosts: Vec<String>,
}

impl OutboundPolicy {
    fn is_internal(&self, host: &str) -> bool {
        let host = host.trim_matches(['[', ']']).trim_end_matches('.');
        self.internal_hosts
            .iter()
            .any(|allowed| allowed.trim_end_matches('.').eq_ignore_ascii_case(host))
    }

    /// Validate the URL before it can enter the connector.
    pub fn validate_url(&self, raw: &str) -> Result<reqwest::Url, OutboundError> {
        let url = reqwest::Url::parse(raw).map_err(|_| OutboundError::Destination)?;
        let host = url.host_str().ok_or(OutboundError::Destination)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || !(url.scheme() == "https" || (url.scheme() == "http" && self.is_internal(host)))
        {
            return Err(OutboundError::Destination);
        }
        if let Ok(ip) = host.trim_matches(['[', ']']).parse() {
            self.validate_address(host, ip)?;
        } else if !self.is_internal(host) {
            destination::check_url_literal(raw).map_err(|_| OutboundError::Destination)?;
        }
        Ok(url)
    }

    fn validate_address(&self, host: &str, ip: IpAddr) -> Result<(), OutboundError> {
        if !destination::is_blocked_ip(ip) {
            return Ok(());
        }
        let internal_address = match ip {
            IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
            IpAddr::V6(ip) => ip.to_ipv4_mapped().map_or_else(
                || ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
                |ip| ip.is_loopback() || ip.is_private(),
            ),
        };
        if self.is_internal(host) && internal_address {
            Ok(())
        } else {
            Err(OutboundError::Destination)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OutboundError {
    #[error("outbound destination refused by network policy")]
    Destination,
    #[error("HTTP client configuration failed: {0}")]
    Client(#[from] reqwest::Error),
}

struct SystemResolver;
impl Resolve for SystemResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addresses: Vec<SocketAddr> =
                tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

struct GuardedResolver {
    policy: OutboundPolicy,
    inner: Arc<dyn Resolve>,
}
impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let pending = self.inner.resolve(name);
        let policy = self.policy.clone();
        Box::pin(async move {
            let addresses: Vec<_> = pending.await?.collect();
            if addresses.is_empty() {
                return Err(OutboundError::Destination.into());
            }
            for address in &addresses {
                policy.validate_address(&host, address.ip())?;
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

/// A client whose raw request entry points cannot skip URL checks.
#[derive(Clone)]
pub struct GuardedClient {
    client: reqwest::Client,
    policy: OutboundPolicy,
}

impl GuardedClient {
    pub fn new(policy: OutboundPolicy, timeout: Duration) -> Result<Self, OutboundError> {
        Self::from_builder(reqwest::Client::builder().timeout(timeout), policy, false)
    }

    /// Apply policy to a trusted TLS/timeout builder. Do not configure address
    /// overrides on the builder. Redirects, when enabled, stay on the same origin.
    pub fn from_builder(
        builder: reqwest::ClientBuilder,
        policy: OutboundPolicy,
        follow_redirects: bool,
    ) -> Result<Self, OutboundError> {
        Self::build(builder, policy, follow_redirects, Arc::new(SystemResolver))
    }

    fn build(
        builder: reqwest::ClientBuilder,
        policy: OutboundPolicy,
        follow_redirects: bool,
        resolver: Arc<dyn Resolve>,
    ) -> Result<Self, OutboundError> {
        let redirect_policy = policy.clone();
        let redirect = reqwest::redirect::Policy::custom(move |attempt| {
            if !follow_redirects {
                return attempt.stop();
            }
            if attempt.previous().len() >= 5
                || redirect_policy
                    .validate_url(attempt.url().as_str())
                    .is_err()
                || attempt
                    .previous()
                    .first()
                    .is_none_or(|origin| origin.origin() != attempt.url().origin())
            {
                attempt.error("redirect refused by outbound policy")
            } else {
                attempt.follow()
            }
        });
        let client = builder
            .no_proxy()
            .redirect(redirect)
            .dns_resolver(Arc::new(GuardedResolver {
                policy: policy.clone(),
                inner: resolver,
            }))
            .build()?;
        Ok(Self { client, policy })
    }

    pub fn request(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, OutboundError> {
        Ok(self.client.request(method, self.policy.validate_url(url)?))
    }

    pub fn post(&self, url: &str) -> Result<reqwest::RequestBuilder, OutboundError> {
        self.request(reqwest::Method::POST, url)
    }
    pub fn head(&self, url: &str) -> Result<reqwest::RequestBuilder, OutboundError> {
        self.request(reqwest::Method::HEAD, url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn urls_and_internal_exceptions_are_exact_and_metadata_is_never_allowed() {
        let policy = OutboundPolicy {
            internal_hosts: vec![
                "localhost".into(),
                "169.254.169.254".into(),
                "127.0.0.1".into(),
            ],
        };
        for url in [
            "https://169.254.169.254/latest",
            "http://localhost.evil.test",
            "https://user:pass@example.com",
            "file:///tmp/x",
            "https://[::ffff:169.254.169.254]/",
            "https://localhost./",
        ] {
            if url == "https://localhost./" {
                assert!(policy.validate_url(url).is_ok());
            } else {
                assert!(policy.validate_url(url).is_err(), "{url}");
            }
        }
        assert!(policy.validate_url("http://127.0.0.1/x").is_ok());
        assert!(
            OutboundPolicy::default()
                .validate_url("http://example.com")
                .is_err()
        );
    }

    struct FakeDns(Vec<SocketAddr>);
    impl Resolve for FakeDns {
        fn resolve(&self, _: Name) -> Resolving {
            let addresses = self.0.clone();
            Box::pin(async move { Ok(Box::new(addresses.into_iter()) as Addrs) })
        }
    }

    #[tokio::test]
    async fn connect_time_resolution_rejects_private_and_mixed_answers() {
        for addresses in [vec!["127.0.0.1:80"], vec!["8.8.8.8:80", "127.0.0.1:80"]] {
            let resolver = GuardedResolver {
                policy: OutboundPolicy::default(),
                inner: Arc::new(FakeDns(
                    addresses.iter().map(|ip| ip.parse().unwrap()).collect(),
                )),
            };
            assert!(
                resolver
                    .resolve("public.example".parse().unwrap())
                    .await
                    .is_err()
            );
        }
        let client = GuardedClient::build(
            reqwest::Client::builder(),
            OutboundPolicy::default(),
            false,
            Arc::new(FakeDns(vec!["127.0.0.1:443".parse().unwrap()])),
        )
        .unwrap();
        assert!(
            client
                .post("https://public.example")
                .unwrap()
                .send()
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn redirects_do_not_forward_secrets_to_another_origin() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://169.254.169.254/latest\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        });
        let policy = OutboundPolicy {
            internal_hosts: vec!["127.0.0.1".into()],
        };
        let client = GuardedClient::from_builder(reqwest::Client::builder(), policy, true).unwrap();
        assert!(
            client
                .post(&format!("http://127.0.0.1:{port}"))
                .unwrap()
                .bearer_auth("secret")
                .send()
                .await
                .is_err()
        );
        server.await.unwrap();
    }
}
