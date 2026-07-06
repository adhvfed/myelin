//! # myelin-config — the env-driven CONFIG layer (the dev<->prod CONFIG SWAP)
//!
//! Code talks to STANDARD interfaces (Postgres, S3, Redis-protocol, NATS) so moving from the
//! local docker-compose dev stack to the Scaleway (fr-par) prod backends is a CONFIG change,
//! not a code change. This crate is that seam: it reads the endpoints from the environment and
//! hands them to the trait impls (the OLTP client in myelin-storage/-substrate, the BlobStore
//! object impl, the cache, the NATS BusTransport) — which all live behind the `integration`
//! cargo feature.
//!
//! ## The env-var contract
//!
//! | var              | meaning                                  | dev default                                                  |
//! |------------------|------------------------------------------|--------------------------------------------------------------|
//! | `DATABASE_URL`   | Postgres OLTP + outbox + ReBAC + audit   | `postgres://myelin_app:myelin_app_pw@localhost:5433/myelin`  |
//! | `S3_ENDPOINT`    | S3-compatible object-store endpoint      | `http://localhost:9000`                                      |
//! | `S3_REGION`      | S3 region label                          | `fr-par`                                                     |
//! | `S3_ACCESS_KEY`  | S3 access key id                         | `myelin_dev_access`                                          |
//! | `S3_SECRET_KEY`  | S3 secret access key                     | `myelin_dev_secret`                                          |
//! | `S3_BUCKET`      | default object-store bucket              | `myelin-dev`                                                 |
//! | `REDIS_URL`      | Valkey/Redis cache URL                   | `redis://localhost:6380`                                     |
//! | `NATS_URL`       | NATS JetStream bus URL                   | `nats://localhost:4222`                                      |
//! | `MYELIN_REGION`  | data-residency region pin                | `fr-par`                                                     |
//!
//! Dev defaults point at `docker-compose.dev.yml`. In prod every var is supplied by the
//! environment and points at Scaleway (FR) — see `docs/dev-stack.md` for the endpoint mapping.
//!
//! ## Object-store path-style addressing
//!
//! [`S3Config::force_path_style`] is `true` so `aws-sdk-s3` addresses RustFS as
//! `http://endpoint/bucket/key` rather than the virtual-host `http://bucket.endpoint/key`
//! form (RustFS/MinIO-class servers want path-style). The same flag works for Scaleway.

#![forbid(unsafe_code)]

use std::env;

