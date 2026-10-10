// SPDX-FileCopyrightText: 2022 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Miscellaneous helpers. Port of `common/helpers`.

/// FlowScope version, set at build time through `FLOWSCOPE_VERSION`.
pub const FLOWSCOPE_VERSION: &str = match option_env!("FLOWSCOPE_VERSION") {
    Some(v) => v,
    None => concat!("v", env!("CARGO_PKG_VERSION")),
};

/// Compiler description, the equivalent of Go's `runtime.Version()`.
pub fn compiler_version() -> &'static str {
    concat!("rustc ", env!("FLOWSCOPE_RUSTC_VERSION"))
}

/// Format an IP the way Go's `netip.Addr.Unmap().String()` does.
pub fn unmap_ip(ip: &std::net::Ipv6Addr) -> String {
    match ip.to_ipv4_mapped() {
        Some(v4) => v4.to_string(),
        None => ip.to_string(),
    }
}

/// Validate a listen address: "host:port", "[v6]:port" or ":port". The host
/// must be an IP or an RFC 1123 hostname.
pub fn validate_listen(s: &str) -> Result<(), String> {
    let (host, port) = split_host_port(s).ok_or_else(|| format!("{s:?} is not a valid listen address"))?;
    match port.parse::<i64>() {
        Ok(p) if (0..=65535).contains(&p) => {}
        _ => return Err(format!("{s:?} has an invalid port")),
    }
    if host.is_empty() || host.parse::<std::net::IpAddr>().is_ok() || is_hostname_rfc1123(host) {
        Ok(())
    } else {
        Err(format!("{s:?} has an invalid host"))
    }
}

/// Split "host:port" like Go's net.SplitHostPort.
pub fn split_host_port(s: &str) -> Option<(&str, &str)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        let port = rest.strip_prefix(':')?;
        if port.contains(':') {
            return None;
        }
        return Some((host, port));
    }
    let (host, port) = s.rsplit_once(':')?;
    if host.contains(':') {
        return None;
    }
    Some((host, port))
}

fn is_hostname_rfc1123(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    let s = s.strip_suffix('.').unwrap_or(s);
    s.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen() {
        for ok in [":8080", "127.0.0.1:161", "[::1]:2055", "localhost:80", "kafka-1.example.com:9092", ":0"] {
            assert!(validate_listen(ok).is_ok(), "{ok}");
        }
        for bad in ["8080", "127.0.0.1", ":65536", "::1:80", "under_score:80", "host:port", ""] {
            assert!(validate_listen(bad).is_err(), "{bad}");
        }
    }
}

pub mod tls;
pub use tls::TlsConfiguration;

pub mod subnetmap;
pub use subnetmap::SubnetMap;

pub mod cache;
