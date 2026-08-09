use crate::credential_store::{
    new_reference, validate_reference, write_owner_only_atomic, CredentialSecretStore,
};
use crate::error::CliError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_EDGE: &str = "http://127.0.0.1:8080";
pub const DEFAULT_SCHEME: &str = "agent";
pub const SESSION_SCHEME: &str = "session";

const LEGACY_CREDENTIAL_VERSION: u8 = 1;
const CREDENTIAL_VERSION: u8 = 2;
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

#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub token: String,
    pub scheme: String,
    /** Edge that issued this credential. Older and externally supplied credentials may not know it. */
    pub edge_url: Option<String>,
    /** Absolute Unix expiry. Long-lived and externally supplied credentials may not declare one. */
    pub expires_at_unix: Option<i64>,
}

impl core::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Credential")
            .field("token", &"<redacted>")
            .field("scheme", &self.scheme)
            .field("edge_url", &self.edge_url)
            .field("expires_at_unix", &self.expires_at_unix)
            .finish()
    }
}

impl Credential {
    pub fn ensure_not_expired(&self) -> Result<(), CliError> {
        if self
            .expires_at_unix
            .is_some_and(|expires_at| expires_at <= unix_now())
        {
            return Err(CliError::NotAuthenticated(
                "the saved CLI session has expired".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyStoredCredential {
    #[serde(rename = "version")]
    _version: u8,
    token: String,
    scheme: String,
    #[serde(default)]
    edge_url: Option<String>,
    #[serde(default)]
    expires_at_unix: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCredentialMetadata {
    version: u8,
    credential_ref: String,
    scheme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edge_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at_unix: Option<i64>,
}

#[derive(Debug)]
enum StoredCredential {
    Legacy(LegacyStoredCredential),
    Referenced(StoredCredentialMetadata),
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
            None,
        );
    }

    if let Some(mut stored) = load_stored_credential(getenv, read_file)? {
        if let Some(scheme) = supplied_scheme {
            stored.scheme = credential(
                &stored.token,
                &scheme,
                stored.edge_url.as_deref(),
                stored.expires_at_unix,
            )?
            .scheme;
        }
        return Ok(stored);
    }

    let legacy_path = legacy_token_path(getenv)?;
    if let Some(token) = read_file(&legacy_path) {
        return credential(
            &token,
            supplied_scheme.as_deref().unwrap_or(DEFAULT_SCHEME),
            None,
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
    match stored {
        StoredCredential::Legacy(stored) => credential(
            &stored.token,
            &stored.scheme,
            stored.edge_url.as_deref(),
            stored.expires_at_unix,
        ),
        StoredCredential::Referenced(stored) => {
            let directory = config_dir(getenv)?;
            let token =
                CredentialSecretStore::selected(&directory, getenv)?.get(&stored.credential_ref)?;
            credential(
                &token,
                &stored.scheme,
                stored.edge_url.as_deref(),
                stored.expires_at_unix,
            )
        }
    }
    .map(Some)
}

pub fn store_credential(
    token: &str,
    scheme: &str,
    edge_url: Option<&str>,
    expires_at_unix: Option<i64>,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<PathBuf, CliError> {
    let credential = credential(token, scheme, edge_url, expires_at_unix)?;
    let dir = config_dir(getenv)?;
    std::fs::create_dir_all(&dir).map_err(|error| {
        CliError::Config(format!(
            "cannot create config dir {}: {error}",
            dir.display()
        ))
    })?;
    let path = dir.join("credentials.json");
    let previous_reference = std::fs::read_to_string(&path)
        .ok()
        .and_then(|encoded| parse_stored_credential(&encoded, &path).ok())
        .and_then(|stored| match stored {
            StoredCredential::Referenced(stored) => Some(stored.credential_ref),
            StoredCredential::Legacy(_) => None,
        });
    let credential_ref = new_reference();
    let stored = StoredCredentialMetadata {
        version: CREDENTIAL_VERSION,
        credential_ref: credential_ref.clone(),
        scheme: credential.scheme,
        edge_url: credential.edge_url,
        expires_at_unix: credential.expires_at_unix,
    };
    let encoded = serde_json::to_vec(&stored)
        .map_err(|error| CliError::Config(format!("cannot encode credential metadata: {error}")))?;
    let secret_store = CredentialSecretStore::selected(&dir, getenv)?;
    secret_store.put(&credential_ref, &credential.token)?;
    if let Err(write_error) = write_owner_only_atomic(&path, &encoded) {
        return match secret_store.delete(&credential_ref) {
            Ok(_) => Err(write_error),
            Err(_) => Err(CliError::Config(
                "credential metadata installation failed and the new OS credential could not be cleaned up"
                    .into(),
            )),
        };
    }
    if let Some(previous_reference) = previous_reference.filter(|old| old != &credential_ref) {
        // The new credential is already installed and usable. Treat failure to remove the old,
        // auth-bounded secret as cleanup debt rather than reporting a false login failure.
        let _ = secret_store.delete(&previous_reference);
    }
    Ok(path)
}

/** Remove the referenced OS credential and every on-disk compatibility format. Environment and
 * command-line credentials are untouched. */
pub fn remove_stored_credentials(
    getenv: &dyn Fn(&str) -> Option<String>,
) -> Result<bool, CliError> {
    let mut removed = false;
    let metadata_path = credential_path(getenv)?;
    if std::fs::metadata(&metadata_path)
        .is_ok_and(|metadata| metadata.len() > MAX_CREDENTIAL_FILE_BYTES as u64)
    {
        return Err(CliError::Config(format!(
            "stored credential {} exceeds the byte limit",
            metadata_path.display()
        )));
    }
    let encoded = match std::fs::read_to_string(&metadata_path) {
        Ok(encoded) => Some(encoded),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(CliError::Config(format!(
                "cannot read stored credential metadata {}: {error}",
                metadata_path.display()
            )))
        }
    };
    if let Some(encoded) = encoded {
        if let StoredCredential::Referenced(stored) =
            parse_stored_credential(&encoded, &metadata_path)?
        {
            let directory = config_dir(getenv)?;
            removed |= CredentialSecretStore::selected(&directory, getenv)?
                .delete(&stored.credential_ref)?;
        }
    }
    for path in [metadata_path, legacy_token_path(getenv)?] {
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

fn credential(
    token: &str,
    scheme: &str,
    edge_url: Option<&str>,
    expires_at_unix: Option<i64>,
) -> Result<Credential, CliError> {
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
    validate_metadata(scheme, edge_url, expires_at_unix)?;
    Ok(Credential {
        token: token.to_string(),
        scheme: scheme.to_string(),
        edge_url: edge_url.map(normalize_stored_edge_url).transpose()?,
        expires_at_unix,
    })
}

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
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
    #[derive(Deserialize)]
    struct Version {
        version: u8,
    }

    let version: Version = serde_json::from_str(encoded).map_err(|_| {
        CliError::Config(format!(
            "stored credential {} is malformed; run `myelin auth login` again",
            path.display()
        ))
    })?;
    match version.version {
        LEGACY_CREDENTIAL_VERSION => {
            let stored: LegacyStoredCredential = malformed_json(encoded, path)?;
            credential(
                &stored.token,
                &stored.scheme,
                stored.edge_url.as_deref(),
                stored.expires_at_unix,
            )?;
            Ok(StoredCredential::Legacy(stored))
        }
        CREDENTIAL_VERSION => {
            let stored: StoredCredentialMetadata = malformed_json(encoded, path)?;
            validate_reference(&stored.credential_ref)?;
            validate_metadata(
                &stored.scheme,
                stored.edge_url.as_deref(),
                stored.expires_at_unix,
            )?;
            Ok(StoredCredential::Referenced(stored))
        }
        unsupported => Err(CliError::Config(format!(
            "stored credential {} has unsupported version {unsupported}",
            path.display()
        ))),
    }
}

fn malformed_json<T: for<'de> Deserialize<'de>>(encoded: &str, path: &Path) -> Result<T, CliError> {
    serde_json::from_str(encoded).map_err(|_| {
        CliError::Config(format!(
            "stored credential {} is malformed; run `myelin auth login` again",
            path.display()
        ))
    })
}

fn validate_metadata(
    scheme: &str,
    edge_url: Option<&str>,
    expires_at_unix: Option<i64>,
) -> Result<(), CliError> {
    if !valid_scheme(scheme) {
        return Err(CliError::Config(
            "credential scheme must match [a-z][a-z0-9_]{0,31}".into(),
        ));
    }
    if let Some(edge_url) = edge_url {
        normalize_stored_edge_url(edge_url)?;
    }
    if expires_at_unix.is_some_and(|expires_at| expires_at <= 0) {
        return Err(CliError::Config(
            "credential expiry must be a positive Unix timestamp".into(),
        ));
    }
    Ok(())
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
                expires_at_unix: None,
            }
        );
        assert_eq!(
            resolve_credential(None, None, &env, &no_file).unwrap(),
            Credential {
                token: "ENV_TOKEN".into(),
                scheme: "ci".into(),
                edge_url: None,
                expires_at_unix: None,
            }
        );
        let rendered = format!(
            "{:?}",
            resolve_credential(Some("DO_NOT_PRINT_ME"), None, &env, &no_file).unwrap()
        );
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("DO_NOT_PRINT_ME"));
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
                expires_at_unix: None,
            }
        );
    }

    #[test]
    fn a_saved_session_remembers_its_issuing_edge() {
        let env = env_from(&[("MYELIN_CONFIG_DIR", "/config")]);
        let read = |path: &Path| {
            (path == Path::new("/config/credentials.json")).then(|| {
                r#"{"version":1,"token":"HUMAN_SESSION","scheme":"session","edge_url":"https://edge.example/","expires_at_unix":4102444800}"#.into()
            })
        };
        assert_eq!(
            resolve_credential(None, None, &env, &read).unwrap(),
            Credential {
                token: "HUMAN_SESSION".into(),
                scheme: SESSION_SCHEME.into(),
                edge_url: Some("https://edge.example".into()),
                expires_at_unix: Some(4_102_444_800),
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
                expires_at_unix: None,
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
    fn an_elapsed_session_is_loaded_but_never_considered_usable() {
        let env = env_from(&[("MYELIN_CONFIG_DIR", "/config")]);
        let read = |path: &Path| {
            (path == Path::new("/config/credentials.json")).then(|| {
                r#"{"version":1,"token":"ELAPSED_SESSION","scheme":"session","edge_url":"https://edge.example","expires_at_unix":1}"#.into()
            })
        };

        let credential = resolve_credential(None, None, &env, &read).unwrap();
        let error = credential.ensure_not_expired().unwrap_err();
        assert_eq!(error.code(), 3);
        assert!(error.to_string().contains("saved CLI session has expired"));
        assert!(!error.to_string().contains("ELAPSED_SESSION"));
    }

    #[test]
    fn store_rotation_resolution_and_logout_keep_secrets_out_of_metadata() {
        let tmp = std::env::temp_dir().join(format!(
            "myelin-cli-credential-{}-{}",
            std::process::id(),
            new_reference()
        ));
        let dir = tmp.to_string_lossy().to_string();
        let env = env_from(&[
            ("MYELIN_CONFIG_DIR", &dir),
            (crate::credential_store::TEST_STORE_ENV, "file"),
        ]);
        let path = store_credential(
            "ROUNDTRIP_TOKEN",
            SESSION_SCHEME,
            Some("https://edge.example/"),
            Some(4_102_444_800),
            &env,
        )
        .unwrap();
        let encoded = std::fs::read_to_string(&path).unwrap();
        assert!(!encoded.contains("ROUNDTRIP_TOKEN"));
        let metadata: StoredCredentialMetadata = serde_json::from_str(&encoded).unwrap();
        assert_eq!(metadata.version, CREDENTIAL_VERSION);
        let store = CredentialSecretStore::selected(&tmp, &env).unwrap();
        let secret_path = store
            .test_secret_path(&metadata.credential_ref)
            .expect("the debug test store has an inspectable path");
        assert_eq!(
            std::fs::read_to_string(&secret_path).unwrap(),
            "ROUNDTRIP_TOKEN"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the credential metadata is owner-only");
            let secret_mode = std::fs::metadata(&secret_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                secret_mode, 0o600,
                "the debug-only test secret is owner-only"
            );
        }

        let rotated_path = store_credential(
            "ROTATED_TOKEN",
            SESSION_SCHEME,
            Some("https://edge.example/"),
            Some(4_102_444_800),
            &env,
        )
        .unwrap();
        assert_eq!(rotated_path, path);
        let rotated_metadata: StoredCredentialMetadata =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_ne!(rotated_metadata.credential_ref, metadata.credential_ref);
        assert!(
            !secret_path.exists(),
            "replacing a credential removes its old secret"
        );
        let rotated_secret_path = store
            .test_secret_path(&rotated_metadata.credential_ref)
            .expect("the debug test store has an inspectable path");
        assert_eq!(
            std::fs::read_to_string(&rotated_secret_path).unwrap(),
            "ROTATED_TOKEN"
        );

        let read = |path: &Path| std::fs::read_to_string(path).ok();
        assert_eq!(
            resolve_credential(None, None, &env, &read).unwrap(),
            Credential {
                token: "ROTATED_TOKEN".into(),
                scheme: SESSION_SCHEME.into(),
                edge_url: Some("https://edge.example".into()),
                expires_at_unix: Some(4_102_444_800),
            }
        );
        std::fs::write(tmp.join("token"), "OLD_TOKEN").unwrap();
        assert!(remove_stored_credentials(&env).unwrap());
        assert!(!path.exists());
        assert!(!rotated_secret_path.exists());
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
