use std::ffi::OsString;
use std::path::Path;

use super::access::{normalized_host, WorkspaceSshAccess};
use super::key::EphemeralSshKey;
use super::process::{exit_description, isolated_command, missing_openssh};
use super::WorkspaceSshCommand;
use crate::error::CliError;

pub(super) struct OpenSshInvocation {
    program: &'static str,
    args: Vec<OsString>,
}

impl OpenSshInvocation {
    pub(super) fn new(
        key: &EphemeralSshKey,
        known_hosts: &Path,
        access: WorkspaceSshAccess,
        request: &WorkspaceSshCommand,
    ) -> Self {
        let mut args = vec![
            OsString::from("-F"),
            OsString::from("/dev/null"),
            OsString::from("-i"),
            key.private_key().as_os_str().to_owned(),
        ];
        let program = if request.sftp {
            args.extend([
                OsString::from("-P"),
                OsString::from(access.port.to_string()),
                OsString::from("-o"),
                OsString::from(format!("User={}", access.username)),
            ]);
            "sftp"
        } else {
            args.extend([
                OsString::from(if request.remote_command.is_some() {
                    "-T"
                } else {
                    "-tt"
                }),
                OsString::from("-l"),
                OsString::from(access.username),
                OsString::from("-p"),
                OsString::from(access.port.to_string()),
            ]);
            "ssh"
        };
        for option in [
            "BatchMode=yes",
            "IdentitiesOnly=yes",
            "IdentityAgent=none",
            "PasswordAuthentication=no",
            "KbdInteractiveAuthentication=no",
            "StrictHostKeyChecking=yes",
            "GlobalKnownHostsFile=/dev/null",
            "UpdateHostKeys=no",
            "ForwardAgent=no",
            "ForwardX11=no",
            "ClearAllForwardings=yes",
            "PermitLocalCommand=no",
            "ProxyCommand=none",
            "ProxyJump=none",
            "ConnectTimeout=10",
        ] {
            args.push(OsString::from("-o"));
            args.push(OsString::from(option));
        }
        args.push(OsString::from("-o"));
        args.push(OsString::from(format!(
            "UserKnownHostsFile={}",
            known_hosts.display()
        )));
        args.push(OsString::from("--"));
        args.push(OsString::from(normalized_host(&access.host)));
        if let (false, Some(command)) = (request.sftp, request.remote_command.as_deref()) {
            args.push(OsString::from(command));
        }
        Self { program, args }
    }

    pub(super) fn run(&self) -> Result<(), CliError> {
        let status = isolated_command(self.program)
            .args(&self.args)
            .status()
            .map_err(|error| missing_openssh(self.program, error))?;
        if status.success() {
            Ok(())
        } else {
            Err(CliError::Transport(format!(
                "workspace {} exited with {}",
                if self.program == "sftp" {
                    "SFTP"
                } else {
                    "SSH"
                },
                exit_description(status)
            )))
        }
    }
}
