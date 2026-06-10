//! Shared outbound HTTP egress validation.
//!
//! Target probes and webhook delivery both initiate server-side HTTP requests.
//! This module keeps the network safety boundary separate from route parsing
//! and transport execution.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

use reqwest::Url;

/// Validated outbound HTTP(S) destination with the DNS answer that was checked.
#[derive(Debug, Clone)]
pub(crate) struct ValidatedHttpDestination {
    pub(crate) url: Url,
    pub(crate) host: String,
    pub(crate) addrs: Vec<SocketAddr>,
}

/// Validate a server-side HTTP(S) destination and return a pinned request plan.
pub(crate) fn validate_public_http_destination(
    raw_url: &str,
    surface: &str,
) -> Result<ValidatedHttpDestination, String> {
    let url = Url::parse(raw_url).map_err(|error| format!("{surface} URL is invalid: {error}"))?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => return Err(format!("{surface} URL scheme {scheme} is not supported")),
    }
    if url.username() != "" || url.password().is_some() {
        return Err(format!("{surface} URL must not include credentials"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| format!("{surface} URL is missing a host"))?
        .to_owned();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| format!("{surface} URL is missing a port"))?;

    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(format!("{surface} URL resolves to localhost"));
    }

    let addrs = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| format!("{surface} DNS resolution failed: {error}"))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err(format!("{surface} DNS resolution returned no addresses"));
    }
    for addr in &addrs {
        if !is_global_ip(addr.ip()) {
            return Err(format!(
                "{surface} resolved to non-global address {}",
                addr.ip()
            ));
        }
    }
    Ok(ValidatedHttpDestination { url, host, addrs })
}

fn is_global_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_global_ipv4(ip),
        IpAddr::V6(ip) => is_global_ipv6(ip),
    }
}

fn is_global_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || (a == 100 && (64..=127).contains(&b))
        || a == 0
        || a >= 224
        || (a == 169 && b == 254)
        || (a == 192 && b == 0)
        || (a == 198 && (18..=19).contains(&b))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || (a == 255 && b == 255))
}

fn is_global_ipv6(ip: Ipv6Addr) -> bool {
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || is_unique_local_ipv6(ip)
        || is_unicast_link_local_ipv6(ip)
        || is_documentation_ipv6(ip))
}

fn is_unique_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_unicast_link_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn is_documentation_ipv6(ip: Ipv6Addr) -> bool {
    ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8
}
