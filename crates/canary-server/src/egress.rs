//! Shared outbound HTTP egress validation.
//!
//! Target probes and webhook delivery both initiate server-side HTTP requests.
//! This module keeps the network safety boundary separate from route parsing
//! and transport execution.
//!
//! `is_global_ip` is the single public-destination oracle for all outbound
//! paths. Do not fork per-path copies — the pre-unification probe and webhook
//! copies drifted (the IPv4-mapped rejection existed on the probe side only,
//! leaving `http://[::ffff:169.254.169.254]/` reachable through webhooks).
//!
//! Any IPv6 form that embeds an IPv4 destination — mapped `::ffff:0:0/96`,
//! compatible `::/96`, Teredo `2001::/32`, 6to4 `2002::/16`, and the NAT64
//! prefixes — is rejected wholesale rather than judged by its embedded
//! address. A translator or transition mechanism sits between the address the
//! filter reads and the host that is actually reached, so the embedded value
//! is not something this filter can hold to the IPv4 rules.

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

    let addrs = resolve_destination_addrs(&host, port, surface)?;
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

/// Resolve a destination host to socket addresses. IP literals (including
/// bracketed IPv6 literals as produced by URL host extraction) are answered
/// directly so the egress filter judges them instead of a DNS lookup error.
pub(crate) fn resolve_destination_addrs(
    host: &str,
    port: u16,
    surface: &str,
) -> Result<Vec<SocketAddr>, String> {
    let literal = host_without_ipv6_brackets(host);
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("{surface} DNS resolution failed: {error}"))?
        .collect::<Vec<_>>();
    if addrs.is_empty() {
        return Err(format!("{surface} DNS resolution returned no addresses"));
    }
    Ok(addrs)
}

/// URL host extraction retains brackets around IPv6 literals. Network APIs
/// that parse an IP or TLS server name require the inner literal instead.
pub(crate) fn host_without_ipv6_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host)
}

/// The one public-destination filter for outbound HTTP. Every server-side
/// egress path (target probes, webhook delivery, and any future outbound
/// surface) must route through this predicate rather than keeping a copy.
pub(crate) fn is_global_ip(ip: IpAddr) -> bool {
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
        || a == 0
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || is_non_global_ietf_protocol_assignment_ipv4(ip)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (18..=19).contains(&b))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113))
}

/// `192.0.0.0/24` is reserved for IETF protocol assignments. IANA marks only
/// the PCP and TURN anycast addresses as globally reachable, so keep those two
/// exact exceptions visible and fail closed for the rest of the block.
fn is_non_global_ietf_protocol_assignment_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, d] = ip.octets();
    a == 192 && b == 0 && c == 0 && d != 9 && d != 10
}

fn is_global_ipv6(ip: Ipv6Addr) -> bool {
    is_allocated_global_unicast_ipv6(ip)
        && !(ip.is_loopback()
            || ip.is_unspecified()
            || ip.is_multicast()
            || is_discard_or_dummy_ipv6(ip)
            || is_non_global_ietf_protocol_assignment_ipv6(ip)
            || is_unique_local_ipv6(ip)
            || is_unicast_link_local_ipv6(ip)
            || is_deprecated_site_local_ipv6(ip)
            || is_documentation_ipv6(ip)
            || ip.segments()[0] == 0x5f00
            || ip.to_ipv4().is_some()
            || is_6to4_ipv6(ip)
            || is_nat64_ipv6(ip))
}

/// IANA currently allocates global IPv6 unicast addresses only from
/// `2000::/3`. Everything outside that allocation fails closed even when it is
/// not yet named in the special-purpose registry; a locally routed reserved
/// prefix is not a public destination.
fn is_allocated_global_unicast_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xe000) == 0x2000
}

/// 6to4 (RFC 3056, 2002::/16) embeds an IPv4 destination in bits 16..48 in
/// cleartext. The filter cannot judge that embedded address as a v4 rule, so
/// the whole prefix is non-global here — same rationale as NAT64 and the
/// `::/96` embedded forms rejected by `to_ipv4`.
fn is_6to4_ipv6(ip: Ipv6Addr) -> bool {
    ip.segments()[0] == 0x2002
}

fn is_unique_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// IANA's discard-only and dummy prefixes are both /64s under `100::/32`.
fn is_discard_or_dummy_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[..4] == [0x0100, 0, 0, 0] || segments[..4] == [0x0100, 0, 0, 1]
}

