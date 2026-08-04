use crate::error::CliError;
use std::path::PathBuf;

pub const DEFAULT_EDGE: &str = "http://127.0.0.1:8080";
pub const DEFAULT_SCHEME: &str = "agent";

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

pub fn config_dir(getenv: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    if let Some(dir) = getenv(env::CONFIG_DIR) {
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = getenv("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(xdg).join("myelin"));
    }
    let home = getenv("HOME")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| CliError::Config("no $HOME, $XDG_CONFIG_HOME, or $MYELIN_CONFIG_DIR set".into()))?;
    Ok(PathBuf::from(home).join(".config").join("myelin"))
}

pub fn token_path(getenv: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    Ok(config_dir(getenv)?.join("token"))
}

pub fn resolve_edge(
    flag_url: Option<&str>,
    flag_scheme: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
) -> EdgeConfig {
    let url = flag_url
        .map(str::to_string)
        .or_else(|| getenv(env::EDGE).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| DEFAULT_EDGE.to_string());
    let scheme = flag_scheme
        .map(str::to_string)
        .or_else(|| getenv(env::SCHEME).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| DEFAULT_SCHEME.to_string());
    EdgeConfig { url, scheme }
}

pub fn resolve_token(
    flag_token: Option<&str>,
    getenv: &dyn Fn(&str) -> Option<String>,
    read_file: &dyn Fn(&std::path::Path) -> Option<String>,
) -> Result<String, CliError> {
    if let Some(t) = flag_token.filter(|s| !s.is_empty()) {
        return Ok(t.to_string());
    }
    if let Some(t) = getenv(env::TOKEN).filter(|s| !s.is_empty()) {
        return Ok(t);
    }
    let path = token_path(getenv)?;
    if let Some(t) = read_file(&path).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        return Ok(t);
    }
    Err(CliError::NotAuthenticated(
        "no token in --token, $MYELIN_TOKEN, or the stored config".into(),
    ))
}

pub fn store_token(token: &str, getenv: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    let dir = config_dir(getenv)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| CliError::Config(format!("cannot create config dir {}: {e}", dir.display())))?;
    let path = dir.join("token");
    write_owner_only(&path, token.as_bytes())?;
    set_owner_only(&path)?;
    Ok(path)
}

#[cfg(unix)]
fn write_owner_only(path: &std::path::Path, bytes: &[u8]) -> Result<(), CliError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| CliError::Config(format!("cannot create token file {}: {e}", path.display())))?;
    f.write_all(bytes)
        .map_err(|e| CliError::Config(format!("cannot write token file {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn write_owner_only(path: &std::path::Path, bytes: &[u8]) -> Result<(), CliError> {
    std::fs::write(path, bytes)
        .map_err(|e| CliError::Config(format!("cannot write token file {}: {e}", path.display())))
}

#[cfg(unix)]
fn set_owner_only(path: &std::path::Path) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| CliError::Config(format!("cannot set 0600 on {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path) -> Result<(), CliError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn config_dir_precedence_is_explicit_then_xdg_then_home() {
        let explicit = env_from(&[("MYELIN_CONFIG_DIR", "/x"), ("HOME", "/h")]);
        assert_eq!(config_dir(&explicit).unwrap(), PathBuf::from("/x"));
        let xdg = env_from(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/h")]);
        assert_eq!(config_dir(&xdg).unwrap(), PathBuf::from("/xdg/myelin"));
        let home = env_from(&[("HOME", "/h")]);
        assert_eq!(config_dir(&home).unwrap(), PathBuf::from("/h/.config/myelin"));
        let none = env_from(&[]);
        assert!(config_dir(&none).is_err(), "no anchor → clean Config error, not a panic");
    }

    #[test]
    fn resolve_edge_precedence_flag_then_env_then_default() {
        let e = env_from(&[("MYELIN_EDGE", "http://envhost:9"), ("MYELIN_TOKEN_SCHEME", "ci")]);
        let c = resolve_edge(Some("http://flag:1"), Some("agent"), &e);
        assert_eq!(c.url, "http://flag:1");
        assert_eq!(c.scheme, "agent");
        let c = resolve_edge(None, None, &e);
        assert_eq!(c.url, "http://envhost:9");
        assert_eq!(c.scheme, "ci");
        let c = resolve_edge(None, None, &env_from(&[]));
        assert_eq!(c.url, DEFAULT_EDGE);
        assert_eq!(c.scheme, DEFAULT_SCHEME);
    }

    #[test]
    fn resolve_token_precedence_and_clean_error_when_absent() {
        let no_file = |_: &std::path::Path| None;
        assert_eq!(
            resolve_token(Some("TOK_FLAG"), &env_from(&[]), &no_file).unwrap(),
            "TOK_FLAG"
        );
        assert_eq!(
            resolve_token(None, &env_from(&[("MYELIN_TOKEN", "TOK_ENV")]), &no_file).unwrap(),
            "TOK_ENV"
        );
        let with_file = |_: &std::path::Path| Some("TOK_FILE\n".to_string());
        assert_eq!(
            resolve_token(None, &env_from(&[("MYELIN_CONFIG_DIR", "/c")]), &with_file).unwrap(),
            "TOK_FILE"
        );
        let err = resolve_token(None, &env_from(&[("MYELIN_CONFIG_DIR", "/c")]), &no_file).unwrap_err();
        assert_eq!(err.code(), 3);
    }

    #[test]
    fn store_then_resolve_roundtrips_and_is_owner_only() {
        let tmp = std::env::temp_dir().join(format!("myelin-cli-test-{}", std::process::id()));
        let dir = tmp.to_string_lossy().to_string();
        let getenv = env_from(&[("MYELIN_CONFIG_DIR", &dir)]);
        let path = store_token("ROUNDTRIP_TOKEN", &getenv).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the token file is owner-only");
        }
        let read = |p: &std::path::Path| std::fs::read_to_string(p).ok();
        assert_eq!(resolve_token(None, &getenv, &read).unwrap(), "ROUNDTRIP_TOKEN");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = path;
    }

    #[cfg(unix)]
    #[test]
    fn store_token_tightens_a_preexisting_world_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("myelin-cli-test-preexist-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("token");
        std::fs::write(&path, b"OLD").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let dir = tmp.to_string_lossy().to_string();
        let getenv = env_from(&[("MYELIN_CONFIG_DIR", &dir)]);
        let path = store_token("NEW_TOKEN", &getenv).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a pre-existing 0644 token file is re-tightened to 0600");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "NEW_TOKEN");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
