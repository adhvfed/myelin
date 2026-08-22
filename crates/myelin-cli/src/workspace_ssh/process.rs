use std::process::{Command, ExitStatus};

use crate::error::CliError;

pub(super) fn isolated_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    for name in ["PATH", "TERM", "LANG", "LC_ALL", "LC_CTYPE", "COLORTERM"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

pub(super) fn missing_openssh(program: &str, error: std::io::Error) -> CliError {
    if error.kind() == std::io::ErrorKind::NotFound {
        CliError::Unsupported(format!(
            "agent workspace access needs OpenSSH `{program}` on PATH"
        ))
    } else {
        CliError::Transport(format!("could not start `{program}`: {error}"))
    }
}

pub(super) fn exit_description(status: ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("status {code}"))
        .unwrap_or_else(|| "a signal".into())
}