/// An error reading the env config. Loud + typed: a missing required var in prod is a
/// fail-fast at boot (architecture §3.2 — never a silent fallback to a wrong endpoint).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A required env var was absent and the chosen mode forbids a dev default.
    Missing(&'static str),
    /// A var was present but its value was not valid UTF-8 / empty where non-empty is required.
    Invalid {
        /// The offending env var name.
        var: &'static str,
        /// Why it was rejected.
        reason: String,
    },
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ConfigError::Missing(v) => write!(f, "required env var {v} is not set"),
            ConfigError::Invalid { var, reason } => write!(f, "env var {var} is invalid: {reason}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// The default residency region (the prod pin lives in `MYELIN_REGION=fr-par`).
pub const DEFAULT_REGION: &str = "fr-par";

// ---- dev defaults: every one points at docker-compose.dev.yml ----
const DEV_DATABASE_URL: &str = "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin";
const DEV_S3_ENDPOINT: &str = "http://localhost:9000";
const DEV_S3_REGION: &str = "fr-par";
const DEV_S3_ACCESS_KEY: &str = "myelin_dev_access";
const DEV_S3_SECRET_KEY: &str = "myelin_dev_secret";
const DEV_S3_BUCKET: &str = "myelin-dev";
const DEV_REDIS_URL: &str = "redis://localhost:6380";
const DEV_NATS_URL: &str = "nats://localhost:4222";

/// How [`MyelinConfig::from_env`] treats absent vars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Dev: a missing var falls back to the docker-compose default (developer convenience).
    DevDefaults,
    /// Prod: every endpoint var MUST be present (fail-fast at boot, no silent dev fallback).
    /// `MYELIN_REGION` still defaults to [`DEFAULT_REGION`] (`fr-par`) — the residency pin.
    RequireEnv,
}

/// The validated, env-first runtime config — the dev<->prod swap target.
///
/// R0.7-C: `Debug` is hand-written (NOT derived) because [`MyelinConfig::database_url`] and
/// [`MyelinConfig::redis_url`] are connection DSNs that embed a password in their userinfo
/// (`postgres://user:PASSWORD@host/db`); a derived `{:?}` in a log line, a panic, or an error
/// context would print that password in clear. The redacting impl below prints `<redacted>` for
/// those two fields (and defers the S3 credential redaction to [`S3Config`]'s own impl).
#[derive(Clone, PartialEq, Eq)]
pub struct MyelinConfig {
    /// Postgres OLTP connection string (OLTP + outbox + ReBAC tuple store + audit).
    pub database_url: String,
    /// The S3-compatible object-store config (RustFS in dev, Scaleway Object Storage in prod).
    pub s3: S3Config,
    /// Valkey/Redis cache URL.
    pub redis_url: String,
    /// NATS JetStream bus URL.
    pub nats_url: String,
    /// The data-residency region pin (`fr-par` in prod — the residency-pin lint's prod value).
    pub region: String,
}

/// The S3-compatible object-store config (the [`MyelinConfig::s3`] slice). Consumed by the
/// `aws-sdk-s3` BlobStore object impl behind the `integration` feature.
///
/// R0.7-C: `Debug` is hand-written (NOT derived) because [`S3Config::access_key`] and
/// [`S3Config::secret_key`] are plaintext credentials; a derived `{:?}` (in a log line, a panic,
/// or an error context) would print the S3 secret in clear. The redacting impl below prints
/// `<redacted>` for both credential fields; the non-secret fields print normally.
#[derive(Clone, PartialEq, Eq)]
pub struct S3Config {
    /// Custom endpoint URL (RustFS/Scaleway) — NOT the AWS default endpoint.
    pub endpoint: String,
    /// Region label.
    pub region: String,
    /// Static access key id (dev creds in dev; Scaleway IAM in prod).
    pub access_key: String,
    /// Static secret access key.
    pub secret_key: String,
    /// Default bucket.
    pub bucket: String,
    /// Use path-style addressing (`true` for RustFS/MinIO-class + Scaleway). The
    /// `aws-sdk-s3` `force_path_style` knob.
    pub force_path_style: bool,
}

impl core::fmt::Debug for S3Config {
    /// R0.7-C: redact the credential fields so a `{:?}` never prints the S3 secret/access key.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("bucket", &self.bucket)
            .field("force_path_style", &self.force_path_style)
            .finish()
    }
}

impl core::fmt::Debug for MyelinConfig {
    /// R0.7-C: redact the credential-bearing DSN fields (`database_url`, `redis_url`) so a `{:?}`
    /// never prints the password embedded in their userinfo. `s3` defers to [`S3Config`]'s own
    /// redacting impl; `nats_url`/`region` carry no credential and print normally.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MyelinConfig")
            .field("database_url", &"<redacted>")
            .field("s3", &self.s3)
            .field("redis_url", &"<redacted>")
            .field("nats_url", &self.nats_url)
            .field("region", &self.region)
            .finish()
    }
}

