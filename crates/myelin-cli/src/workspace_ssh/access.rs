use std::net::IpAddr;

use chrono::DateTime;
use serde::Deserialize;

use super::key::{ed25519_fingerprint, EphemeralSshKey};
use crate::dispatch::is_canonical_uuid;
use crate::error::CliError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkspaceSshGrant {
    access: WorkspaceSshAccess,
    workspace: WorkspaceReceipt,
    #[serde(rename = "created")]
    _created: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkspaceSshAccess {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) username: String,
    expires_at: String,
    public_key_fingerprint: String,
    pub(super) host_public_key: String,
    host_key_fingerprint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceReceipt {
    id: String,
    generation: u64,
}

impl WorkspaceSshGrant {
    pub(super) fn parse(
        value: serde_json::Value,
        key: &EphemeralSshKey,
    ) -> Result<WorkspaceSshAccess, CliError> {
        let grant: Self = serde_json::from_value(value).map_err(|error| {
            CliError::Transport(format!(
                "Edge returned malformed workspace SSH access: {error}"
            ))
        })?;
        validate_host(&grant.access.host)?;
        if grant.access.port == 0 {
            return Err(unsafe_access("port is zero"));
        }
        validate_username(&grant.access.username)?;
        let expires_at = DateTime::parse_from_rfc3339(&grant.access.expires_at)
            .map_err(|_| unsafe_access("expiry is not RFC 3339"))?;
        if expires_at.timestamp() <= unix_now() {
            return Err(unsafe_access("grant is already expired"));
        }
        if grant.access.public_key_fingerprint != key.fingerprint {
            return Err(unsafe_access(
                "grant is not bound to the generated one-shot key",
            ));
        }
        let host_fingerprint = ed25519_fingerprint(&grant.access.host_public_key, false)?;
        if grant.access.host_key_fingerprint != host_fingerprint {
            return Err(unsafe_access("host key fingerprint does not match its key"));
        }
        if !is_canonical_uuid(&grant.workspace.id) || grant.workspace.generation == 0 {
            return Err(unsafe_access("workspace receipt is not canonical"));
        }
        Ok(grant.access)
    }
}

fn validate_host(host: &str) -> Result<(), CliError> {
    if host.starts_with('[') || host.ends_with(']') {
        let Some(inner) = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
        else {
            return Err(unsafe_access(
                "host is not a bounded DNS name or IP literal",
            ));
        };
        return match inner.parse::<IpAddr>() {
            Ok(IpAddr::V6(_)) => Ok(()),
            _ => Err(unsafe_access(
                "host is not a bounded DNS name or IP literal",
            )),
        };
    }
    let normalized = normalized_host(host);
    if normalized.is_empty() || normalized.len() > 253 || normalized.starts_with('-') {
        return Err(unsafe_access(
            "host is not a bounded DNS name or IP literal",
        ));
    }
    if normalized.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    if normalized.contains(':')
        || normalized.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(unsafe_access(
            "host is not a bounded DNS name or IP literal",
        ));
    }
    Ok(())
}

pub(super) fn normalized_host(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
}

pub(super) fn known_host_name(host: &str, port: u16) -> String {
    let host = normalized_host(host);
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

fn validate_username(username: &str) -> Result<(), CliError> {
    let route = username
        .strip_prefix("ws1_")
        .ok_or_else(|| unsafe_access("username is not an opaque workspace route"))?;
    if route.is_empty()
        || username.len() > 255
        || !route
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(unsafe_access("username is not an opaque workspace route"));
    }
    Ok(())
}

pub(super) fn unsafe_access(detail: &str) -> CliError {
    CliError::Transport(format!(
        "Edge returned unsafe workspace SSH access: {detail}"
    ))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_and_workspace_routes_are_data_not_ssh_options() {
        for host in ["ssh.myelin.example", "127.0.0.1", "::1", "[::1]"] {
            validate_host(host).unwrap();
        }
        for host in ["-oProxyCommand=oops", "host name", "bad..name", "[not-ip]"] {
            assert!(validate_host(host).is_err(), "accepted {host}");
        }
        assert_eq!(known_host_name("[::1]", 2222), "[::1]:2222");
        validate_username("ws1_c2FmZS1yb3V0ZQ").unwrap();
        assert!(validate_username("root").is_err());
    }
}
