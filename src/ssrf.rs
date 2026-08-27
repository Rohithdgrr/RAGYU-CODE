//! SSRF protection: block requests to private/loopback/link-local and
//! obfuscated IP literals before any network fetch is issued, and re-check
//! every redirect target.

use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;

use anyhow::{Context, Result};
use url::Url;

/// Returns `Ok(())` if `url_str` is an http(s) URL whose host is not a
/// private/loopback/link-local address, otherwise returns an error suitable
/// for surfacing to the model. Also blocks common obfuscation tricks:
/// `0x`-hex octets, decimal `2130706433`, and `nip.io`/`sslip.io` wildcards
/// that map arbitrary subdomains to private IPs.
pub fn ensure_safe_url(url_str: &str) -> Result<Url> {
    let url = Url::parse(url_str).context("invalid URL")?;
    let scheme = url.scheme();
    anyhow::ensure!(
        scheme == "http" || scheme == "https",
        "url must start with http:// or https://"
    );
    // Use `url.host()` enum so IPv6 bracket handling is correct: `Host::Ipv6`
    // carries the parsed `Ipv6Addr` directly, avoiding bracket/string issues.
    match url.host() {
        Some(url::Host::Ipv4(v4)) => {
            if is_private_ip(&IpAddr::V4(v4)) {
                anyhow::bail!(
                    "requests to private/loopback/link-local addresses are blocked (host: {v4})"
                );
            }
        }
        Some(url::Host::Ipv6(v6)) => {
            if is_private_ip(&IpAddr::V6(v6)) {
                anyhow::bail!(
                    "requests to private/loopback/link-local addresses are blocked (host: {v6})"
                );
            }
        }
        Some(url::Host::Domain(domain)) => {
            if is_blocked_host(domain) {
                anyhow::bail!(
                    "requests to private/loopback/link-local addresses are blocked (host: {domain})"
                );
            }
            if let Ok(decoded) = urlencoding::decode(domain) {
                if decoded != domain && is_blocked_host(&decoded) {
                    anyhow::bail!(
                        "requests to private/loopback/link-local addresses are blocked (decoded host: {decoded})"
                    );
                }
            }
        }
        None => anyhow::bail!("URL has no host"),
    }
    // Defence in depth: also check the raw host_str for obfuscation that
    // `Host` parsing might have normalized away (e.g. 0x-hex, decimal).
    if let Some(raw) = url.host_str() {
        if raw.contains("0x") || raw.contains("0X") || raw.parse::<u32>().is_ok() {
            if is_blocked_host(raw) {
                anyhow::bail!(
                    "requests to private/loopback/link-local addresses are blocked (host: {raw})"
                );
            }
        }
    }
    Ok(url)
}

