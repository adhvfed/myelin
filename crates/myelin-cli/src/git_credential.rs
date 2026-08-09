use crate::config::Credential;
use crate::error::CliError;
use hyper::Uri;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

const MAX_CREDENTIAL_REQUEST_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Get,
    Store,
    Erase,
}

impl Operation {
    pub fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "get" => Ok(Self::Get),
            "store" => Ok(Self::Store),
            "erase" => Ok(Self::Erase),
            _ => Err(CliError::Usage(
                "git credential operation must be get, store, or erase".into(),
            )),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CredentialRequest {
    protocol: Option<String>,
    host: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CredentialScope {
    protocol: String,
    host: String,
    edge_origin: String,
}

impl CredentialScope {
    pub fn from_edge_url(edge_url: &str) -> Result<Self, CliError> {
        let edge_url = edge_url.trim().trim_end_matches('/');
        if edge_url.contains('#') {
            return invalid_edge_origin();
        }
        let uri: Uri = edge_url.parse().map_err(|_| invalid_edge_origin_error())?;
        let protocol = uri
            .scheme_str()
            .filter(|scheme| matches!(*scheme, "http" | "https"))
            .ok_or_else(invalid_edge_origin_error)?;
        let authority = uri.authority().ok_or_else(invalid_edge_origin_error)?;
        if authority.as_str().contains('@')
            || uri.query().is_some()
            || !matches!(uri.path(), "" | "/")
        {
            return invalid_edge_origin();
        }
        if protocol == "http" && !is_loopback_host(authority.host()) {
            return Err(CliError::Config(
                "Git credentials require an HTTPS Edge (or loopback HTTP for development)".into(),
            ));
        }
        let host = authority.as_str().to_ascii_lowercase();
        Ok(Self {
            protocol: protocol.to_string(),
            edge_origin: format!("{protocol}://{host}"),
            host,
        })
    }

    pub fn edge_origin(&self) -> &str {
        &self.edge_origin
    }

    fn matches(&self, request: &CredentialRequest) -> bool {
        request.protocol.as_deref() == Some(self.protocol.as_str())
            && request
                .host
                .as_deref()
                .is_some_and(|host| host.eq_ignore_ascii_case(&self.host))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct GitConfiguration {
    key: String,
    helper: String,
}

impl GitConfiguration {
    pub fn new(
        scope: &CredentialScope,
        executable: &Path,
        credential_scheme: &str,
    ) -> Result<Self, CliError> {
        git_username(credential_scheme)?;
        let executable = executable.to_str().ok_or_else(|| {
            CliError::Config("the Myelin executable path is not valid UTF-8".into())
        })?;
        if executable.chars().any(char::is_control) {
            return Err(CliError::Config(
                "the Myelin executable path contains control characters".into(),
            ));
        }
        Ok(Self {
            key: format!("credential.{}.helper", scope.edge_origin),
            helper: format!("!{} auth git-credential", shell_quote(executable)),
        })
    }
}

/** Answer Git's credential-helper protocol only for the exact Edge that issued the session. */
pub fn serve(
    operation: Operation,
    credential: &Credential,
    input: impl Read,
    mut output: impl Write,
) -> Result<(), CliError> {
    let request = read_request(input)?;
    if operation != Operation::Get {
        return Ok(());
    }
    credential.ensure_not_expired()?;
    let edge_url = credential.edge_url.as_deref().ok_or_else(|| {
        CliError::NotAuthenticated(
            "the saved credential predates Edge-aware login; run `myelin auth login` again".into(),
        )
    })?;
    let scope = CredentialScope::from_edge_url(edge_url)?;
    if !scope.matches(&request) {
        return Ok(());
    }
    let username = git_username(&credential.scheme)?;
    writeln!(output, "username={username}")
        .and_then(|()| writeln!(output, "password={}", credential.token))
        .and_then(|()| writeln!(output))
        .and_then(|()| output.flush())
        .map_err(|error| CliError::Config(format!("cannot answer Git credential request: {error}")))
}

pub fn configure(configuration: &GitConfiguration) -> Result<bool, CliError> {
    if registration_exists(configuration)? {
        return Ok(false);
    }
    git_status(&[
        "config",
        "--global",
        "--add",
        &configuration.key,
        &configuration.helper,
    ])?;
    Ok(true)
}

pub fn unconfigure(configuration: &GitConfiguration) -> Result<bool, CliError> {
    if !registration_exists(configuration)? {
        return Ok(false);
    }
    git_status(&[
        "config",
        "--global",
        "--fixed-value",
        "--unset-all",
        &configuration.key,
        &configuration.helper,
    ])?;
    Ok(true)
}

fn registration_exists(configuration: &GitConfiguration) -> Result<bool, CliError> {
    let status = Command::new("git")
        .args([
            "config",
            "--global",
            "--fixed-value",
            "--get-all",
            &configuration.key,
            &configuration.helper,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| CliError::Config(format!("cannot run Git: {error}")))?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(CliError::Config(
            "Git could not read the global credential-helper configuration".into(),
        )),
    }
}

fn git_status(arguments: &[&str]) -> Result<(), CliError> {
    let status = Command::new("git")
        .args(arguments)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| CliError::Config(format!("cannot run Git: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Config(
            "Git could not update the global credential-helper configuration".into(),
        ))
    }
}

fn read_request(input: impl Read) -> Result<CredentialRequest, CliError> {
    let mut bytes = Vec::new();
    input
        .take(MAX_CREDENTIAL_REQUEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CliError::Config(format!("cannot read Git credential request: {error}"))
        })?;
    if bytes.len() as u64 > MAX_CREDENTIAL_REQUEST_BYTES {
        return malformed_request("request exceeds the byte limit");
    }
    let encoded = std::str::from_utf8(&bytes)
        .map_err(|_| malformed_request_error("request is not valid UTF-8"))?;
    let mut request = CredentialRequest {
        protocol: None,
        host: None,
    };
    for line in encoded.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            break;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| malformed_request_error("request line has no equals sign"))?;
        if key.is_empty()
            || key.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return malformed_request("request contains invalid field bytes");
        }
        match key {
            "protocol" => set_once(&mut request.protocol, value, "protocol")?,
            "host" => set_once(&mut request.host, value, "host")?,
            _ => {}
        }
    }
    Ok(request)
}

