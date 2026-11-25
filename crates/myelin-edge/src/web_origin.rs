//! The web app's origin (`MYELIN_WEB_PUBLIC_URL`). Invitation links and CLI approval links are
//! built from it and carry a bearer secret, so it must be https unless it never leaves this
//! machine.

use std::net::{IpAddr, Ipv6Addr};

const NOT_AN_ORIGIN: &str =
    "MYELIN_WEB_PUBLIC_URL must be an absolute, credential-free HTTP(S) origin";

/// Validates an origin and returns it rebuilt from its parsed scheme, host and port, never the
/// raw text: no trailing slash, and nothing (fragment, query, path, credentials) that could
/// change where a link built from it points.
pub fn validated_web_origin(raw: &str) -> Result<String, String> {
    let origin = parse_origin(raw).ok_or_else(|| NOT_AN_ORIGIN.to_string())?;
    if origin.scheme == "http" && !origin.host.is_loopback() {
        return Err(
            "MYELIN_WEB_PUBLIC_URL must use https; plain http is allowed only for localhost or a \
             loopback address, because the links built from it carry secrets"
                .into(),
        );
    }
    Ok(origin.to_string())
}

pub(crate) enum Host {
    Name(String),
    Ip(IpAddr),
}

impl Host {
    pub(crate) fn is_loopback(&self) -> bool {
        match self {
            Self::Name(name) => name == "localhost",
            Self::Ip(ip) => ip.is_loopback(),
        }
    }

    /// The host as a resolver or TLS name check takes it: an IPv6 literal without brackets.
    pub(crate) fn unbracketed(&self) -> String {
        match self {
            Self::Name(name) => name.clone(),
            Self::Ip(ip) => ip.to_string(),
        }
    }
}

struct Origin {
    scheme: &'static str,
    host: Host,
    port: Option<u16>,
}

impl core::fmt::Display for Origin {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{}://", self.scheme)?;
        match &self.host {
            Host::Name(name) => formatter.write_str(name)?,
            Host::Ip(IpAddr::V6(ip)) => write!(formatter, "[{ip}]")?,
            Host::Ip(ip) => write!(formatter, "{ip}")?,
        }
        match self.port {
            Some(port) => write!(formatter, ":{port}"),
            None => Ok(()),
        }
    }
}

fn parse_origin(raw: &str) -> Option<Origin> {
    let (scheme, rest) = raw.split_once("://")?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "http" => "http",
        "https" => "https",
        _ => return None,
    };
    // Only a bare or root path may follow the authority. A fragment or query is refused wherever
    // it sits, and so is anything that would make the authority mean something else.
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if !matches!(path, "" | "/") || authority.contains(['#', '?', '@', '\\', '%']) {
        return None;
    }
    let (host, port) = split_host_port(authority)?;
    Some(Origin { scheme, host, port })
}

pub(crate) fn split_host_port(authority: &str) -> Option<(Host, Option<u16>)> {
    // Brackets are for IPv6 literals only, so `[localhost]` and `[127.0.0.1]` are refused.
    if let Some(bracketed) = authority.strip_prefix('[') {
        let (literal, after) = bracketed.split_once(']')?;
        let ip: Ipv6Addr = literal.parse().ok()?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?),
        };
        return Some((Host::Ip(IpAddr::V6(ip)), parse_port(port)?));
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    Some((parse_host(host)?, parse_port(port)?))
}

fn parse_host(host: &str) -> Option<Host> {
    let plain = host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'));
    if host.is_empty() || !plain {
        return None;
    }
    Some(match host.parse::<IpAddr>() {
        Ok(ip) => Host::Ip(ip),
        Err(_) => Host::Name(host.to_ascii_lowercase()),
    })
}

/// `None` is no port; a port that is present must be 1 to 65535 in plain digits.
fn parse_port(port: Option<&str>) -> Option<Option<u16>> {
    let Some(port) = port else {
        return Some(None);
    };
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    match port.parse::<u16>().ok()? {
        0 => None,
        port => Some(Some(port)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_anywhere_and_http_only_on_loopback() {
        for accepted in [
            "https://myelin.example",
            "https://myelin.example/",
            "https://10.0.0.5:8443",
            "https://[2001:db8::1]:8443",
            "http://127.0.0.1:3000",
            "http://127.8.9.10",
            "http://localhost:3000/",
            "http://LOCALHOST",
            "http://[::1]:3000",
        ] {
            assert!(validated_web_origin(accepted).is_ok(), "{accepted}");
        }
        for refused in [
            "http://myelin.example",
            "http://10.0.0.5:3000",
            "http://0.0.0.0:3000",
            "http://localhost.myelin.example",
            "http://127.0.0.1.nip.io",
            "http://[::2]",
            "ftp://myelin.example",
            "https://user@myelin.example",
            "https://myelin.example/app",
            "https://myelin.example?x=1",
            "myelin.example",
        ] {
            assert!(validated_web_origin(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn the_origin_is_rebuilt_not_echoed() {
        for (raw, origin) in [
            ("https://myelin.example/", "https://myelin.example"),
            ("HTTPS://MyElin.Example", "https://myelin.example"),
            (
                "https://myelin.example:8443/",
                "https://myelin.example:8443",
            ),
            (
                "https://myelin.example:08443",
                "https://myelin.example:8443",
            ),
            ("http://LOCALHOST:3000", "http://localhost:3000"),
            ("http://[::1]:3000/", "http://[::1]:3000"),
            ("http://[0:0:0:0:0:0:0:1]", "http://[::1]"),
            ("https://[2001:DB8::1]", "https://[2001:db8::1]"),
            ("http://127.0.0.1", "http://127.0.0.1"),
        ] {
            assert_eq!(validated_web_origin(raw).as_deref(), Ok(origin), "{raw}");
        }
    }

    #[test]
    fn nothing_after_the_authority_can_redirect_a_link() {
        for refused in [
            "https://x.example/#frag",
            "https://x.example#frag",
            "https://x.example/#",
            "https://x.example:8443#frag",
            "https://x.example?",
            "https://x.example/?",
            "https://x.example/?next=1",
            "https://x.example//",
            "https://x.example/a",
            "https://x.example/a/",
            "https://x.example/%2e",
            "https://x.example\\@evil.example",
            "https://x.example%40evil.example",
            "https://x.example evil.example",
            "https://x.example\n",
        ] {
            assert!(validated_web_origin(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn a_host_is_required_and_brackets_are_for_ipv6_only() {
        for refused in [
            "https://",
            "https:///",
            "https://:80",
            "http://:3000",
            "http://[localhost]",
            "http://[127.0.0.1]",
            "https://[x.example]",
            "https://[2001:db8::1",
            "https://[]",
            "https://[::1]x",
            "https://[::1]:",
            "https://2001:db8::1",
            "https://exa mple.test",
        ] {
            assert!(validated_web_origin(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn a_port_is_a_plain_number_from_1_to_65535() {
        for accepted in ["https://x.example:1", "https://x.example:65535"] {
            assert!(validated_web_origin(accepted).is_ok(), "{accepted}");
        }
        for refused in [
            "https://x.example:",
            "https://x.example:0",
            "https://x.example:65536",
            "https://x.example:99999999999",
            "https://x.example:abc",
            "https://x.example:80a",
            "https://x.example:+80",
            "https://x.example:-1",
            "https://x.example:80:80",
            "https://x.example: 80",
        ] {
            assert!(validated_web_origin(refused).is_err(), "{refused:?}");
        }
    }
}
