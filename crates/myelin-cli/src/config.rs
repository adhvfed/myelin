use crate::error::CliError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_EDGE: &str = "http://127.0.0.1:8080";
pub const DEFAULT_SCHEME: &str = "agent";
pub const SESSION_SCHEME: &str = "session";

const CREDENTIAL_VERSION: u8 = 1;
const MAX_TOKEN_BYTES: usize = 32 * 1024;
const MAX_CREDENTIAL_FILE_BYTES: usize = 64 * 1024;

pub mod env {
    pub const TOKEN: &str = "MYELIN_TOKEN";
    pub const SCHEME: &str = "MYELIN_TOKEN_SCHEME";
    pub const EDGE: &str = "MYELIN_EDGE";
    pub const CONFIG_DIR: &str = "MYELIN_CONFIG_DIR";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeConfig {
    pub url: String,
    pub scheme: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    pub token: String,
    pub scheme: String,
    /** Edge that issued this credential. Older and externally supplied credentials may not know it. */
    pub edge_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCredential {
    version: u8,
    token: String,
    scheme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edge_url: Option<String>,
}

pub fn config_dir(getenv: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    if let Some(dir) = getenv(env::CONFIG_DIR).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = getenv("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(xdg).join("myelin"));
    }
    let home = getenv("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CliError::Config("no $HOME, $XDG_CONFIG_HOME, or $MYELIN_CONFIG_DIR set".into())
        })?;
    Ok(PathBuf::from(home).join(".config").join("myelin"))
}

pub fn credential_path(getenv: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    Ok(config_dir(getenv)?.join("credentials.json"))
}

/** The pre-device-login token file. Read-only compatibility keeps existing installations working. */
pub fn legacy_token_path(getenv: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    Ok(config_dir(getenv)?.join("token"))
}

pub fn resolve_edge(
    flag_url: Option<&str>,
    scheme: Option<&str>,
    stored_url: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> EdgeConfig {
    let url = flag_url
        .map(str::to_string)
        .or_else(|| getenv(env::EDGE).filter(|value| !value.is_empty()))
        .or_else(|| stored_url.map(str::to_string))
        .unwrap_or_else(|| DEFAULT_EDGE.to_string());
    EdgeConfig {
        url,
        scheme: scheme.unwrap_or(DEFAULT_SCHEME).to_string(),
    }
}

pub fn resolve_credential(
    flag_token: Option<&str>,
    flag_scheme: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&Path) -> Option<String>,
) -> Result<Credential, CliError> {
    let supplied_scheme = flag_scheme
        .map(str::to_string)
        .or_else(|| getenv(env::SCHEME).filter(|value| !value.is_empty()));

    if let Some(token) = flag_token
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| getenv(env::TOKEN).filter(|value| !value.is_empty()))
    {
        return credential(
            &token,
            supplied_scheme.as_deref().unwrap_or(DEFAULT_SCHEME),
            None,
        );
    }

    if let Some(mut stored) = load_stored_credential(getenv, read_file)? {
        if let Some(scheme) = supplied_scheme {
            stored.scheme = credential(&stored.token, &scheme, stored.edge_url.as_deref())?.scheme;
        }
        return Ok(stored);
    }

    let legacy_path = legacy_token_path(getenv)?;
    if let Some(token) = read_file(&legacy_path) {
        return credential(
            &token,
            supplied_scheme.as_deref().unwrap_or(DEFAULT_SCHEME),
            None,
        );
    }

    Err(CliError::NotAuthenticated(
        "no token in --token, $MYELIN_TOKEN, or the stored config".into(),
    ))
}

/** Load only the modern credential file, without allowing flags, environment tokens, or legacy
 * token files to silently change the credential/Edge pair. */
pub fn load_stored_credential(
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&Path) -> Option<String>,
) -> Result<Option<Credential>, CliError> {
    let path = credential_path(getenv)?;
    let Some(encoded) = read_file(&path) else {
        return Ok(None);
    };
    let stored = parse_stored_credential(&encoded, &path)?;
    credential(&stored.token, &stored.scheme, stored.edge_url.as_deref()).map(Some)
}

pub fn store_credential(
    token: &str,
    scheme: &str,
    edge_url: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<PathBuf, CliError> {
    let credential = credential(token, scheme, edge_url)?;
    let stored = StoredCredential {
        version: CREDENTIAL_VERSION,
        token: credential.token,
        scheme: credential.scheme,
        edge_url: credential.edge_url,
    };
    let encoded = serde_json::to_vec(&stored)
        .map_err(|error| CliError::Config(format!("cannot encode credentials: {error}")))?;
    let dir = config_dir(getenv)?;
    std::fs::create_dir_all(&dir).map_err(|error| {
        CliError::Config(format!(
            "cannot create config dir {}: {error}",
            dir.display()
        ))
    })?;
    let path = dir.join("credentials.json");
    write_owner_only_atomic(&path, &encoded)?;
    Ok(path)
}

/** Remove every on-disk credential format. Environment and command-line credentials are untouched. */
pub fn remove_stored_credentials(
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<bool, CliError> {
    let mut removed = false;
    for path in [credential_path(getenv)?, legacy_token_path(getenv)?] {
        match std::fs::remove_file(&path) {
            Ok(()) => removed = true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CliError::Config(format!(
                    "cannot remove stored credential {}: {error}",
                    path.display()
                )))
            }
        }
    }
    Ok(removed)
}

