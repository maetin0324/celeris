use std::net::Ipv4Addr;

use super::{BrowserPolicyError, valid_host};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowedOrigin {
    scheme: Scheme,
    host: String,
    port: u16,
    wildcard: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    Http,
    Https,
}

impl AllowedOrigin {
    pub fn parse(value: &str) -> Result<Self, BrowserPolicyError> {
        let (scheme, rest) = if let Some(rest) = value.strip_prefix("https://") {
            (Scheme::Https, rest)
        } else if let Some(rest) = value.strip_prefix("http://") {
            (Scheme::Http, rest)
        } else {
            return Err(BrowserPolicyError::InvalidDomain);
        };
        if !rest.is_ascii() || rest.contains(['@', '?', '#', '\\']) {
            return Err(BrowserPolicyError::InvalidDomain);
        }
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.contains('/') {
            return Err(BrowserPolicyError::InvalidDomain);
        }
        let (host, port) = if let Some(tail) = authority.strip_prefix("[::1]") {
            if !tail.is_empty() && !tail.starts_with(':') {
                return Err(BrowserPolicyError::InvalidDomain);
            }
            ("[::1]", tail.strip_prefix(':'))
        } else if let Some((host, port)) = authority.split_once(':') {
            (host, Some(port))
        } else {
            (authority, None)
        };
        let port = match port {
            Some(p) if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) => {
                p.parse::<u16>().ok().filter(|p| *p > 0)
            }
            None => Some(if scheme == Scheme::Https { 443 } else { 80 }),
            _ => None,
        }
        .ok_or(BrowserPolicyError::InvalidDomain)?;
        let host = host.to_ascii_lowercase();
        let (wildcard, base) = match host.strip_prefix("*.") {
            Some(base) => (true, base),
            None => (false, host.as_str()),
        };
        let loopback = matches!(base, "localhost" | "127.0.0.1" | "[::1]");
        if scheme == Scheme::Http && !loopback {
            return Err(BrowserPolicyError::InvalidDomain);
        }
        let valid = base == "[::1]"
            || (valid_host(base)
                && (base == "localhost"
                    || (base.parse::<Ipv4Addr>().is_ok())
                    || (base.contains('.')
                        && !base.bytes().all(|c| c.is_ascii_digit() || c == b'.'))));
        if !valid
            || (wildcard && (loopback || base.parse::<Ipv4Addr>().is_ok() || public_suffix(base)))
        {
            return Err(BrowserPolicyError::InvalidDomain);
        }
        Ok(Self {
            scheme,
            host,
            port,
            wildcard,
        })
    }

    pub fn canonical(&self) -> String {
        let scheme = if self.scheme == Scheme::Https {
            "https"
        } else {
            "http"
        };
        let default_port = if self.scheme == Scheme::Https {
            443
        } else {
            80
        };
        if self.port == default_port {
            format!("{scheme}://{}", self.host)
        } else {
            format!("{scheme}://{}:{}", self.host, self.port)
        }
    }

    pub fn covers(&self, inner: &Self) -> bool {
        self.scheme == inner.scheme
            && self.port == inner.port
            && match (self.wildcard, inner.wildcard) {
                (false, false) => self.host == inner.host,
                (false, true) => false,
                (true, _) => {
                    inner
                        .host
                        .ends_with(&format!(".{}", self.host.trim_start_matches("*.")))
                        || (inner.wildcard && self.host == inner.host)
                }
            }
    }

    pub fn intersection(&self, other: &Self) -> Option<Self> {
        if self.covers(other) {
            Some(other.clone())
        } else if other.covers(self) {
            Some(self.clone())
        } else {
            None
        }
    }
}

pub(crate) fn public_suffix(base: &str) -> bool {
    const PRIVATE_SUFFIXES: &[&str] = &["github.io", "appspot.com", "pages.dev", "cloudfront.net"];
    let labels: Vec<_> = base.split('.').collect();
    labels.len() == 1
        || (labels.len() == 2 && labels[1].len() == 2)
        || PRIVATE_SUFFIXES.contains(&base)
}

/// Read legacy host patterns as HTTPS on port 443 only.
pub fn parse_allowed_origin(value: &str) -> Result<AllowedOrigin, BrowserPolicyError> {
    if value.contains("://") {
        AllowedOrigin::parse(value)
    } else if value.contains(':') {
        Err(BrowserPolicyError::InvalidDomain)
    } else {
        AllowedOrigin::parse(&format!("https://{value}"))
    }
}

pub fn origin_covers(outer: &str, inner: &str) -> bool {
    match (parse_allowed_origin(outer), parse_allowed_origin(inner)) {
        (Ok(outer), Ok(inner)) => outer.covers(&inner),
        _ => false,
    }
}

pub fn intersect_origins(a: &str, b: &str) -> Option<String> {
    parse_allowed_origin(a)
        .ok()?
        .intersection(&parse_allowed_origin(b).ok()?)
        .map(|o| o.canonical())
}

pub fn minimize_origins(origins: Vec<String>) -> Vec<String> {
    let unique: std::collections::BTreeSet<_> = origins.into_iter().collect();
    unique
        .iter()
        .filter(|o| {
            !unique
                .iter()
                .any(|other| other != *o && origin_covers(other, o))
        })
        .cloned()
        .collect()
}
