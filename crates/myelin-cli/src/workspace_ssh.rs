use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::{rngs::OsRng, RngCore as _};
use serde_json::json;
use std::path::{Path, PathBuf};

use crate::client::EdgeHttpClient;
use crate::config::EdgeConfig;
use crate::dispatch::{is_canonical_uuid, EdgeCall, HttpMethod, RetryPolicy};
use crate::error::CliError;

mod access;
mod key;
mod openssh;
mod process;
mod profile;

use access::WorkspaceSshGrant;
use key::EphemeralSshKey;
use openssh::OpenSshInvocation;
use profile::WorkspaceSshProfile;

const MAX_REMOTE_COMMAND_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSshCommand {
    thread_id: String,
    remote_command: Option<String>,
    sftp: bool,
    profile: Option<WorkspaceSshProfileRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceSshProfileRequest {
    identity_path: PathBuf,
    public_key_path: Option<PathBuf>,
}

impl WorkspaceSshCommand {
    pub fn parse_agent_args(args: &[String]) -> Result<Option<Self>, CliError> {
        if args.first().map(String::as_str) != Some("thread")
            || !matches!(args.get(1).map(String::as_str), Some("ssh" | "ssh-config"))
        {
            return Ok(None);
        }
        let config_mode = args.get(1).is_some_and(|command| command == "ssh-config");
        let thread_id = args
            .get(2)
            .ok_or_else(|| CliError::Usage("agent thread ssh needs a thread id".into()))?;
        if !is_canonical_uuid(thread_id) {
            return Err(CliError::Usage(
                "agent thread id must be a canonical lowercase UUID".into(),
            ));
        }
        let mut remote_command = None;
        let mut sftp = false;
        let mut identity_path = None;
        let mut public_key_path = None;
        let mut index = 3;
        while index < args.len() {
            match args[index].as_str() {
                "--command" if !config_mode && remote_command.is_none() => {
                    let value = args.get(index + 1).ok_or_else(|| {
                        CliError::Usage("agent thread ssh --command needs a value".into())
                    })?;
                    validate_remote_command(value)?;
                    remote_command = Some(value.clone());
                    index += 2;
                }
                "--command" => {
                    return Err(CliError::Usage(
                        "agent thread ssh accepts --command only once".into(),
                    ));
                }
                "--sftp" if !config_mode && !sftp => {
                    sftp = true;
                    index += 1;
                }
                "--sftp" => {
                    return Err(CliError::Usage(
                        "agent thread ssh accepts --sftp only once".into(),
                    ));
                }
                "--identity" if config_mode && identity_path.is_none() => {
                    identity_path = Some(PathBuf::from(args.get(index + 1).ok_or_else(|| {
                        CliError::Usage("agent thread ssh-config --identity needs a path".into())
                    })?));
                    index += 2;
                }
                "--public-key" if config_mode && public_key_path.is_none() => {
                    public_key_path =
                        Some(PathBuf::from(args.get(index + 1).ok_or_else(|| {
                            CliError::Usage(
                                "agent thread ssh-config --public-key needs a path".into(),
                            )
                        })?));
                    index += 2;
                }
                argument => {
                    return Err(CliError::Usage(format!(
                        "unexpected agent thread ssh argument `{argument}`"
                    )));
                }
            }
        }
        if sftp && remote_command.is_some() {
            return Err(CliError::Usage(
                "agent thread ssh accepts either --sftp or --command, not both".into(),
            ));
        }
        let profile = if config_mode {
            Some(WorkspaceSshProfileRequest {
                identity_path: identity_path.ok_or_else(|| {
                    CliError::Usage("agent thread ssh-config needs --identity <path>".into())
                })?,
                public_key_path,
            })
        } else {
            None
        };
        Ok(Some(Self {
            thread_id: thread_id.clone(),
            remote_command,
            sftp,
            profile,
        }))
    }

    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    pub fn is_profile(&self) -> bool {
        self.profile.is_some()
    }
}

pub async fn enter(
    edge: &EdgeConfig,
    credential: &str,
    request: &WorkspaceSshCommand,
) -> Result<(), CliError> {
    let key = EphemeralSshKey::generate()?;
    let call = access_call(request.thread_id(), key.public_key())?;
    let client = EdgeHttpClient::new()?;
    let value = match client.execute(edge, credential, &call).await {
        Err(error) if error.is_retryable_response_loss() => {
            client.execute(edge, credential, &call).await?
        }
        result => result?,
    };
    let grant = WorkspaceSshGrant::parse(value, &key.fingerprint)?;
    let known_hosts = key.pin_host(&grant.access)?;
    OpenSshInvocation::new(&key, &known_hosts, grant.access, request).run()
}

pub async fn install_profile(
    edge: &EdgeConfig,
    credential: &str,
    config_root: &Path,
    request: &WorkspaceSshCommand,
    json_output: bool,
) -> Result<String, CliError> {
    let profile = request
        .profile
        .as_ref()
        .ok_or_else(|| CliError::Usage("agent thread ssh-config request is missing".into()))?;
    let (public_key, fingerprint) =
        profile::requested_public_key(&profile.identity_path, profile.public_key_path.as_deref())?;
    let call = access_call(request.thread_id(), &public_key)?;
    let client = EdgeHttpClient::new()?;
    let value = match client.execute(edge, credential, &call).await {
        Err(error) if error.is_retryable_response_loss() => {
            client.execute(edge, credential, &call).await?
        }
        result => result?,
    };
    let grant = WorkspaceSshGrant::parse(value, &fingerprint)?;
    WorkspaceSshProfile::install(
        config_root,
        request.thread_id(),
        &profile.identity_path,
        profile.public_key_path.as_deref(),
        grant,
    )?
    .render(json_output)
}

fn access_call(thread_id: &str, public_key: &str) -> Result<EdgeCall, CliError> {
    let mut random = [0_u8; 18];
    OsRng.fill_bytes(&mut random);
    EdgeCall {
        method: HttpMethod::Post,
        path: format!("/v1/agent-threads/{thread_id}/ssh-access"),
        query: None,
        payload: Some(json!({ "public_key": public_key }).to_string().into_bytes()),
        idempotency_key: None,
        retry_policy: RetryPolicy::CallerKeyRequired,
    }
    .with_idempotency_key(&format!("workspace-ssh-{}", URL_SAFE_NO_PAD.encode(random)))
}

fn validate_remote_command(value: &str) -> Result<(), CliError> {
    if value.trim().is_empty()
        || value.len() > MAX_REMOTE_COMMAND_BYTES
        || value.chars().any(|character| character == '\0')
    {
        Err(CliError::Usage(format!(
            "workspace SSH command must contain 1..={MAX_REMOTE_COMMAND_BYTES} UTF-8 bytes and no NUL"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = "22222222-2222-2222-2222-222222222222";

    #[test]
    fn ssh_command_is_explicit_and_bounded() {
        let interactive =
            WorkspaceSshCommand::parse_agent_args(&["thread".into(), "ssh".into(), THREAD.into()])
                .unwrap()
                .unwrap();
        assert_eq!(interactive.thread_id(), THREAD);
        assert_eq!(interactive.remote_command, None);
        assert!(!interactive.sftp);
        assert!(!interactive.is_profile());

        let command = WorkspaceSshCommand::parse_agent_args(&[
            "thread".into(),
            "ssh".into(),
            THREAD.into(),
            "--command".into(),
            "cat notes/continuity.md".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            command.remote_command.as_deref(),
            Some("cat notes/continuity.md")
        );
        assert!(!command.sftp);

        let sftp = WorkspaceSshCommand::parse_agent_args(&[
            "thread".into(),
            "ssh".into(),
            THREAD.into(),
            "--sftp".into(),
        ])
        .unwrap()
        .unwrap();
        assert!(sftp.sftp);
        assert_eq!(sftp.remote_command, None);

        let profile = WorkspaceSshCommand::parse_agent_args(&[
            "thread".into(),
            "ssh-config".into(),
            THREAD.into(),
            "--identity".into(),
            "/keys/editor".into(),
            "--public-key".into(),
            "/keys/editor.pub".into(),
        ])
        .unwrap()
        .unwrap();
        assert!(profile.is_profile());

        for malformed in [
            vec!["thread", "ssh"],
            vec!["thread", "ssh", "not-a-thread"],
            vec!["thread", "ssh", THREAD, "--command"],
            vec!["thread", "ssh", THREAD, "--command", ""],
            vec!["thread", "ssh", THREAD, "--idempotency-key", "x"],
            vec!["thread", "ssh", THREAD, "--sftp", "--sftp"],
            vec!["thread", "ssh", THREAD, "--sftp", "--command", "pwd"],
            vec!["thread", "ssh-config", THREAD],
            vec!["thread", "ssh-config", THREAD, "--identity"],
            vec![
                "thread",
                "ssh-config",
                THREAD,
                "--identity",
                "/key",
                "--sftp",
            ],
            vec![
                "thread",
                "ssh-config",
                THREAD,
                "--identity",
                "/key",
                "--identity",
                "/other",
            ],
        ] {
            let args = malformed.into_iter().map(String::from).collect::<Vec<_>>();
            assert!(
                WorkspaceSshCommand::parse_agent_args(&args).is_err(),
                "accepted {args:?}"
            );
        }
    }
}