impl MyelinConfig {
    /// Read the config from the process environment using `mode`.
    ///
    /// In [`Mode::DevDefaults`] an absent var falls back to its docker-compose default. In
    /// [`Mode::RequireEnv`] (prod) an absent endpoint var is a [`ConfigError::Missing`]
    /// (fail-fast). `MYELIN_REGION` defaults to `fr-par` in BOTH modes — the residency pin.
    pub fn from_env(mode: Mode) -> Result<MyelinConfig, ConfigError> {
        let database_url = req(mode, "DATABASE_URL", DEV_DATABASE_URL)?;
        let s3 = S3Config {
            endpoint: req(mode, "S3_ENDPOINT", DEV_S3_ENDPOINT)?,
            region: req(mode, "S3_REGION", DEV_S3_REGION)?,
            access_key: req(mode, "S3_ACCESS_KEY", DEV_S3_ACCESS_KEY)?,
            secret_key: req(mode, "S3_SECRET_KEY", DEV_S3_SECRET_KEY)?,
            bucket: req(mode, "S3_BUCKET", DEV_S3_BUCKET)?,
            // Path-style is the correct addressing for RustFS and Scaleway alike.
            force_path_style: true,
        };
        let redis_url = req(mode, "REDIS_URL", DEV_REDIS_URL)?;
        let nats_url = req(mode, "NATS_URL", DEV_NATS_URL)?;
        // MYELIN_REGION is the residency pin: it defaults to fr-par in BOTH modes (the lint's
        // prod value). An empty value is rejected — a blank region is never a valid pin.
        let region = match read(mode, "MYELIN_REGION") {
            Some(v) if v.trim().is_empty() => {
                return Err(ConfigError::Invalid {
                    var: "MYELIN_REGION",
                    reason: "must not be empty".into(),
                })
            }
            Some(v) => v,
            None => DEFAULT_REGION.to_string(),
        };

        Ok(MyelinConfig {
            database_url,
            s3,
            redis_url,
            nats_url,
            region,
        })
    }

    /// Convenience: the dev config (the docker-compose stack) with no env reads required.
    pub fn dev() -> MyelinConfig {
        // Cannot fail: every default is a non-empty literal.
        MyelinConfig::from_env(Mode::DevDefaults).expect("dev defaults are always valid")
    }
}

