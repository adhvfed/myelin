use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chrono::DateTime;
use rand::{rngs::OsRng, RngCore as _};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

use crate::client::EdgeHttpClient;
use crate::config::EdgeConfig;
use crate::dispatch::{is_canonical_uuid, EdgeCall, HttpMethod, RetryPolicy};
use crate::error::CliError;

const MAX_REMOTE_COMMAND_BYTES: usize = 32 * 1024;
const MAX_PUBLIC_KEY_BYTES: usize = 4 * 1024;

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
    let invocation = OpenSshInvocation::new(&key, &known_hosts, access, request);
    let status = invocation.run()?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Transport(format!(
            "workspace SSH exited with {}",
            exit_description(status)
        )))
    }
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

struct EphemeralSshKey {
    directory: TempDir,
    private_key: PathBuf,
    public_key: String,
    fingerprint: String,
}

impl EphemeralSshKey {
    fn generate() -> Result<Self, CliError> {
        let directory = tempfile::Builder::new()
            .prefix("myelin-workspace-ssh-")
            .tempdir()
            .map_err(|error| {
                CliError::Config(format!(
                    "could not create temporary SSH key directory: {error}"
                ))
            })?;
        let private_key = directory.path().join("id_ed25519");
        let output = isolated_command("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "myelin-one-shot",
                "-f",
            ])
            .arg(&private_key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| missing_openssh("ssh-keygen", error))?;
        if !output.status.success() {
            return Err(CliError::Unsupported(format!(
                "ssh-keygen could not create a one-shot Ed25519 key ({})",
                exit_description(output.status)
            )));
        }
        let encoded = fs::read(private_key.with_extension("pub")).map_err(|error| {
            CliError::Config(format!(
                "could not read the temporary SSH public key: {error}"
            ))
        })?;
        if encoded.len() > MAX_PUBLIC_KEY_BYTES {
            return Err(CliError::Config(
                "ssh-keygen returned an unexpectedly large public key".into(),
            ));
        }
        let public_key = String::from_utf8(encoded)
            .map_err(|_| CliError::Config("ssh-keygen returned a non-UTF-8 public key".into()))?;
        let public_key = public_key.trim_end_matches(['\r', '\n']).to_string();
        let fingerprint = ed25519_fingerprint(&public_key, true)?;
        Ok(Self {
            directory,
            private_key,
            public_key,
            fingerprint,
        })
    }

    fn public_key(&self) -> &str {
        &self.public_key
    }

    fn pin_host(&self, access: &WorkspaceSshAccess) -> Result<PathBuf, CliError> {
        let known_hosts = self.directory.path().join("known_hosts");
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&known_hosts).map_err(|error| {
            CliError::Config(format!(
                "could not create the temporary known-hosts file: {error}"
            ))
        })?;
        writeln!(
            file,
            "{} {}",
            known_host_name(&access.host, access.port),
            access.host_public_key
        )
        .map_err(|error| {
            CliError::Config(format!("could not pin the workspace SSH host key: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            CliError::Config(format!(
                "could not persist the workspace SSH host pin: {error}"
            ))
        })?;
        Ok(known_hosts)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceSshGrant {
    access: WorkspaceSshAccess,
    workspace: WorkspaceReceipt,
    created: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceSshAccess {
    host: String,
    port: u16,
    username: String,
    expires_at: String,
    public_key_fingerprint: String,
    host_public_key: String,
    host_key_fingerprint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceReceipt {
    id: String,
    generation: u64,
}

impl WorkspaceSshGrant {
    fn parse(
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
            return Err(malformed_access("port is zero"));
        }
        validate_username(&grant.access.username)?;
        let expires_at = DateTime::parse_from_rfc3339(&grant.access.expires_at)
            .map_err(|_| malformed_access("expiry is not RFC 3339"))?;
        if expires_at.timestamp() <= unix_now() {
            return Err(malformed_access("grant is already expired"));
        }
        if grant.access.public_key_fingerprint != key.fingerprint {
            return Err(malformed_access(
                "grant is not bound to the generated one-shot key",
            ));
        }
        let host_fingerprint = ed25519_fingerprint(&grant.access.host_public_key, false)?;
        if grant.access.host_key_fingerprint != host_fingerprint {
            return Err(malformed_access(
                "host key fingerprint does not match its key",
            ));
        }
        if !is_canonical_uuid(&grant.workspace.id) || grant.workspace.generation == 0 {
            return Err(malformed_access("workspace receipt is not canonical"));
        }
        let _ = grant.created;
        Ok(grant.access)
    }
}

struct OpenSshInvocation {
    args: Vec<OsString>,
}

impl OpenSshInvocation {
    fn new(
        key: &EphemeralSshKey,
        known_hosts: &Path,
        access: WorkspaceSshAccess,
        request: &WorkspaceSshCommand,
    ) -> Self {
        let mut args = vec![
            OsString::from("-F"),
            OsString::from("/dev/null"),
            OsString::from(if request.remote_command.is_some() {
                "-T"
            } else {
                "-tt"
            }),
            OsString::from("-i"),
            key.private_key.as_os_str().to_owned(),
            OsString::from("-l"),
            OsString::from(access.username),
            OsString::from("-p"),
            OsString::from(access.port.to_string()),
        ];
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
        if let Some(command) = request.remote_command.as_deref() {
            args.push(OsString::from(command));
        }
        Self { args }
    }

    fn run(&self) -> Result<ExitStatus, CliError> {
        isolated_command("ssh")
            .args(&self.args)
            .status()
            .map_err(|error| missing_openssh("ssh", error))
    }
}

fn isolated_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    for name in ["PATH", "TERM", "LANG", "LC_ALL", "LC_CTYPE", "COLORTERM"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

fn missing_openssh(program: &str, error: std::io::Error) -> CliError {
    if error.kind() == std::io::ErrorKind::NotFound {
        CliError::Unsupported(format!(
            "agent workspace access needs OpenSSH `{program}` on PATH"
        ))
    } else {
        CliError::Transport(format!("could not start `{program}`: {error}"))
    }
}

fn exit_description(status: ExitStatus) -> String {
    status
        .code()
        .map(|code| format!("status {code}"))
        .unwrap_or_else(|| "a signal".into())
}

fn validate_host(host: &str) -> Result<(), CliError> {
    if host.starts_with('[') || host.ends_with(']') {
        let Some(inner) = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
        else {
            return Err(malformed_access(
                "host is not a bounded DNS name or IP literal",
            ));
        };
        return match inner.parse::<IpAddr>() {
            Ok(IpAddr::V6(_)) => Ok(()),
            _ => Err(malformed_access(
                "host is not a bounded DNS name or IP literal",
            )),
        };
    }
    let normalized = normalized_host(host);
    if normalized.is_empty() || normalized.len() > 253 || normalized.starts_with('-') {
        return Err(malformed_access(
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
        return Err(malformed_access(
            "host is not a bounded DNS name or IP literal",
        ));
    }
    Ok(())
}

fn normalized_host(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
}

fn known_host_name(host: &str, port: u16) -> String {
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
        .ok_or_else(|| malformed_access("username is not an opaque workspace route"))?;
    if route.is_empty()
        || username.len() > 255
        || !route
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(malformed_access(
            "username is not an opaque workspace route",
        ));
    }
    Ok(())
}

fn ed25519_fingerprint(public_key: &str, allow_comment: bool) -> Result<String, CliError> {
    if public_key.is_empty()
        || public_key.len() > MAX_PUBLIC_KEY_BYTES
        || public_key.trim() != public_key
        || public_key.chars().any(char::is_control)
    {
        return Err(malformed_access("SSH public key is not one bounded line"));
    }
    let fields = public_key.split_ascii_whitespace().collect::<Vec<_>>();
    let expected_fields = if allow_comment { 2..=3 } else { 2..=2 };
    if !expected_fields.contains(&fields.len()) || fields[0] != "ssh-ed25519" {
        return Err(malformed_access("SSH public key is not Ed25519"));
    }
    let blob = STANDARD
        .decode(fields[1])
        .map_err(|_| malformed_access("SSH public key is not canonical base64"))?;
    if STANDARD.encode(&blob) != fields[1] || !canonical_ed25519_blob(&blob) {
        return Err(malformed_access(
            "SSH public key blob is not canonical Ed25519",
        ));
    }
    Ok(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(&blob))
    ))
}

fn canonical_ed25519_blob(blob: &[u8]) -> bool {
    let mut cursor = 0;
    read_ssh_string(blob, &mut cursor).is_some_and(|value| value == b"ssh-ed25519")
        && read_ssh_string(blob, &mut cursor).is_some_and(|value| value.len() == 32)
        && cursor == blob.len()
}

fn read_ssh_string<'a>(blob: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    let length_end = cursor.checked_add(4)?;
    let length = u32::from_be_bytes(blob.get(*cursor..length_end)?.try_into().ok()?) as usize;
    let value_end = length_end.checked_add(length)?;
    let value = blob.get(length_end..value_end)?;
    *cursor = value_end;
    Some(value)
}

fn malformed_access(detail: &str) -> CliError {
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