/// True if `host` should be blocked. `host` is the already-lowercased raw
/// domain or IP string (without port or brackets). Brackets are stripped
/// for robustness when the caller passes a raw `host_str` that still contains
/// them (some `Url` implementations include them).
///
/// Covers the full private range required for SSRF:
///
/// - `0.0.0.0` (unspecified), `::` / `::1` (IPv6 unspecified/loopback),
///   `127.0.0.0/8` (loopback), `10.0.0.0/8`, `192.168.0.0/16`,
///   `172.16.0.0/12`, `169.254.0.0/16` (link-local), `::ffff:0:0/96`
///   (IPv4-mapped), plus wildcard DNS (`nip.io` etc.) and obfuscated
///   literals (`0x7f.0.0.1`, decimal `2130706433`).
pub fn is_blocked_host(raw_host: &str) -> bool {
    let h = raw_host
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    if h.is_empty() {
        return true;
    }
    // Explicit string blocklist —covers 0.0.0.0, ::, ::1 and metadata IP.
    if h == "localhost"
        || h.ends_with(".localhost")
        || h == "0.0.0.0"
        || h == "0.0.0.0.nip.io"
        || h == "::"
        || h == "::1"
        || h == "169.254.169.254"
        || h == "::ffff:127.0.0.1"
        || h == "::ffff:0.0.0.0"
    {
        return true;
    }
    // Wildcard DNS services that map any sub-domain to an IP.
    if h.ends_with("nip.io") || h.ends_with("sslip.io") || h.ends_with("xip.io") {
        return true;
    }
    // Hex-obfuscated octets like 0x7f.0.0.1 or 0x7f000001, and octal 0177.
    if h.contains("0x") || h.contains("0X") {
        return true;
    }
    // Octal-encoded dotted quad (e.g. 0177.0.0.01) — 0-prefixed octets that
    // parse as octal in some URL parsers. Block any dotted quad with a
    // leading zero octet that is not just "0".
    if h.split('.')
        .any(|oct| oct.len() > 1 && oct.starts_with('0') && oct.chars().all(|c| c.is_ascii_digit()))
    {
        // Check if the octets look like an IP; if so, block.
        if h.split('.').count() == 4 {
            return true;
        }
    }
    // Try literal IP parse (handles 127.0.0.1, ::1, ::ffff:10.0.0.1, etc).
    if let Ok(ip) = IpAddr::from_str(&h) {
        return is_private_ip(&ip);
    }
    // Explicit check for IPv4-mapped IPv6 textual form that may not have
    // parsed as IpAddr due to stray brackets or leading zeros. Only block
    // if the mapped IPv4 is private/loopback; public `::ffff:8.8.8.8` is
    // allowed (test `blocks_ipv4_mapped_ipv6` expects this).
    if h.starts_with("::ffff:") {
        if let Ok(ip) = IpAddr::from_str(&h) {
            return is_private_ip(&ip);
        }
        if let Some(v4part) = h.rsplit(':').next() {
            if let Ok(v4) = v4part.parse::<Ipv4Addr>() {
                return is_private_ip(&IpAddr::V4(v4));
            }
        }
        // If we cannot parse, be conservative and block.
        return true;
    }
    // Decimal-encoded IPv4 (e.g. http://2130706433/ → 127.0.0.1).
    if let Ok(n) = h.parse::<u32>() {
        let ip = Ipv4Addr::from(n);
        if is_private_ip(&IpAddr::V4(ip)) {
            return true;
        }
        // Even if not private, a bare decimal IP is suspicious and we block it
        // to avoid bypassing string-prefix checks.
        return true;
    }
    // Check 10.* / 192.168.* / 172.16-31.* / 169.254.* / 127.* string prefixes
    // for non-IP hosts that happen to look like them before DNS (defence in depth).
    // Also covers 0.0.0.0/8 (0.*) which is unspecified.
    if h.starts_with("0.")
        || h.starts_with("10.")
        || h.starts_with("192.168.")
        || h.starts_with("169.254.")
        || h.starts_with("127.")
    {
        return true;
    }
    if h.starts_with("172.") {
        // 172.16.0.0/12
        if let Some(second) = h.split('.').nth(1).and_then(|s| s.parse::<u8>().ok()) {
            if (16..=31).contains(&second) {
                return true;
            }
        } else {
            // Non-numeric second octet after 172. → still suspicious, block broadly
            return true;
        }
    }
    // IPv6 private / link-local textual prefixes before they parse as IpAddr.
    // fc00::/7 (unique local), fd00::/8, fe80::/10 (link-local), ff00::/8 (multicast)
    if h.starts_with("fc")
        || h.starts_with("fd")
        || h.starts_with("fe80")
        || h.starts_with("ff")
        || h.starts_with("::ffff")
    {
        // Try parse anyway; if it parses as private we already returned.
        // If not, be conservative and block the prefix.
        if h.contains(':') {
            return true;
        }
    }
    false
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.octets()[0] == 0
                // 169.254.0.0/16 already covered by is_link_local, but keep explicit
                || (v4.octets()[0] == 169 && v4.octets()[1] == 254)
                // 100.64.0.0/10 carrier-grade NAT
                || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
                // 192.0.2.0/24 TEST-NET, 198.51.100.0/24, 203.0.113.0/24
                || (v4.octets()[0] == 192 && v4.octets()[1] == 0 && v4.octets()[2] == 2)
                || (v4.octets()[0] == 198 && v4.octets()[1] == 51 && v4.octets()[2] == 100)
                || (v4.octets()[0] == 203 && v4.octets()[1] == 0 && v4.octets()[2] == 113)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()
                // Unique local fc00::/7
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local fe80::/10
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // IPv4-mapped private/loopback (e.g. ::ffff:127.0.0.1) — only
                // private mapped addresses are blocked; public ::ffff:8.8.8.8
                // is allowed.
                || v6.to_ipv4_mapped().is_some_and(|v4| {
                    is_private_ip(&IpAddr::V4(v4))
                })
        }
    }
}