/// Read an env var, returning `None` for absent OR present-but-empty (an empty endpoint is
/// treated as unset so a stray `VAR=` does not become a silent bad endpoint).
fn read(_mode: Mode, var: &'static str) -> Option<String> {
    match env::var(var) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// Resolve a required endpoint var per `mode`: dev falls back to `dev_default`; prod fails fast.
fn req(mode: Mode, var: &'static str, dev_default: &str) -> Result<String, ConfigError> {
    match read(mode, var) {
        Some(v) => Ok(v),
        None => match mode {
            Mode::DevDefaults => Ok(dev_default.to_string()),
            Mode::RequireEnv => Err(ConfigError::Missing(var)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests mutate the process env, so they MUST run serialized. We gate the whole
    // module behind a single mutex to avoid cross-test env races (cargo runs tests in
    // parallel threads within a binary).
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear() {
        for v in [
            "DATABASE_URL",
            "S3_ENDPOINT",
            "S3_REGION",
            "S3_ACCESS_KEY",
            "S3_SECRET_KEY",
            "S3_BUCKET",
            "REDIS_URL",
            "NATS_URL",
            "MYELIN_REGION",
        ] {
            env::remove_var(v);
        }
    }

    #[test]
    fn dev_defaults_point_at_compose_stack() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        let cfg = MyelinConfig::from_env(Mode::DevDefaults).unwrap();
        assert_eq!(cfg.database_url, DEV_DATABASE_URL);
        assert_eq!(cfg.s3.endpoint, "http://localhost:9000");
        assert_eq!(cfg.s3.bucket, "myelin-dev");
        assert!(cfg.s3.force_path_style);
        assert_eq!(cfg.redis_url, "redis://localhost:6380");
        assert_eq!(cfg.nats_url, "nats://localhost:4222");
        // The residency pin defaults to fr-par.
        assert_eq!(cfg.region, "fr-par");
    }

    #[test]
    fn prod_requires_env_and_fails_fast() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        let err = MyelinConfig::from_env(Mode::RequireEnv).unwrap_err();
        assert_eq!(err, ConfigError::Missing("DATABASE_URL"));
    }

    #[test]
    fn region_pin_defaults_fr_par_even_in_prod() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        // Supply every endpoint var but NOT MYELIN_REGION.
        env::set_var("DATABASE_URL", "postgres://prod/db");
        env::set_var("S3_ENDPOINT", "https://s3.fr-par.scw.cloud");
        env::set_var("S3_REGION", "fr-par");
        env::set_var("S3_ACCESS_KEY", "k");
        env::set_var("S3_SECRET_KEY", "s");
        env::set_var("S3_BUCKET", "myelin-prod");
        env::set_var("REDIS_URL", "rediss://prod:6379");
        env::set_var("NATS_URL", "nats://prod:4222");
        let cfg = MyelinConfig::from_env(Mode::RequireEnv).unwrap();
        assert_eq!(cfg.region, DEFAULT_REGION);
        assert_eq!(cfg.s3.endpoint, "https://s3.fr-par.scw.cloud");
        clear();
    }

    #[test]
    fn env_override_wins_in_dev() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("DATABASE_URL", "postgres://custom/x");
        env::set_var("MYELIN_REGION", "fr-par");
        let cfg = MyelinConfig::from_env(Mode::DevDefaults).unwrap();
        assert_eq!(cfg.database_url, "postgres://custom/x");
        clear();
    }

    /// R0.7-C: a `{:?}` of the config must NOT contain the S3 secret / access key nor the DB
    /// password embedded in the DSN — it must print the `<redacted>` marker instead.
    #[test]
    fn debug_redacts_secrets() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("DATABASE_URL", "postgres://dbuser:SUPER_SECRET_DB_PW@host:5432/myelin");
        env::set_var("S3_ENDPOINT", "https://s3.fr-par.scw.cloud");
        env::set_var("S3_REGION", "fr-par");
        env::set_var("S3_ACCESS_KEY", "AKIA_SECRET_ACCESS_ID");
        env::set_var("S3_SECRET_KEY", "TOP_SECRET_S3_KEY_MATERIAL");
        env::set_var("S3_BUCKET", "myelin-prod");
        env::set_var("REDIS_URL", "rediss://redisuser:REDIS_SECRET_PW@prod:6379");
        env::set_var("NATS_URL", "nats://prod:4222");
        let cfg = MyelinConfig::from_env(Mode::RequireEnv).unwrap();

        // S3Config on its own redacts both credential fields.
        let s3_dbg = format!("{:?}", cfg.s3);
        assert!(!s3_dbg.contains("AKIA_SECRET_ACCESS_ID"), "access_key leaked: {s3_dbg}");
        assert!(!s3_dbg.contains("TOP_SECRET_S3_KEY_MATERIAL"), "secret_key leaked: {s3_dbg}");
        assert!(s3_dbg.contains("<redacted>"), "expected a redaction marker: {s3_dbg}");
        assert!(s3_dbg.contains("myelin-prod"), "non-secret bucket should still print: {s3_dbg}");

        // The whole config's Debug redacts the S3 secret AND the DSN passwords.
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("TOP_SECRET_S3_KEY_MATERIAL"), "s3 secret leaked via MyelinConfig: {dbg}");
        assert!(!dbg.contains("SUPER_SECRET_DB_PW"), "db password leaked: {dbg}");
        assert!(!dbg.contains("REDIS_SECRET_PW"), "redis password leaked: {dbg}");
        assert!(dbg.contains("<redacted>"), "expected a redaction marker: {dbg}");
        clear();
    }

    #[test]
    fn empty_region_is_rejected() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("MYELIN_REGION", "");
        // Empty var is treated as unset by `read`, so it falls back to the pin — NOT rejected.
        // To hit the Invalid branch the var must be present-and-whitespace.
        env::set_var("MYELIN_REGION", "   ");
        let err = MyelinConfig::from_env(Mode::DevDefaults).unwrap_err();
        assert_eq!(
            err,
            ConfigError::Invalid {
                var: "MYELIN_REGION",
                reason: "must not be empty".into()
            }
        );
        clear();
    }
}
