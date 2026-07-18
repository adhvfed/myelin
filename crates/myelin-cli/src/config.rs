//! # CLI configuration — the edge URL, the token scheme, and token acquisition/storage.
//!
//! ## Auth design (real vs the named seam)
//! The CLI authenticates to the edge with a **capability token** presented as `Authorization: Bearer
//! <token>` (the REAL PASETO from MR-011). What is REAL here, end-to-end:
//! - the Bearer PRESENTATION (the header + the `x-myelin-token-scheme` the gateway reads),
//! - the token's resolution from `--token` / `$MYELIN_TOKEN` / the stored config file,
//! - the round-trip to the edge, which verifies the token's Ed25519 signature against the cell.
//!
//! The **named seam (deferred):** the full human-login MINT — `POST /v1/auth/login` exchanging an
//! IdP assertion for a freshly-minted token — is MR-012-deferred (the edge `login` route *refuses,
//! not mocks*, returning 503). So `myelin login` here STORES a capability token the operator already
//! holds (minted by the cell / a future `myelin auth` IdP flow); the obtain-from-IdP step is the seam.
//! This is honest: the Bearer presentation + the edge verification are real; the token MINT is not.
//!
//! ## The token uses an UNBOUND machine/capability scheme
//! Per the MR-014 follow-up, DPoP-bound PATs are not threaded through the edge yet, so the CLI
//! presents an UNBOUND short-lived machine/capability token. The default scheme is `agent` (a
//! TTL-constrained machine kind — its life IS the constraint); the edge's default `pat` scheme would
//! be refused for an unbound token (a long-lived PAT MUST be DPoP sender-constrained, §4). Override
//! with `--scheme` / `$MYELIN_TOKEN_SCHEME`.
//!
//! ## The token is NEVER logged
//! The token is read into memory and presented as a Bearer header; it is never printed, never placed
//! in an error message, and the stored file is created `0600` (owner-only).

use crate::error::CliError;
use std::path::PathBuf;

/// The default edge base URL (the dev edge from the deployable `edge` binary binds `127.0.0.1:8080`).
pub const DEFAULT_EDGE: &str = "http://127.0.0.1:8080";
/// The default token scheme — an UNBOUND machine/capability kind (NOT `pat`, which the edge requires
/// to be DPoP-bound).
pub const DEFAULT_SCHEME: &str = "agent";

/// The environment variables the CLI reads (named so a test / a script knows the contract).
pub mod env {
    /// The capability token (highest precedence after an explicit `--token`).
    pub const TOKEN: &str = "MYELIN_TOKEN";
    /// The token scheme (`agent` default).
    pub const SCHEME: &str = "MYELIN_TOKEN_SCHEME";
    /// The edge base URL.
    pub const EDGE: &str = "MYELIN_EDGE";
    /// An explicit config directory (overrides `$XDG_CONFIG_HOME`/`$HOME` — used by tests).
    pub const CONFIG_DIR: &str = "MYELIN_CONFIG_DIR";
}

/// The resolved connection config (the edge URL + the token scheme). The token is resolved
/// separately (and never stored on a struct that derives Debug) so it cannot leak via a `{:?}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeConfig {
    /// The edge base URL (`https://host:port` in production; loopback `http://` is development-only).
    pub url: String,
    /// The token scheme presented in `x-myelin-token-scheme`.
    pub scheme: String,
}

/// The config directory: `$MYELIN_CONFIG_DIR`, else `$XDG_CONFIG_HOME/myelin`, else
/// `$HOME/.config/myelin`. Total: a missing `$HOME` is a clean [`CliError::Config`], never a panic.
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

/// The path of the stored token file (`<config_dir>/token`).
pub fn token_path(getenv: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    Ok(config_dir(getenv)?.join("token"))
}

/// Resolve the edge URL + scheme from (in precedence order) the explicit flag, then the environment,
/// then the default.
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

/// **Resolve the capability token** (precedence: explicit `--token`, then `$MYELIN_TOKEN`, then the
/// stored `<config_dir>/token` file). Returns [`CliError::NotAuthenticated`] if none is available —
/// a clean, actionable error, never a panic. The returned string is the secret: callers MUST NOT log
/// it.
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