/// `2001::/23` is reserved for IETF protocol assignments and is non-global
/// unless IANA has made a narrower globally reachable assignment. Keep the
/// small exception list explicit so new assignments fail closed until policy
/// and its table-driven oracle are reviewed together.
fn is_non_global_ietf_protocol_assignment_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    let in_protocol_space = segments[0] == 0x2001 && segments[1] <= 0x01ff;
    in_protocol_space && !is_global_ietf_protocol_assignment_ipv6(segments)
}

fn is_global_ietf_protocol_assignment_ipv6(segments: [u16; 8]) -> bool {
    matches!(segments, [0x2001, 0x0001, 0, 0, 0, 0, 0, 1..=3])
        || (segments[0] == 0x2001 && segments[1] == 0x0003)
        || segments[..3] == [0x2001, 0x0004, 0x0112]
        || (segments[0] == 0x2001 && matches!(segments[1] & 0xfff0, 0x0020 | 0x0030))
}

fn is_unicast_link_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

fn is_deprecated_site_local_ipv6(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfec0
}

fn is_documentation_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] == 0x3fff && (segments[1] & 0xf000) == 0)
}

/// NAT64 translation prefixes: the well-known 64:ff9b::/96 (RFC 6052) and the
/// local-use 64:ff9b:1::/48 (RFC 8215). Both embed an IPv4 destination the
/// filter cannot see through a translator, so the whole prefixes are
/// non-global here.
fn is_nat64_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[0] == 0x0064
        && segments[1] == 0xff9b
        && (segments[2..6] == [0, 0, 0, 0] || segments[2] == 0x0001)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row per address class from the IANA special-purpose registries.
    /// Every outbound path shares `is_global_ip`, so this table is the
    /// egress contract for probes and webhooks alike.
    const BLOCKED: &[(&str, &str)] = &[
        ("127.0.0.1", "IPv4 loopback"),
        ("10.0.0.1", "RFC1918 10/8"),
        ("172.16.0.1", "RFC1918 172.16/12"),
        ("192.168.1.1", "RFC1918 192.168/16"),
        ("169.254.169.254", "IPv4 link-local / metadata"),
        ("100.64.0.1", "CGNAT 100.64/10"),
        ("100.127.255.255", "CGNAT upper bound"),
        ("0.0.0.0", "unspecified"),
        ("0.1.2.3", "this-network 0/8"),
        (
            "192.0.0.8",
            "IANA special 192.0.0/24 below anycast exceptions",
        ),
        ("192.0.0.170", "IANA special 192.0.0/24"),
        ("192.0.2.10", "TEST-NET-1"),
        ("192.88.99.1", "deprecated 6to4 relay anycast"),
        ("198.18.0.1", "benchmarking 198.18/15"),
        ("198.19.255.255", "benchmarking upper bound"),
        ("198.51.100.7", "TEST-NET-2"),
        ("203.0.113.9", "TEST-NET-3"),
        ("224.0.0.1", "IPv4 multicast"),
        ("240.0.0.1", "reserved 240/4"),
        ("255.255.255.255", "broadcast"),
        ("::1", "IPv6 loopback"),
        ("::", "IPv6 unspecified"),
        ("fc00::1", "unique-local fc00::/7"),
        ("fd12:3456::1", "unique-local fd00::/8"),
        ("fe80::1", "IPv6 link-local"),
        ("ff02::1", "IPv6 multicast"),
        ("100::1", "IPv6 discard-only 100::/64"),
        ("100:0:0:1::1", "IPv6 dummy prefix 100:0:0:1::/64"),
        ("2001::1", "Teredo 2001::/32"),
        ("2001:100::1", "unassigned IETF protocol space 2001::/23"),
        ("2001:2::1", "IPv6 benchmarking 2001:2::/48"),
        ("2001:10::1", "deprecated ORCHID 2001:10::/28"),
        ("2001:db8::1", "IPv6 documentation"),
        ("3fff::1", "IPv6 documentation 3fff::/20"),
        ("5f00::1", "SRv6 SID 5f00::/16"),
        ("fec0::1", "deprecated IPv6 site-local fec0::/10"),
        ("::ffff:169.254.169.254", "IPv4-mapped metadata endpoint"),
        ("::ffff:127.0.0.1", "IPv4-mapped loopback"),
        ("::ffff:10.0.0.1", "IPv4-mapped RFC1918"),
        (
            "::ffff:8.8.8.8",
            "IPv4-mapped global (embedded forms rejected wholesale)",
        ),
        ("::169.254.169.254", "IPv4-compatible metadata endpoint"),
        ("::127.0.0.1", "IPv4-compatible loopback"),
        ("::10.0.0.1", "IPv4-compatible RFC1918"),
        (
            "::8.8.8.8",
            "IPv4-compatible global (embedded forms rejected wholesale)",
        ),
        ("2002:7f00:1::", "6to4 embedding loopback"),
        ("2002:a9fe:a9fe::", "6to4 embedding the metadata endpoint"),
        ("2002:808:808::", "6to4 embedding a global address"),
        ("64:ff9b::a00:1", "NAT64 well-known 64:ff9b::/96"),
        ("64:ff9b::808:808", "NAT64 well-known prefix, global embed"),
        ("64:ff9b:1::a00:1", "NAT64 local-use 64:ff9b:1::/48"),
        (
            "64:ff9b:2::1",
            "unallocated IPv6 adjacent to NAT64 local-use",
        ),
        (
            "65:ff9b::1",
            "unallocated IPv6 adjacent to NAT64 well-known",
        ),
        ("4000::1", "unallocated IPv6 above global-unicast 2000::/3"),
    ];

    const ALLOWED: &[(&str, &str)] = &[
        ("93.184.216.34", "public IPv4"),
        ("8.8.8.8", "public IPv4 resolver"),
        ("192.0.0.9", "PCP anycast exception in IANA special space"),
        ("192.0.0.10", "TURN anycast exception in IANA special space"),
        ("192.0.1.1", "global space adjacent to IANA 192.0.0/24"),
        ("198.51.99.1", "global space adjacent to TEST-NET-2"),
        ("203.0.112.1", "global space adjacent to TEST-NET-3"),
        ("100.63.255.255", "global space below CGNAT range"),
        ("100.128.0.0", "global space above CGNAT range"),
        ("2606:4700:4700::1111", "public IPv6 resolver"),
        ("2001:1::1", "PCP anycast in IETF protocol space"),
        ("2001:1::2", "TURN anycast in IETF protocol space"),
        ("2001:1::3", "DNS-SD anycast in IETF protocol space"),
        ("2001:3::1", "AMT in IETF protocol space"),
        ("2001:4:112::1", "AS112-v6 in IETF protocol space"),
        ("2001:20::1", "ORCHIDv2 in IETF protocol space"),
        ("2001:30::1", "Drone Remote ID in IETF protocol space"),
        ("2003::1", "global space adjacent to the 6to4 prefix"),
        (
            "2001:db9::1",
            "global space adjacent to documentation prefix",
        ),
    ];

    #[test]
    fn egress_oracle_blocks_every_non_global_class_and_admits_global_space()
    -> Result<(), std::net::AddrParseError> {
        for (address, class) in BLOCKED {
            let ip: IpAddr = address.parse()?;
            assert!(!is_global_ip(ip), "{class} ({address}) must be blocked");
        }
        for (address, class) in ALLOWED {
            let ip: IpAddr = address.parse()?;
            assert!(is_global_ip(ip), "{class} ({address}) must be allowed");
        }
        Ok(())
    }

    fn assert_rejected_as_non_global(raw_url: &str, surface: &str) -> Result<(), String> {
        match validate_public_http_destination(raw_url, surface) {
            Ok(_) => Err(format!("{raw_url} must be rejected on the {surface} path")),
            Err(error) => {
                assert!(
                    error.contains("non-global address"),
                    "{raw_url}: unexpected rejection reason: {error}"
                );
                Ok(())
            }
        }
    }

    #[test]
    fn webhook_destination_validation_rejects_ipv4_mapped_metadata_literal() -> Result<(), String> {
        assert_rejected_as_non_global("http://[::ffff:169.254.169.254]/", "webhook")
    }

    #[test]
    fn webhook_destination_validation_rejects_nat64_literal() -> Result<(), String> {
        assert_rejected_as_non_global("https://[64:ff9b::a00:1]/hook", "webhook")
    }

    #[test]
    fn destination_validation_accepts_and_pins_global_ipv6_literal() -> Result<(), String> {
        let destination =
            validate_public_http_destination("https://[2606:4700:4700::1111]/hook", "webhook")?;

        assert_eq!(destination.host, "[2606:4700:4700::1111]");
        assert_eq!(
            destination.addrs,
            [SocketAddr::new(
                "2606:4700:4700::1111"
                    .parse()
                    .map_err(|error| format!("test address must parse: {error}"))?,
                443,
            )]
        );
        Ok(())
    }
}
