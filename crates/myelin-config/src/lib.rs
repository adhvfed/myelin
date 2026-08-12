#![forbid(unsafe_code)]

use std::{env, io::Read as _};

pub const OIDC_JWKS_MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing(&'static str),
    Invalid { var: &'static str, reason: String },
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

pub const DEFAULT_REGION: &str = "fr-par";

const DEV_DATABASE_URL: &str = "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin";
const DEV_DATABASE_MIGRATION_URL: &str =
    "postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin";
const DEV_S3_ENDPOINT: &str = "http://localhost:9000";
const DEV_S3_REGION: &str = "fr-par";
const DEV_S3_ACCESS_KEY: &str = "myelin_dev_access";
const DEV_S3_SECRET_KEY: &str = "myelin_dev_secret";
const DEV_S3_BUCKET: &str = "myelin-dev";
const DEV_REDIS_URL: &str = "redis://localhost:6380";
const DEV_NATS_URL: &str = "nats://localhost:4222";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    DevDefaults,
    RequireEnv,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MyelinConfig {
    pub database_url: String,
    pub database_migration_url: String,
    pub s3: S3Config,
    pub redis_url: String,
    pub nats_url: String,
    pub region: String,
    pub oidc: Option<OidcSettings>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct OidcSettings {
    pub issuer: String,
    pub audience: String,
    pub jwks_json: Option<String>,
    pub jwks_uri: Option<String>,
}

impl core::fmt::Debug for OidcSettings {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OidcSettings")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field(
                "jwks_json",
                &self
                    .jwks_json
                    .as_ref()
                    .map(|json| format!("<{} bytes>", json.len())),
            )
            .field("jwks_uri", &self.jwks_uri.as_ref().map(|_| "<configured>"))
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct S3Config {
    pub endpoint: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
    pub force_path_style: bool,
}

impl core::fmt::Debug for S3Config {
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
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MyelinConfig")
            .field("database_url", &"<redacted>")
            .field("database_migration_url", &"<redacted>")
            .field("s3", &self.s3)
            .field("redis_url", &"<redacted>")
            .field("nats_url", &"<redacted>")
            .field("region", &self.region)
            .field("oidc", &self.oidc)
            .finish()
    }
}

impl MyelinConfig {
    pub fn from_env(mode: Mode) -> Result<MyelinConfig, ConfigError> {
        let database_url = req(mode, "DATABASE_URL", DEV_DATABASE_URL)?;
        let database_migration_url =
            req(mode, "DATABASE_MIGRATION_URL", DEV_DATABASE_MIGRATION_URL)?;
        if database_url == database_migration_url {
            return Err(ConfigError::Invalid {
                var: "DATABASE_MIGRATION_URL",
                reason: "must use a credential distinct from DATABASE_URL".into(),
            });
        }
        let s3 = S3Config {
            endpoint: req(mode, "S3_ENDPOINT", DEV_S3_ENDPOINT)?,
            region: req(mode, "S3_REGION", DEV_S3_REGION)?,
            access_key: req(mode, "S3_ACCESS_KEY", DEV_S3_ACCESS_KEY)?,
            secret_key: req(mode, "S3_SECRET_KEY", DEV_S3_SECRET_KEY)?,
            bucket: req(mode, "S3_BUCKET", DEV_S3_BUCKET)?,
            force_path_style: true,
        };
        let redis_url = req(mode, "REDIS_URL", DEV_REDIS_URL)?;
        let nats_url = req(mode, "NATS_URL", DEV_NATS_URL)?;
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

        let oidc = oidc_from_env(mode)?;

        Ok(MyelinConfig {
            database_url,
            database_migration_url,
            s3,
            redis_url,
            nats_url,
            region,
            oidc,
        })
    }

    pub fn dev() -> MyelinConfig {
        MyelinConfig::from_env(Mode::DevDefaults).expect("dev defaults are always valid")
    }
}

fn read(_mode: Mode, var: &'static str) -> Option<String> {
    match env::var(var) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

fn req(mode: Mode, var: &'static str, dev_default: &str) -> Result<String, ConfigError> {
    match env::var(var) {
        Ok(v) if v.trim().is_empty() => Err(ConfigError::Invalid {
            var,
            reason: "must not be empty".into(),
        }),
        Ok(v) => Ok(v),
        Err(env::VarError::NotPresent) => match mode {
            Mode::DevDefaults => Ok(dev_default.to_string()),
            Mode::RequireEnv => Err(ConfigError::Missing(var)),
        },
        Err(env::VarError::NotUnicode(_)) => Err(ConfigError::Invalid {
            var,
            reason: "must be valid UTF-8".into(),
        }),
    }
}

fn non_empty(value: String, var: &'static str) -> Result<String, ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::Invalid {
            var,
            reason: "must not be empty".into(),
        });
    }
    Ok(value)
}