/// **Store a capability token** to `<config_dir>/token`, owner-readable only (`0600`). The token is
/// written to disk but NEVER echoed to stdout/stderr (the "never log the token" floor). Returns the
/// path it was written to (safe to print — it carries no secret).
pub fn store_token(token: &str, getenv: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, CliError> {
    let dir = config_dir(getenv)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| CliError::Config(format!("cannot create config dir {}: {e}", dir.display())))?;
    let path = dir.join("token");
    // R0.7-A (TOCTOU fix): create the file with mode 0600 ATOMICALLY *before* any bytes land, so the
    // capability token never exists on disk world-readable. The prior `fs::write` created the file
    // with the default umask (typically 0644) and only chmod'd 0600 afterwards — a window in which the
    // secret was world-readable. The atomic create closes that window; `set_owner_only` below is
    // belt-and-braces (it also tightens a pre-existing file whose perms were wrong).
    write_owner_only(&path, token.as_bytes())?;
    set_owner_only(&path)?;
    Ok(path)
}

/// Write `bytes` to `path`, creating the file with mode `0600` ATOMICALLY (the file never exists with
/// broader perms). R0.7-A: closes the TOCTOU window a plain `fs::write` + post-chmod leaves open.
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

/// On a non-unix host there is no mode bit to set atomically; write normally (the file lands under the
/// user's profile dir and the cross-platform ACL hardening is a deployment concern — mirrors
/// [`set_owner_only`]).
#[cfg(not(unix))]
fn write_owner_only(path: &std::path::Path, bytes: &[u8]) -> Result<(), CliError> {
    std::fs::write(path, bytes)
        .map_err(|e| CliError::Config(format!("cannot write token file {}: {e}", path.display())))
}

/// Set `0600` on the token file (owner read/write only) so a stored credential is not world-readable.
#[cfg(unix)]
fn set_owner_only(path: &std::path::Path) -> Result<(), CliError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| CliError::Config(format!("cannot set 0600 on {}: {e}", path.display())))
}

/// On a non-unix host the permission tightening is a named no-op (the file lands under the user's
/// profile dir); the cross-platform ACL hardening is a deployment concern.
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
        // flag wins
        let c = resolve_edge(Some("http://flag:1"), Some("agent"), &e);
        assert_eq!(c.url, "http://flag:1");
        assert_eq!(c.scheme, "agent");
        // env next
        let c = resolve_edge(None, None, &e);
        assert_eq!(c.url, "http://envhost:9");
        assert_eq!(c.scheme, "ci");
        // default last
        let c = resolve_edge(None, None, &env_from(&[]));
        assert_eq!(c.url, DEFAULT_EDGE);
        assert_eq!(c.scheme, DEFAULT_SCHEME);
    }

    #[test]
    fn resolve_token_precedence_and_clean_error_when_absent() {
        let no_file = |_: &std::path::Path| None;
        // flag wins
        assert_eq!(
            resolve_token(Some("TOK_FLAG"), &env_from(&[]), &no_file).unwrap(),
            "TOK_FLAG"
        );
        // env next
        assert_eq!(
            resolve_token(None, &env_from(&[("MYELIN_TOKEN", "TOK_ENV")]), &no_file).unwrap(),
            "TOK_ENV"
        );
        // file last
        let with_file = |_: &std::path::Path| Some("TOK_FILE\n".to_string());
        assert_eq!(
            resolve_token(None, &env_from(&[("MYELIN_CONFIG_DIR", "/c")]), &with_file).unwrap(),
            "TOK_FILE"
        );
        // none → a clean NotAuthenticated (exit 3), never a panic.
        let err = resolve_token(None, &env_from(&[("MYELIN_CONFIG_DIR", "/c")]), &no_file).unwrap_err();
        assert_eq!(err.code(), 3);
    }

    /// Storing then resolving round-trips, and the file is `0600` (owner-only) — the stored token is
    /// not world-readable.
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
            // R0.7-A: the file is 0600 immediately after `store_token`. Because the file is created
            // ATOMICALLY with mode 0600 (via `write_owner_only`'s `OpenOptions::mode(0o600)`) BEFORE
            // any bytes land, it was never created world-readable — the assertion on the final mode is
            // the observable proof; the atomic create closes the TOCTOU window a post-chmod would leave.
            assert_eq!(mode, 0o600, "the token file is owner-only");
        }
        let read = |p: &std::path::Path| std::fs::read_to_string(p).ok();
        assert_eq!(resolve_token(None, &getenv, &read).unwrap(), "ROUNDTRIP_TOKEN");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = path; // silence unused on non-unix
    }

    /// R0.7-A belt-and-braces: even if a token file already exists world-readable (0644), a subsequent
    /// `store_token` truncate-opens it and re-tightens to 0600 — the stored credential ends owner-only.
    #[cfg(unix)]
    #[test]
    fn store_token_tightens_a_preexisting_world_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("myelin-cli-test-preexist-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("token");
        // Pre-seed a world-readable file (simulating a token stored by an older, unpatched CLI).
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
