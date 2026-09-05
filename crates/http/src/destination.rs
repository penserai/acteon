//! Destination classification shared by every guarded HTTP transport.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Why a push URL was rejected as an SSRF risk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SsrfReason {
    InvalidScheme,
    Credentials,
    /// The URL did not parse.
    Unparseable,
    /// The URL has no host component.
    NoHost,
    /// The host (literal, or a resolved address) is in a blocked
    /// range.
    BlockedIp(IpAddr),
    /// The hostname is one of the known-internal names
    /// (`localhost`, `*.internal`, the metadata names, …).
    BlockedHostname(String),
    /// DNS resolution itself failed, or returned no addresses. The
    /// delivery guard treats this as a refusal — a config whose
    /// host cannot be resolved cannot be safely delivered to.
    ResolutionFailed,
}

impl std::fmt::Display for SsrfReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidScheme => write!(f, "URL scheme must be HTTP or HTTPS"),
            Self::Credentials => write!(f, "URL credentials are forbidden"),
            Self::Unparseable => write!(f, "URL did not parse"),
            Self::NoHost => write!(f, "URL has no host"),
            Self::BlockedIp(ip) => write!(f, "host resolves to a blocked address ({ip})"),
            Self::BlockedHostname(h) => write!(f, "hostname '{h}' is internal-only"),
            Self::ResolutionFailed => write!(f, "hostname did not resolve"),
        }
    }
}

/// Classify an IPv4 address as SSRF-blocked. Uses only predicates
/// stable since well before the workspace MSRV (1.88); the
/// carrier-grade-NAT and "this-network" ranges are checked by hand
/// because their std predicates were unstable for a long time.
fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_documentation()
        || o[0] >= 240
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        || ip.is_loopback()            // 127.0.0.0/8
        || ip.is_private()      // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local()   // 169.254.0.0/16 — incl. cloud metadata
        || ip.is_unspecified()  // 0.0.0.0
        || ip.is_broadcast()    // 255.255.255.255
        || ip.is_multicast()    // 224.0.0.0/4
        || o[0] == 0            // 0.0.0.0/8 "this network"
        // Carrier-grade NAT 100.64.0.0/10.
        || (o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000)
}

/// Classify an IPv6 address as SSRF-blocked. Link-local and
/// unique-local are checked by segment mask — the std predicates
/// (`is_unicast_link_local`, `is_unique_local`) were unstable past
/// the MSRV. An IPv4-mapped address is unwrapped and classified as
/// IPv4 so `::ffff:127.0.0.1` cannot smuggle a loopback target.
fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_blocked_v4(v4);
    }
    let seg0 = ip.segments()[0];
    // Only global unicast 2000::/3; reject translation, transition and
    // documentation ranges that can encode internal destinations.
    (seg0 & 0xe000) != 0x2000
        || seg0 == 0x2002
        || (seg0 == 0x2001 && (ip.segments()[1] < 0x200 || ip.segments()[1] == 0xdb8))
        || ip.is_loopback()                 // ::1
        || ip.is_unspecified()       // ::
        || ip.is_multicast()         // ff00::/8
        || (seg0 & 0xfe00) == 0xfc00 // unique-local fc00::/7
        || (seg0 & 0xffc0) == 0xfe80 // link-local fe80::/10
}

/// Classify any IP address as SSRF-blocked.
#[must_use]
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

/// Strip the `[ ]` brackets the `url` crate keeps around an IPv6
/// literal in `host_str()`, so the bare address parses as an
/// `IpAddr`. A domain or IPv4 literal is returned unchanged.
fn unbracket(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
}

/// Reject hostnames that name an internal target without needing
/// DNS. Conservative — it only blocks names that are
/// unambiguously internal; a name that *resolves* to a private IP
/// is caught by [`check_url_resolved`] instead.
///
/// `clippy::case_sensitive_file_extension_comparisons` is allowed:
/// these `.ends_with` calls match DNS-name suffixes, not file
/// extensions, and the host is already lowercased so there is no
/// case-sensitivity bug to flag.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn check_hostname_literal(host: &str) -> Result<(), SsrfReason> {
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    let blocked = lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".internal")
        || lower.ends_with(".local")
        || lower == "metadata"
        || lower == "metadata.google.internal";
    if blocked {
        return Err(SsrfReason::BlockedHostname(host.to_string()));
    }
    Ok(())
}

