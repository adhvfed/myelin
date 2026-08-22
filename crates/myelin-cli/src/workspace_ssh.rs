use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::{rngs::OsRng, RngCore as _};
use serde_json::json;

use crate::client::EdgeHttpClient;
use crate::config::EdgeConfig;
use crate::dispatch::{is_canonical_uuid, EdgeCall, HttpMethod, RetryPolicy};
use crate::error::CliError;

mod access;
mod key;
mod openssh;
mod process;

use access::WorkspaceSshGrant;
use key::EphemeralSshKey;
use openssh::OpenSshInvocation;

const MAX_REMOTE_COMMAND_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSshCommand {
    thread_id: String,
    remote_command: Option<String>,
}

impl WorkspaceSshCommand {
    pub fn parse_agent_args(args: &[String]) -> Result<Option<Self>, CliError> {
        if args.first().map(String::as_str) != Some("thread")
            || args.get(1).map(String::as_str) != Some("ssh")
        {
            return Ok(None);
        }
        let thread_id = args
            .get(2)
            .ok_or_else(|| CliError::Usage("agent thread ssh needs a thread id".into()))?;
        if !is_canonical_uuid(thread_id) {
            return Err(CliError::Usage(
                "agent thread id must be a canonical lowercase UUID".into(),
            ));
        }
        let mut remote_command = None;
        let mut index = 3;
        while index < args.len() {
            match args[index].as_str() {
                "--command" if remote_command.is_none() => {
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
                argument => {
                    return Err(CliError::Usage(format!(
                        "unexpected agent thread ssh argument `{argument}`"
                    )));
                }
            }
        }
        Ok(Some(Self {
            thread_id: thread_id.clone(),
            remote_command,
        }))
    }

    pub fn thread_id(&self) -> &str {
        &self.thread_id
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
    let access = WorkspaceSshGrant::parse(value, &key)?;
    let known_hosts = key.pin_host(&access)?;
    OpenSshInvocation::new(&key, &known_hosts, access, request).run()
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

        for malformed in [
            vec!["thread", "ssh"],
            vec!["thread", "ssh", "not-a-thread"],
            vec!["thread", "ssh", THREAD, "--command"],
            vec!["thread", "ssh", THREAD, "--command", ""],
            vec!["thread", "ssh", THREAD, "--idempotency-key", "x"],
        ] {
            let args = malformed.into_iter().map(String::from).collect::<Vec<_>>();
            assert!(
                WorkspaceSshCommand::parse_agent_args(&args).is_err(),
                "accepted {args:?}"
            );
        }
    }
}