fn bounded_jwks(document: String, var: &'static str) -> Result<String, ConfigError> {
    if document.trim().is_empty() {
        return Err(ConfigError::Invalid {
            var,
            reason: "must not be empty".into(),
        });
    }
    if document.len() > OIDC_JWKS_MAX_BYTES {
        return Err(ConfigError::Invalid {
            var,
            reason: format!("must not exceed {OIDC_JWKS_MAX_BYTES} bytes"),
        });
    }
    Ok(document)
}

fn read_jwks_file(path: &str) -> Result<String, ConfigError> {
    let file = std::fs::File::open(path).map_err(|error| ConfigError::Invalid {
        var: "MYELIN_OIDC_JWKS_FILE",
        reason: format!("cannot open JWKS file: {error}"),
    })?;
    let mut document = String::new();
    file.take((OIDC_JWKS_MAX_BYTES + 1) as u64)
        .read_to_string(&mut document)
        .map_err(|error| ConfigError::Invalid {
            var: "MYELIN_OIDC_JWKS_FILE",
            reason: format!("cannot read JWKS file: {error}"),
        })?;
    bounded_jwks(document, "MYELIN_OIDC_JWKS_FILE")
}

fn oidc_from_env(mode: Mode) -> Result<Option<OidcSettings>, ConfigError> {
    let issuer = read(mode, "MYELIN_OIDC_ISSUER");
    let audience = read(mode, "MYELIN_OIDC_AUDIENCE");
    let jwks_inline = read(mode, "MYELIN_OIDC_JWKS");
    let jwks_file = read(mode, "MYELIN_OIDC_JWKS_FILE");
    let jwks_uri = read(mode, "MYELIN_OIDC_JWKS_URI");

    if issuer.is_none()
        && audience.is_none()
        && jwks_inline.is_none()
        && jwks_file.is_none()
        && jwks_uri.is_none()
    {
        return Ok(None);
    }

    let issuer = non_empty(
        issuer.ok_or(ConfigError::Missing("MYELIN_OIDC_ISSUER"))?,
        "MYELIN_OIDC_ISSUER",
    )?;
    let audience = non_empty(
        audience.ok_or(ConfigError::Missing("MYELIN_OIDC_AUDIENCE"))?,
        "MYELIN_OIDC_AUDIENCE",
    )?;
    let jwks_json = match (jwks_inline, jwks_file) {
        (Some(_), Some(_)) => {
            return Err(ConfigError::Invalid {
                var: "MYELIN_OIDC_JWKS",
                reason: "set only ONE JWKS source - MYELIN_OIDC_JWKS (inline JSON) OR \
                         MYELIN_OIDC_JWKS_FILE (a path), not both"
                    .into(),
            })
        }
        (Some(json), None) => Some(bounded_jwks(json, "MYELIN_OIDC_JWKS")?),
        (None, Some(path)) => Some(read_jwks_file(&path)?),
        (None, None) => None,
    };
    if mode == Mode::RequireEnv && jwks_uri.is_none() {
        return Err(ConfigError::Missing("MYELIN_OIDC_JWKS_URI"));
    }
    if jwks_json.is_none() && jwks_uri.is_none() {
        return Err(ConfigError::Missing("MYELIN_OIDC_JWKS"));
    }

    Ok(Some(OidcSettings {
        issuer,
        audience,
        jwks_json,
        jwks_uri: jwks_uri
            .map(|value| non_empty(value, "MYELIN_OIDC_JWKS_URI"))
            .transpose()?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear() {
        for v in [
            "DATABASE_URL",
            "DATABASE_MIGRATION_URL",
            "S3_ENDPOINT",
            "S3_REGION",
            "S3_ACCESS_KEY",
            "S3_SECRET_KEY",
            "S3_BUCKET",
            "REDIS_URL",
            "NATS_URL",
            "MYELIN_REGION",
            "MYELIN_OIDC_ISSUER",
            "MYELIN_OIDC_AUDIENCE",
            "MYELIN_OIDC_JWKS",
            "MYELIN_OIDC_JWKS_FILE",
            "MYELIN_OIDC_JWKS_URI",
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
        assert_eq!(cfg.database_migration_url, DEV_DATABASE_MIGRATION_URL);
        assert_ne!(cfg.database_url, cfg.database_migration_url);
        assert_eq!(cfg.s3.endpoint, "http://localhost:9000");
        assert_eq!(cfg.s3.bucket, "myelin-dev");
        assert!(cfg.s3.force_path_style);
        assert_eq!(cfg.redis_url, "redis://localhost:6380");
        assert_eq!(cfg.nats_url, "nats://localhost:4222");
        assert_eq!(cfg.region, "fr-par");
    }

    #[test]
    fn prod_requires_env_and_fails_fast() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        let err = MyelinConfig::from_env(Mode::RequireEnv).unwrap_err();
        assert_eq!(err, ConfigError::Missing("DATABASE_URL"));

        env::set_var("DATABASE_URL", "postgres://runtime-secret@prod/myelin");
        let err = MyelinConfig::from_env(Mode::RequireEnv).unwrap_err();
        assert_eq!(err, ConfigError::Missing("DATABASE_MIGRATION_URL"));
        assert!(!format!("{err:?} {err}").contains("runtime-secret"));
        clear();
    }

    #[test]
    fn database_credentials_reject_empty_and_identical_values() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("DATABASE_URL", " ");
        let err = MyelinConfig::from_env(Mode::DevDefaults).unwrap_err();
        assert_eq!(
            err,
            ConfigError::Invalid {
                var: "DATABASE_URL",
                reason: "must not be empty".into(),
            }
        );

        env::set_var("DATABASE_URL", "postgres://runtime:secret@prod/myelin");
        env::set_var("DATABASE_MIGRATION_URL", "\t");
        let err = MyelinConfig::from_env(Mode::DevDefaults).unwrap_err();
        assert_eq!(
            err,
            ConfigError::Invalid {
                var: "DATABASE_MIGRATION_URL",
                reason: "must not be empty".into(),
            }
        );

        env::set_var(
            "DATABASE_MIGRATION_URL",
            "postgres://runtime:secret@prod/myelin",
        );
        let err = MyelinConfig::from_env(Mode::DevDefaults).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "DATABASE_MIGRATION_URL",
                ..
            }
        ));
        let rendered = format!("{err:?} {err}");
        assert!(!rendered.contains("runtime:secret"));
        clear();
    }

    #[test]
    fn region_pin_defaults_fr_par_even_in_prod() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("DATABASE_URL", "postgres://prod/db");
        env::set_var("DATABASE_MIGRATION_URL", "postgres://migrator@prod/db");
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
        env::set_var("DATABASE_MIGRATION_URL", "postgres://custom-admin/x");
        env::set_var("MYELIN_REGION", "fr-par");
        let cfg = MyelinConfig::from_env(Mode::DevDefaults).unwrap();
        assert_eq!(cfg.database_url, "postgres://custom/x");
        clear();
    }

    #[test]
    fn debug_redacts_secrets() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var(
            "DATABASE_URL",
            "postgres://dbuser:SUPER_SECRET_DB_PW@host:5432/myelin",
        );
        env::set_var(
            "DATABASE_MIGRATION_URL",
            "postgres://migrator:MIGRATION_SECRET_DB_PW@host:5432/myelin",
        );
        env::set_var("S3_ENDPOINT", "https://s3.fr-par.scw.cloud");
        env::set_var("S3_REGION", "fr-par");
        let access_key = ["AK", "IA_SECRET_ACCESS_ID"].concat();
        env::set_var("S3_ACCESS_KEY", &access_key);
        env::set_var("S3_SECRET_KEY", "TOP_SECRET_S3_KEY_MATERIAL");
        env::set_var("S3_BUCKET", "myelin-prod");
        env::set_var("REDIS_URL", "rediss://redisuser:REDIS_SECRET_PW@prod:6379");
        env::set_var("NATS_URL", "nats://natsuser:NATS_SECRET_PW@prod:4222");
        let cfg = MyelinConfig::from_env(Mode::RequireEnv).unwrap();

        let s3_dbg = format!("{:?}", cfg.s3);
        assert!(!s3_dbg.contains(&access_key), "access_key leaked: {s3_dbg}");
        assert!(
            !s3_dbg.contains("TOP_SECRET_S3_KEY_MATERIAL"),
            "secret_key leaked: {s3_dbg}"
        );
        assert!(
            s3_dbg.contains("<redacted>"),
            "expected a redaction marker: {s3_dbg}"
        );
        assert!(
            s3_dbg.contains("myelin-prod"),
            "non-secret bucket should still print: {s3_dbg}"
        );

        let dbg = format!("{cfg:?}");
        assert!(
            !dbg.contains("TOP_SECRET_S3_KEY_MATERIAL"),
            "s3 secret leaked via MyelinConfig: {dbg}"
        );
        assert!(
            !dbg.contains("SUPER_SECRET_DB_PW"),
            "db password leaked: {dbg}"
        );
        assert!(
            !dbg.contains("MIGRATION_SECRET_DB_PW"),
            "migration db password leaked: {dbg}"
        );
        assert!(
            !dbg.contains("REDIS_SECRET_PW"),
            "redis password leaked: {dbg}"
        );
        assert!(
            !dbg.contains("NATS_SECRET_PW"),
            "NATS password leaked: {dbg}"
        );
        assert!(
            dbg.contains("<redacted>"),
            "expected a redaction marker: {dbg}"
        );
        clear();
    }

    #[test]
    fn oidc_absent_is_none_and_boots() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        assert!(MyelinConfig::from_env(Mode::DevDefaults)
            .unwrap()
            .oidc
            .is_none());
        env::set_var("DATABASE_URL", "postgres://prod/db");
        env::set_var("DATABASE_MIGRATION_URL", "postgres://migrator@prod/db");
        env::set_var("S3_ENDPOINT", "https://s3.fr-par.scw.cloud");
        env::set_var("S3_REGION", "fr-par");
        env::set_var("S3_ACCESS_KEY", "k");
        env::set_var("S3_SECRET_KEY", "s");
        env::set_var("S3_BUCKET", "myelin-prod");
        env::set_var("REDIS_URL", "rediss://prod:6379");
        env::set_var("NATS_URL", "nats://prod:4222");
        assert!(MyelinConfig::from_env(Mode::RequireEnv)
            .unwrap()
            .oidc
            .is_none());
        clear();
    }

    #[test]
    fn oidc_fully_set_is_wired() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        let jwks = r#"{"keys":[{"kty":"RSA","kid":"k1","n":"AQAB","e":"AQAB"}]}"#;
        env::set_var("MYELIN_OIDC_ISSUER", "https://idp.example.com");
        env::set_var("MYELIN_OIDC_AUDIENCE", "myelin-rp");
        env::set_var("MYELIN_OIDC_JWKS", jwks);
        let cfg = MyelinConfig::from_env(Mode::DevDefaults).unwrap();
        let oidc = cfg.oidc.expect("oidc must be Some");
        assert_eq!(oidc.issuer, "https://idp.example.com");
        assert_eq!(oidc.audience, "myelin-rp");
        assert_eq!(oidc.jwks_json.as_deref(), Some(jwks));
        assert_eq!(oidc.jwks_uri, None);
        let dbg = format!("{oidc:?}");
        assert!(
            dbg.contains("<"),
            "jwks_json should print a byte summary: {dbg}"
        );
        assert!(
            !dbg.contains("AQAB"),
            "jwks_json body should not be dumped: {dbg}"
        );
        clear();
    }

    #[test]
    fn production_oidc_requires_refresh_uri() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("MYELIN_OIDC_ISSUER", "https://idp.example.com");
        env::set_var("MYELIN_OIDC_AUDIENCE", "myelin-rp");
        env::set_var("MYELIN_OIDC_JWKS", r#"{"keys":[]}"#);
        assert_eq!(
            oidc_from_env(Mode::RequireEnv).unwrap_err(),
            ConfigError::Missing("MYELIN_OIDC_JWKS_URI")
        );

        env::remove_var("MYELIN_OIDC_JWKS");
        env::set_var(
            "MYELIN_OIDC_JWKS_URI",
            "https://idp.example.com/.well-known/jwks.json",
        );
        let oidc = oidc_from_env(Mode::RequireEnv)
            .unwrap()
            .expect("OIDC should be configured");
        assert_eq!(oidc.jwks_json, None);
        assert_eq!(
            oidc.jwks_uri.as_deref(),
            Some("https://idp.example.com/.well-known/jwks.json")
        );
        clear();
    }

    #[test]
    fn oidc_debug_redacts_uri_user_info() {
        let oidc = OidcSettings {
            issuer: "https://idp.example.com".into(),
            audience: "myelin-rp".into(),
            jwks_json: None,
            jwks_uri: Some("https://user:TOP_SECRET@idp.example.com/jwks".into()),
        };
        let dbg = format!("{oidc:?}");
        assert!(!dbg.contains("TOP_SECRET"), "JWKS URI leaked: {dbg}");
        assert!(dbg.contains("<configured>"));
    }

    #[test]
    fn oidc_partial_fails_loud() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("MYELIN_OIDC_ISSUER", "https://idp.example.com");
        let err = MyelinConfig::from_env(Mode::DevDefaults).unwrap_err();
        assert_eq!(err, ConfigError::Missing("MYELIN_OIDC_AUDIENCE"));
        env::set_var("MYELIN_OIDC_AUDIENCE", "myelin-rp");
        let err = MyelinConfig::from_env(Mode::DevDefaults).unwrap_err();
        assert_eq!(err, ConfigError::Missing("MYELIN_OIDC_JWKS"));
        env::set_var("MYELIN_OIDC_JWKS", "{}");
        env::set_var("MYELIN_OIDC_JWKS_FILE", "/tmp/nope.json");
        let err = MyelinConfig::from_env(Mode::DevDefaults).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "MYELIN_OIDC_JWKS",
                ..
            }
        ));
        clear();
    }

    #[test]
    fn oidc_rejects_blank_identity_and_oversized_bootstrap() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("MYELIN_OIDC_ISSUER", "   ");
        env::set_var("MYELIN_OIDC_AUDIENCE", "myelin-rp");
        env::set_var("MYELIN_OIDC_JWKS", "{}");
        assert!(matches!(
            oidc_from_env(Mode::DevDefaults),
            Err(ConfigError::Invalid {
                var: "MYELIN_OIDC_ISSUER",
                ..
            })
        ));

        env::set_var("MYELIN_OIDC_ISSUER", "https://idp.example.com");
        env::set_var("MYELIN_OIDC_JWKS", "x".repeat(OIDC_JWKS_MAX_BYTES + 1));
        assert!(matches!(
            oidc_from_env(Mode::DevDefaults),
            Err(ConfigError::Invalid {
                var: "MYELIN_OIDC_JWKS",
                ..
            })
        ));
        clear();
    }

    #[test]
    fn empty_region_is_rejected() {
        let _g = ENV_LOCK.lock().unwrap();
        clear();
        env::set_var("MYELIN_REGION", "");
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