/// Synchronous, no-DNS SSRF check — used at config **registration**
/// for fast feedback. Catches literal IP targets in a blocked range
/// and obviously-internal hostnames. A hostname that resolves to a
/// private address is *not* caught here — [`check_url_resolved`] is
/// a preflight diagnostic only; transport enforcement uses the guarded resolver.
pub fn check_url_literal(raw: &str) -> Result<(), SsrfReason> {
    let url = reqwest::Url::parse(raw).map_err(|_| SsrfReason::Unparseable)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(SsrfReason::InvalidScheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SsrfReason::Credentials);
    }
    let host = url.host_str().ok_or(SsrfReason::NoHost)?;
    let host = unbracket(host);
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(SsrfReason::BlockedIp(ip));
        }
    } else {
        check_hostname_literal(host)?;
    }
    Ok(())
}

/// Async, DNS-resolving SSRF check — used at **delivery** time and
/// a preflight diagnostic. Resolves the hostname and rejects if
/// *any* returned address is in a blocked range.
pub async fn check_url_resolved(raw: &str) -> Result<(), SsrfReason> {
    check_url_literal(raw)?;
    let url = reqwest::Url::parse(raw).map_err(|_| SsrfReason::Unparseable)?;
    let host = unbracket(url.host_str().ok_or(SsrfReason::NoHost)?).to_string();
    if let Ok(ip) = host.parse::<IpAddr>() {
        // Literal IP — no DNS needed.
        if is_blocked_ip(ip) {
            return Err(SsrfReason::BlockedIp(ip));
        }
        return Ok(());
    }
    // Hostname: reject obviously-internal names, then resolve.
    check_hostname_literal(&host)?;
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| SsrfReason::ResolutionFailed)?;
    let mut saw_any = false;
    for sa in addrs {
        saw_any = true;
        if is_blocked_ip(sa.ip()) {
            return Err(SsrfReason::BlockedIp(sa.ip()));
        }
    }
    if !saw_any {
        return Err(SsrfReason::ResolutionFailed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse().unwrap())
    }
    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse().unwrap())
    }

    #[test]
    fn blocks_loopback_v4() {
        assert!(is_blocked_ip(v4("127.0.0.1")));
        assert!(is_blocked_ip(v4("127.255.255.254")));
    }

    #[test]
    fn blocks_rfc1918_private_ranges() {
        assert!(is_blocked_ip(v4("10.0.0.1")));
        assert!(is_blocked_ip(v4("172.16.0.1")));
        assert!(is_blocked_ip(v4("172.31.255.255")));
        assert!(is_blocked_ip(v4("192.168.1.1")));
    }

    #[test]
    fn blocks_cloud_metadata_address() {
        // 169.254.169.254 — the AWS/GCP/Azure metadata endpoint —
        // falls in the 169.254.0.0/16 link-local range.
        assert!(is_blocked_ip(v4("169.254.169.254")));
        assert!(is_blocked_ip(v4("169.254.0.1")));
    }

    #[test]
    fn blocks_unspecified_broadcast_cgnat_and_this_network() {
        assert!(is_blocked_ip(v4("0.0.0.0")));
        assert!(is_blocked_ip(v4("0.1.2.3"))); // 0.0.0.0/8
        assert!(is_blocked_ip(v4("255.255.255.255")));
        assert!(is_blocked_ip(v4("100.64.0.1"))); // CGNAT
        assert!(is_blocked_ip(v4("100.127.255.255")));
    }

    #[test]
    fn allows_public_v4() {
        assert!(!is_blocked_ip(v4("8.8.8.8")));
        assert!(!is_blocked_ip(v4("1.1.1.1")));
        assert!(!is_blocked_ip(v4("93.184.216.34"))); // example.com
        // 100.63 and 100.128 are just outside the CGNAT 100.64/10.
        assert!(!is_blocked_ip(v4("100.63.255.255")));
        assert!(!is_blocked_ip(v4("100.128.0.0")));
    }

    #[test]
    fn blocks_loopback_and_local_v6() {
        assert!(is_blocked_ip(v6("::1")));
        assert!(is_blocked_ip(v6("::"))); // unspecified
        assert!(is_blocked_ip(v6("fe80::1"))); // link-local
        assert!(is_blocked_ip(v6("fc00::1"))); // unique-local
        assert!(is_blocked_ip(v6("fd12:3456::1"))); // unique-local
    }

    #[test]
    fn blocks_ipv4_mapped_loopback() {
        // ::ffff:127.0.0.1 must not smuggle a loopback target past
        // the v6 path.
        assert!(is_blocked_ip(v6("::ffff:127.0.0.1")));
        assert!(is_blocked_ip(v6("::ffff:10.0.0.1")));
    }

    #[test]
    fn allows_public_v6() {
        assert!(!is_blocked_ip(v6("2606:4700:4700::1111"))); // cloudflare
        assert!(!is_blocked_ip(v6("2001:4860:4860::8888"))); // google
    }

    #[test]
    fn literal_check_rejects_blocked_ip_urls() {
        assert_eq!(
            check_url_literal("http://127.0.0.1/hook"),
            Err(SsrfReason::BlockedIp(v4("127.0.0.1"))),
        );
        assert_eq!(
            check_url_literal("https://169.254.169.254/latest/meta-data/"),
            Err(SsrfReason::BlockedIp(v4("169.254.169.254"))),
        );
        assert_eq!(
            check_url_literal("http://[::1]:9000/x"),
            Err(SsrfReason::BlockedIp(v6("::1"))),
        );
    }

    #[test]
    fn literal_check_rejects_internal_hostnames() {
        for host in [
            "http://localhost/x",
            "http://LocalHost/x",
            "http://db.internal/x",
            "http://service.local/x",
            "http://metadata.google.internal/x",
        ] {
            assert!(
                matches!(check_url_literal(host), Err(SsrfReason::BlockedHostname(_))),
                "expected {host} to be rejected",
            );
        }
    }

    #[test]
    fn literal_check_allows_ordinary_public_urls() {
        assert_eq!(check_url_literal("https://hooks.example.com/a2a"), Ok(()));
        assert_eq!(check_url_literal("https://8.8.8.8/x"), Ok(()));
        assert_eq!(
            check_url_literal("http://my-service.example.org:8080/cb"),
            Ok(()),
        );
    }

    #[test]
    fn literal_check_rejects_unparseable() {
        assert_eq!(check_url_literal("not a url"), Err(SsrfReason::Unparseable),);
    }

    #[tokio::test]
    async fn resolved_check_rejects_literal_loopback() {
        assert_eq!(
            check_url_resolved("http://127.0.0.1:8080/x").await,
            Err(SsrfReason::BlockedIp(v4("127.0.0.1"))),
        );
    }

    #[tokio::test]
    async fn resolved_check_rejects_internal_hostname_without_dns() {
        // `localhost` is rejected by the hostname literal check
        // before any DNS lookup is attempted.
        assert!(matches!(
            check_url_resolved("http://localhost:9000/x").await,
            Err(SsrfReason::BlockedHostname(_)),
        ));
    }

    #[tokio::test]
    async fn resolved_check_rejects_hostname_resolving_to_loopback() {
        // Many resolvers map this name to 127.0.0.1; if this one
        // does, the resolved guard must catch it. If the resolver
        // returns no record (CI sandbox), ResolutionFailed is also
        // a refusal — either way the URL is not allowed through.
        let r = check_url_resolved("http://localtest.me:8080/x").await;
        assert!(
            matches!(
                r,
                Err(SsrfReason::BlockedIp(_)) | Err(SsrfReason::ResolutionFailed),
            ),
            "expected a refusal, got {r:?}",
        );
    }
}