fn credential(token: &str, scheme: &str, edge_url: Option<&str>) -> Result<Credential, CliError> {
    let token = token.trim();
    let scheme = scheme.trim();
    if token.is_empty()
        || token.len() > MAX_TOKEN_BYTES
        || !token.as_bytes().iter().all(u8::is_ascii_graphic)
    {
        return Err(CliError::Config(
            "credential token must be bounded printable ASCII without spaces".into(),
        ));
    }
    if !valid_scheme(scheme) {
        return Err(CliError::Config(
            "credential scheme must match [a-z][a-z0-9_]{0,31}".into(),
        ));
    }
    Ok(Credential {
        token: token.to_string(),
        scheme: scheme.to_string(),
        edge_url: edge_url.map(normalize_stored_edge_url).transpose()?,
    })
}

fn normalize_stored_edge_url(edge_url: &str) -> Result<String, CliError> {
    let edge_url = edge_url.trim().trim_end_matches('/');
    if edge_url.is_empty()
        || edge_url.len() > 2_048
        || !edge_url.as_bytes().iter().all(u8::is_ascii_graphic)
    {
        return Err(CliError::Config(
            "credential edge URL must be bounded printable ASCII without spaces".into(),
        ));
    }
    Ok(edge_url.to_string())
}

fn valid_scheme(scheme: &str) -> bool {
    let mut bytes = scheme.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && scheme.len() <= 32
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn parse_stored_credential(encoded: &str, path: &Path) -> Result<StoredCredential, CliError> {
    if encoded.len() > MAX_CREDENTIAL_FILE_BYTES {
        return Err(CliError::Config(format!(
            "stored credential {} exceeds the byte limit",
            path.display()
        )));
    }
    let stored: StoredCredential = serde_json::from_str(encoded).map_err(|_| {
        CliError::Config(format!(
            "stored credential {} is malformed; run `myelin auth login` again",
            path.display()
        ))
    })?;
    if stored.version != CREDENTIAL_VERSION {
        return Err(CliError::Config(format!(
            "stored credential {} has unsupported version {}",
            path.display(),
            stored.version
        )));
    }
    credential(&stored.token, &stored.scheme, stored.edge_url.as_deref())?;
    Ok(stored)
}

#[cfg(unix)]
fn write_owner_only_atomic(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| CliError::Config("credential path has no parent directory".into()))?;
    let mut temporary = None;
    for attempt in 0..100 {
        let candidate = parent.join(format!(
            ".credentials.json.tmp-{}-{attempt}",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(CliError::Config(format!(
                    "cannot create credential file in {}: {error}",
                    parent.display()
                )))
            }
        }
    }
    let (temporary_path, mut file) = temporary.ok_or_else(|| {
        CliError::Config(format!(
            "cannot allocate a temporary credential file in {}",
            parent.display()
        ))
    })?;
    let result = (|| {
        file.write_all(bytes).map_err(|error| {
            CliError::Config(format!(
                "cannot write credential file {}: {error}",
                temporary_path.display()
            ))
        })?;
        file.sync_all().map_err(|error| {
            CliError::Config(format!(
                "cannot sync credential file {}: {error}",
                temporary_path.display()
            ))
        })?;
        drop(file);
        std::fs::rename(&temporary_path, path).map_err(|error| {
            CliError::Config(format!(
                "cannot install credential file {}: {error}",
                path.display()
            ))
        })?;
        set_owner_only(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(unix))]
fn write_owner_only_atomic(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    std::fs::write(path, bytes).map_err(|error| {
        CliError::Config(format!(
            "cannot write credential file {}: {error}",
            path.display()
        ))
    })
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        CliError::Config(format!("cannot set 0600 on {}: {error}", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    fn no_file(_: &Path) -> Option<String> {
        None
    }

    #[test]
    fn config_dir_precedence_is_explicit_then_xdg_then_home() {
        let explicit = env_from(&[("MYELIN_CONFIG_DIR", "/x"), ("HOME", "/h")]);
        assert_eq!(config_dir(&explicit).unwrap(), PathBuf::from("/x"));
        let xdg = env_from(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/h")]);
        assert_eq!(config_dir(&xdg).unwrap(), PathBuf::from("/xdg/myelin"));
        let home = env_from(&[("HOME", "/h")]);
        assert_eq!(
            config_dir(&home).unwrap(),
            PathBuf::from("/h/.config/myelin")
        );
        assert!(config_dir(&env_from(&[])).is_err());
    }

    #[test]
    fn explicit_and_environment_credentials_precede_disk() {
        let env = env_from(&[("MYELIN_TOKEN", "ENV_TOKEN"), ("MYELIN_TOKEN_SCHEME", "ci")]);
        assert_eq!(
            resolve_credential(Some("FLAG_TOKEN"), Some("pat"), &env, &no_file).unwrap(),
            Credential {
                token: "FLAG_TOKEN".into(),
                scheme: "pat".into(),
                edge_url: None,
            }
        );
        assert_eq!(
            resolve_credential(None, None, &env, &no_file).unwrap(),
            Credential {
                token: "ENV_TOKEN".into(),
                scheme: "ci".into(),
                edge_url: None,
            }
        );
    }

    #[test]
    fn a_saved_human_session_keeps_its_scheme() {
        let env = env_from(&[("MYELIN_CONFIG_DIR", "/config")]);
        let read = |path: &Path| {
            (path == Path::new("/config/credentials.json"))
                .then(|| r#"{"version":1,"token":"HUMAN_SESSION","scheme":"session"}"#.into())
        };
        assert_eq!(
            resolve_credential(None, None, &env, &read).unwrap(),
            Credential {
                token: "HUMAN_SESSION".into(),
                scheme: SESSION_SCHEME.into(),
                edge_url: None,
            }
        );
    }

    #[test]
    fn a_saved_session_remembers_its_issuing_edge() {
        let env = env_from(&[("MYELIN_CONFIG_DIR", "/config")]);
        let read = |path: &Path| {
            (path == Path::new("/config/credentials.json")).then(|| {
                r#"{"version":1,"token":"HUMAN_SESSION","scheme":"session","edge_url":"https://edge.example/"}"#.into()
            })
        };
        assert_eq!(
            resolve_credential(None, None, &env, &read).unwrap(),
            Credential {
                token: "HUMAN_SESSION".into(),
                scheme: SESSION_SCHEME.into(),
                edge_url: Some("https://edge.example".into()),
            }
        );
    }

    #[test]
    fn a_legacy_token_remains_usable_as_an_agent_credential() {
        let env = env_from(&[("MYELIN_CONFIG_DIR", "/config")]);
        let read =
            |path: &Path| (path == Path::new("/config/token")).then(|| "LEGACY_TOKEN\n".into());
        assert_eq!(
            resolve_credential(None, None, &env, &read).unwrap(),
            Credential {
                token: "LEGACY_TOKEN".into(),
                scheme: DEFAULT_SCHEME.into(),
                edge_url: None,
            }
        );
    }

    #[test]
    fn malformed_stored_credentials_fail_honestly() {
        let env = env_from(&[("MYELIN_CONFIG_DIR", "/config")]);
        let read = |path: &Path| {
            (path == Path::new("/config/credentials.json")).then(|| "not-json".into())
        };
        let error = resolve_credential(None, None, &env, &read).unwrap_err();
        assert!(error.to_string().contains("malformed"));
    }

    #[test]
    fn store_resolve_and_logout_are_owner_only_and_complete() {
        let tmp =
            std::env::temp_dir().join(format!("myelin-cli-credential-{}", std::process::id()));
        let dir = tmp.to_string_lossy().to_string();
        let env = env_from(&[("MYELIN_CONFIG_DIR", &dir)]);
        let path = store_credential(
            "ROUNDTRIP_TOKEN",
            SESSION_SCHEME,
            Some("https://edge.example/"),
            &env,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the credential file is owner-only");
        }
        let read = |path: &Path| std::fs::read_to_string(path).ok();
        assert_eq!(
            resolve_credential(None, None, &env, &read).unwrap(),
            Credential {
                token: "ROUNDTRIP_TOKEN".into(),
                scheme: SESSION_SCHEME.into(),
                edge_url: Some("https://edge.example".into()),
            }
        );
        std::fs::write(tmp.join("token"), "OLD_TOKEN").unwrap();
        assert!(remove_stored_credentials(&env).unwrap());
        assert!(!path.exists());
        assert!(!tmp.join("token").exists());
        assert!(!remove_stored_credentials(&env).unwrap());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_edge_precedence_is_flag_then_environment_then_saved_then_default() {
        let env = env_from(&[("MYELIN_EDGE", "http://envhost:9")]);
        let config = resolve_edge(
            Some("http://flag:1"),
            Some(SESSION_SCHEME),
            Some("https://saved.example"),
            &env,
        );
        assert_eq!(config.url, "http://flag:1");
        assert_eq!(config.scheme, SESSION_SCHEME);

        let config = resolve_edge(None, None, Some("https://saved.example"), &env);
        assert_eq!(config.url, "http://envhost:9");

        let config = resolve_edge(None, None, Some("https://saved.example"), &env_from(&[]));
        assert_eq!(config.url, "https://saved.example");

        let config = resolve_edge(None, None, None, &env_from(&[]));
        assert_eq!(config.url, DEFAULT_EDGE);
    }
}
