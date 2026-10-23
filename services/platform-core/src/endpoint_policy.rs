//! Registration-time and probe-time endpoint policy (SSRF hardening).
//!
//! Endpoints must be stable *logical* names (Docker DNS / reverse proxy / Kubernetes Service),
//! never container IPs, loopback or cloud-metadata addresses.

use std::net::IpAddr;

use url::{Host, Url};

pub fn host_allowed(host: &str, allow: &[String]) -> bool {
    allow.iter().any(|a| match a.strip_prefix("*.") {
        Some(suffix) => {
            host.len() > suffix.len() + 1
                && host.ends_with(suffix)
                && host[..host.len() - suffix.len()].ends_with('.')
        }
        None => a.eq_ignore_ascii_case(host),
    })
}

pub fn validate_endpoint(raw: &str, allow: &[String]) -> Result<Url, String> {
    if raw.len() > 256 {
        return Err("endpoint too long".into());
    }
    let url = Url::parse(raw).map_err(|_| "endpoint is not a valid URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("endpoint scheme must be http or https".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("endpoint must not embed credentials".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("endpoint must not contain a query or fragment".into());
    }
    let host = url.host().ok_or("endpoint has no host")?;
    let host_str = match &host {
        Host::Domain(d) => d.to_ascii_lowercase(),
        Host::Ipv4(a) => a.to_string(),
        Host::Ipv6(a) => a.to_string(),
    };
    if !allow.is_empty() {
        return if host_allowed(&host_str, allow) {
            Ok(url)
        } else {
            Err("endpoint host is not in the configured allowlist".into())
        };
    }
    match host {
        Host::Ipv4(_) | Host::Ipv6(_) => {
            Err("IP literal endpoints are not allowed; register a stable DNS name".into())
        }
        Host::Domain(d) => {
            let d = d.to_ascii_lowercase();
            let bad = d == "localhost"
                || d.ends_with(".localhost")
                || d == "metadata"
                || d.starts_with("metadata.")
                || d.ends_with(".metadata.google.internal");
            if bad {
                Err("endpoint host is not permitted".into())
            } else {
                Ok(url)
            }
        }
    }
}

/// Addresses a probe must never connect to when the host is not explicitly allowlisted.
pub fn forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            a.is_loopback()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_multicast()
                || a.is_broadcast()
        }
        IpAddr::V6(a) => {
            a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || (a.segments()[0] & 0xffc0) == 0xfe80 // link-local
                || a.to_ipv4_mapped().is_some_and(|v4| forbidden_ip(IpAddr::V4(v4)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy() {
        assert!(validate_endpoint("http://example-capability:8080", &[]).is_ok());
        assert!(validate_endpoint("http://169.254.169.254/latest", &[]).is_err());
        assert!(validate_endpoint("http://127.0.0.1:80", &[]).is_err());
        assert!(validate_endpoint("http://localhost:80", &[]).is_err());
        assert!(validate_endpoint("http://[::1]:80", &[]).is_err());
        assert!(validate_endpoint("http://metadata.google.internal", &[]).is_err());
        assert!(validate_endpoint("file:///etc/passwd", &[]).is_err());
        assert!(validate_endpoint("http://user:pw@svc:80", &[]).is_err());
        assert!(validate_endpoint("http://svc:80/?x=1", &[]).is_err());
    }

    #[test]
    fn allowlist_overrides() {
        let a = vec!["localhost".to_string(), "*.svc.internal".to_string()];
        assert!(validate_endpoint("http://localhost:1", &a).is_ok());
        assert!(validate_endpoint("http://x.svc.internal", &a).is_ok());
        assert!(validate_endpoint("http://other:1", &a).is_err());
        assert!(!host_allowed("svc.internal", &a));
    }

    #[test]
    fn forbidden_ips() {
        assert!(forbidden_ip("127.0.0.1".parse().unwrap()));
        assert!(forbidden_ip("169.254.169.254".parse().unwrap()));
        assert!(forbidden_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!forbidden_ip("10.0.0.5".parse().unwrap()));
    }
}
