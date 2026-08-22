use std::net::SocketAddr;
use std::path::{Component, PathBuf};

use myelin_ci_sandbox::gvisor::{ENV_GVISOR_GIT_ROOTFS, ENV_RUNSC_BIN};

pub const ENV_WORKSPACE_SSH_ADDR: &str = "MYELIN_WORKSPACE_SSH_ADDR";
pub const ENV_AGENT_WORKSPACE_ROOT: &str = "MYELIN_AGENT_WORKSPACE_ROOT";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceGatewayRuntimeConfig {
    pub listen_addr: SocketAddr,
    pub workspace_root: PathBuf,
    pub runsc: PathBuf,
    pub gvisor_rootfs: PathBuf,
}

impl WorkspaceGatewayRuntimeConfig {
    pub fn from_env() -> Result<Self, WorkspaceGatewayConfigError> {
        Self::from_reader(|name| std::env::var(name).ok())
    }

    fn from_reader(
        mut read: impl FnMut(&'static str) -> Option<String>,
    ) -> Result<Self, WorkspaceGatewayConfigError> {
        let listen_addr = required(ENV_WORKSPACE_SSH_ADDR, read(ENV_WORKSPACE_SSH_ADDR))?
            .parse::<SocketAddr>()
            .ok()
            .filter(|address| address.port() != 0)
            .ok_or_else(|| {
                invalid(
                    ENV_WORKSPACE_SSH_ADDR,
                    "must be a numeric IP address and nonzero port",
                )
            })?;
        Ok(Self {
            listen_addr,
            workspace_root: absolute_path(
                ENV_AGENT_WORKSPACE_ROOT,
                read(ENV_AGENT_WORKSPACE_ROOT),
            )?,
            runsc: absolute_path(ENV_RUNSC_BIN, read(ENV_RUNSC_BIN))?,
            gvisor_rootfs: absolute_path(ENV_GVISOR_GIT_ROOTFS, read(ENV_GVISOR_GIT_ROOTFS))?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceGatewayConfigError {
    message: String,
}

impl core::fmt::Display for WorkspaceGatewayConfigError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WorkspaceGatewayConfigError {}

fn required(
    name: &'static str,
    value: Option<String>,
) -> Result<String, WorkspaceGatewayConfigError> {
    value
        .filter(|value| !value.is_empty() && value.trim() == value)
        .ok_or_else(|| invalid(name, "is required as nonempty trimmed text"))
}

fn absolute_path(
    name: &'static str,
    value: Option<String>,
) -> Result<PathBuf, WorkspaceGatewayConfigError> {
    let path = PathBuf::from(required(name, value)?);
    if !path.is_absolute()
        || path.parent().is_none()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid(
            name,
            "must be an absolute non-root path without `.` or `..` components",
        ));
    }
    Ok(path)
}

fn invalid(name: &'static str, reason: &str) -> WorkspaceGatewayConfigError {
    WorkspaceGatewayConfigError {
        message: format!("{name} {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn valid_values() -> HashMap<&'static str, String> {
        HashMap::from([
            (ENV_WORKSPACE_SSH_ADDR, "127.0.0.1:2224".into()),
            (
                ENV_AGENT_WORKSPACE_ROOT,
                "/var/lib/myelin/workspaces".into(),
            ),
            (ENV_RUNSC_BIN, "/opt/myelin/bin/runsc".into()),
            (ENV_GVISOR_GIT_ROOTFS, "/opt/myelin/git-rootfs".into()),
        ])
    }

    fn parse(
        values: &HashMap<&'static str, String>,
    ) -> Result<WorkspaceGatewayRuntimeConfig, WorkspaceGatewayConfigError> {
        WorkspaceGatewayRuntimeConfig::from_reader(|name| values.get(name).cloned())
    }

    #[test]
    fn runtime_is_pinned_to_one_numeric_listener_and_persistent_paths() {
        let config = parse(&valid_values()).unwrap();
        assert_eq!(config.listen_addr, "127.0.0.1:2224".parse().unwrap());
        assert_eq!(
            config.workspace_root,
            PathBuf::from("/var/lib/myelin/workspaces")
        );
        assert_eq!(config.runsc, PathBuf::from("/opt/myelin/bin/runsc"));
        assert_eq!(
            config.gvisor_rootfs,
            PathBuf::from("/opt/myelin/git-rootfs")
        );
    }

    #[test]
    fn ambiguous_listeners_and_paths_are_refused_before_startup() {
        for (name, value) in [
            (ENV_WORKSPACE_SSH_ADDR, "localhost:2224"),
            (ENV_WORKSPACE_SSH_ADDR, "127.0.0.1:0"),
            (ENV_AGENT_WORKSPACE_ROOT, "workspaces"),
            (ENV_AGENT_WORKSPACE_ROOT, "/var/lib/../tmp"),
            (ENV_RUNSC_BIN, "runsc"),
            (ENV_GVISOR_GIT_ROOTFS, "/"),
        ] {
            let mut values = valid_values();
            values.insert(name, value.into());
            let error = parse(&values).unwrap_err();
            assert!(error.to_string().starts_with(name), "{error}");
        }
    }
}