fn set_once(field: &mut Option<String>, value: &str, name: &str) -> Result<(), CliError> {
    if value.is_empty() || field.replace(value.to_string()).is_some() {
        return malformed_request(format!("{name} is empty or repeated"));
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn git_username(scheme: &str) -> Result<String, CliError> {
    if matches!(scheme, "session" | "pat" | "agent" | "deploy_key") {
        Ok(format!("myelin-{scheme}"))
    } else {
        Err(CliError::Config(
            "the saved credential scheme cannot authenticate Git".into(),
        ))
    }
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

fn invalid_edge_origin<T>() -> Result<T, CliError> {
    Err(invalid_edge_origin_error())
}

fn invalid_edge_origin_error() -> CliError {
    CliError::Config(
        "Git credentials require the saved Edge to be an absolute HTTP(S) origin".into(),
    )
}

fn malformed_request<T>(reason: impl Into<String>) -> Result<T, CliError> {
    Err(malformed_request_error(reason))
}

fn malformed_request_error(reason: impl Into<String>) -> CliError {
    CliError::Config(format!(
        "malformed Git credential request: {}",
        reason.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn credential(edge_url: Option<&str>) -> Credential {
        Credential {
            token: "SECRET_SESSION".into(),
            scheme: "session".into(),
            edge_url: edge_url.map(str::to_string),
            expires_at_unix: None,
        }
    }

    #[test]
    fn get_answers_only_the_exact_issuing_edge() {
        let saved = credential(Some("https://edge.example"));
        let mut matching = Vec::new();
        serve(
            Operation::Get,
            &saved,
            "protocol=https\nhost=EDGE.EXAMPLE\npath=acme/eu/repo.git\n\n".as_bytes(),
            &mut matching,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(matching).unwrap(),
            "username=myelin-session\npassword=SECRET_SESSION\n\n"
        );

        for request in [
            "protocol=https\nhost=attacker.example\n\n",
            "protocol=http\nhost=edge.example\n\n",
            "protocol=https\nhost=edge.example:443\n\n",
        ] {
            let mut output = Vec::new();
            serve(Operation::Get, &saved, request.as_bytes(), &mut output).unwrap();
            assert!(output.is_empty());
        }
    }

    #[test]
    fn store_and_erase_never_persist_or_reveal_a_secret() {
        for operation in [Operation::Store, Operation::Erase] {
            let mut output = Vec::new();
            serve(
                operation,
                &credential(None),
                "protocol=https\nhost=edge.example\npassword=FROM_GIT\n\n".as_bytes(),
                &mut output,
            )
            .unwrap();
            assert!(output.is_empty());
        }
    }

    #[test]
    fn an_expired_session_is_never_disclosed_to_git() {
        let mut expired = credential(Some("https://edge.example"));
        expired.expires_at_unix = Some(1);
        let mut output = Vec::new();

        let error = serve(
            Operation::Get,
            &expired,
            "protocol=https\nhost=edge.example\n\n".as_bytes(),
            &mut output,
        )
        .unwrap_err();

        assert_eq!(error.code(), 3);
        assert!(error.to_string().contains("saved CLI session has expired"));
        assert!(output.is_empty());
    }

    #[test]
    fn requests_are_bounded_and_security_fields_are_unambiguous() {
        let saved = credential(Some("https://edge.example"));
        for request in [
            "protocol=https\nprotocol=https\nhost=edge.example\n\n".to_string(),
            "protocol=https\nhost=edge.example\nhost=edge.example\n\n".to_string(),
            format!(
                "unknown={}\n\n",
                "x".repeat(MAX_CREDENTIAL_REQUEST_BYTES as usize)
            ),
        ] {
            let error = serve(Operation::Get, &saved, request.as_bytes(), Vec::new()).unwrap_err();
            assert!(error
                .to_string()
                .contains("malformed Git credential request"));
            assert!(!error.to_string().contains("SECRET_SESSION"));
        }
    }

    #[test]
    fn scopes_require_https_or_loopback_http_origins() {
        for accepted in [
            "https://edge.example",
            "http://localhost:8080/",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] {
            CredentialScope::from_edge_url(accepted).unwrap();
        }
        for refused in [
            "http://edge.example",
            "https://user@edge.example",
            "https://edge.example/base",
            "https://edge.example?query=yes",
            "https://edge.example/#fragment",
        ] {
            assert!(
                CredentialScope::from_edge_url(refused).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn global_configuration_is_scoped_and_shell_quotes_the_executable() {
        let scope = CredentialScope::from_edge_url("https://edge.example").unwrap();
        let config = GitConfiguration::new(
            &scope,
            &PathBuf::from("/opt/Myelin's tools/myelin"),
            "session",
        )
        .unwrap();
        assert_eq!(config.key, "credential.https://edge.example.helper");
        assert_eq!(
            config.helper,
            "!'/opt/Myelin'\"'\"'s tools/myelin' auth git-credential"
        );
    }
}