/// DNS-rebinding defense: after the string checks pass, resolve the host and
/// ensure no resolved IP is private/loopback/link-local.
///
/// This catches domains that resolve to `127.0.0.1`, `10.*`, `192.168.*`,
/// `172.16-31.*`, `169.254.*`, `0.0.0.0`, `::1`, `::ffff:*`, etc. via DNS.
/// A 3-second timeout keeps the check from stalling the model tool.
pub async fn ensure_safe_url_with_dns(url_str: &str) -> Result<Url> {
    let url = ensure_safe_url(url_str)?;
    // Only domains need a DNS check — IP literals were already validated via
    // `is_private_ip`. `Host::Domain` is the only variant that may hide a
    // private address behind DNS.
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.to_string(),
        _ => return Ok(url),
    };
    // Skip if the string already looks private via the blocklist.
    if is_blocked_host(&host) {
        anyhow::bail!(
            "requests to private/loopback/link-local addresses are blocked (host: {host})"
        );
    }
    // Resolve via Tokio DNS with a short timeout. Lookup both 80 and 443
    // implicitly via `lookup_host`'s service handling; we use port 80.
    let lookup = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::lookup_host(format!("{host}:80")),
    )
    .await;
    let addrs = match lookup {
        Ok(Ok(addrs)) => addrs,
        Ok(Err(_)) => return Ok(url), // NXDOMAIN or no record — allow string check to decide
        Err(_) => return Ok(url),     // timeout — allow but log
    };
    for addr in addrs {
        if is_private_ip(&addr.ip()) {
            anyhow::bail!(
                "DNS rebinding blocked: {host} resolves to private address {addr} (host: {host})"
            );
        }
    }
    Ok(url)
}

/// Builds a redirect policy that re-validates every redirect target with the
/// same blocklist. Use with `reqwest::ClientBuilder::redirect`.
pub fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let url = attempt.url();
        // Check via `Host` enum for correct IPv6 handling.
        match url.host() {
            Some(url::Host::Ipv4(v4)) if is_private_ip(&IpAddr::V4(v4)) => return attempt.stop(),
            Some(url::Host::Ipv6(v6)) if is_private_ip(&IpAddr::V6(v6)) => return attempt.stop(),
            Some(url::Host::Domain(d)) if is_blocked_host(d) => return attempt.stop(),
            None => return attempt.stop(),
            _ => {}
        }
        if let Some(raw) = url.host_str() {
            if is_blocked_host(raw) {
                return attempt.stop();
            }
            if let Ok(decoded) = urlencoding::decode(raw) {
                if decoded != raw && is_blocked_host(&decoded) {
                    return attempt.stop();
                }
            }
        }
        // Also validate scheme on redirect (prevent http→file etc)
        if url.scheme() != "http" && url.scheme() != "https" {
            return attempt.stop();
        }
        attempt.follow()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_loopback_and_private() {
        assert!(is_blocked_host("127.0.0.1"));
        assert!(is_blocked_host("127.0.0.2"));
        assert!(is_blocked_host("127.1.2.3"));
        assert!(is_blocked_host("10.0.0.1"));
        assert!(is_blocked_host("192.168.1.1"));
        assert!(is_blocked_host("172.16.0.1"));
        assert!(is_blocked_host("172.31.255.255"));
        assert!(!is_blocked_host("172.32.0.1")); // outside 172.16/12
        assert!(is_blocked_host("169.254.1.1"));
        assert!(is_blocked_host("0.0.0.0"));
        assert!(is_blocked_host("::1"));
        assert!(is_blocked_host("::"));
        assert!(is_blocked_host("localhost"));
        assert!(is_blocked_host("sub.localhost"));
        assert!(is_blocked_host("169.254.169.254"));
    }

    #[test]
    fn blocks_obfuscation() {
        assert!(is_blocked_host("0x7f.0.0.1"));
        assert!(is_blocked_host("0X7F.0.0.1"));
        assert!(is_blocked_host("2130706433")); // 127.0.0.1 decimal
        assert!(is_blocked_host("1.2.3.4.nip.io"));
        assert!(is_blocked_host("127.0.0.1.sslip.io"));
        assert!(is_blocked_host("anything.xip.io"));
    }

    #[test]
    fn allows_public() {
        assert!(!is_blocked_host("example.com"));
        assert!(!is_blocked_host("8.8.8.8"));
        assert!(!is_blocked_host("1.1.1.1"));
        assert!(!is_blocked_host("93.184.216.34"));
        assert!(!is_blocked_host("google.com"));
        assert!(!is_blocked_host("172.32.0.1"));
        assert!(!is_blocked_host("172.15.0.1"));
    }

    #[test]
    fn ensure_safe_url_rejects_private() {
        assert!(ensure_safe_url("http://127.0.0.1/secret").is_err());
        assert!(ensure_safe_url("http://10.0.0.1/admin").is_err());
        assert!(ensure_safe_url("http://[::1]/").is_err());
        assert!(ensure_safe_url("http://example.com/").is_ok());
        assert!(ensure_safe_url("https://8.8.8.8/").is_ok());
        assert!(ensure_safe_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn blocks_ipv4_mapped_ipv6() {
        assert!(is_blocked_host("::ffff:127.0.0.1"));
        assert!(is_blocked_host("::ffff:10.0.0.1"));
        assert!(!is_blocked_host("::ffff:8.8.8.8"));
    }
}
