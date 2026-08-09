use base64::Engine as _;
use myelin_identity::Principal;
use myelin_storage::migration::{Migration, Migrations};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::BTreeSet;
#[cfg(test)]
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use tokio::runtime::{Handle, RuntimeFlavor};

pub const DEVICE_AUTHORIZATION_TTL_SECS: i64 = 10 * 60;
pub const DEVICE_AUTHORIZATION_POLL_INTERVAL_SECS: u64 = 2;
pub const DEVICE_AUTHORIZATION_TABLE: &str = "auth_device_authorization";

const SECRET_BYTES: usize = 32;
const SECRET_B64URL_LEN: usize = 43;
const USER_CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

const DEVICE_AUTHORIZATION_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS auth_device_authorization (
    device_digest       bytea  PRIMARY KEY CHECK (octet_length(device_digest) = 32),
    user_code_digest    bytea  NOT NULL UNIQUE CHECK (octet_length(user_code_digest) = 32),
    verifier_challenge  bytea  NOT NULL CHECK (octet_length(verifier_challenge) = 32),
    expires_at_unix     bigint NOT NULL,
    approved_at_unix    bigint,
    approved_principal  jsonb,
    approved_authority  jsonb,
    source_expires_unix bigint,
    CHECK (
      (approved_at_unix IS NULL AND approved_principal IS NULL
       AND approved_authority IS NULL AND source_expires_unix IS NULL)
      OR
      (approved_at_unix IS NOT NULL AND approved_principal IS NOT NULL
       AND approved_authority IS NOT NULL AND source_expires_unix IS NOT NULL)
    )
);
CREATE INDEX IF NOT EXISTS auth_device_authorization_expiry_idx
    ON auth_device_authorization (expires_at_unix);
REVOKE ALL ON TABLE auth_device_authorization FROM PUBLIC;
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE auth_device_authorization TO myelin_app;
"#;

// Kept as a forward migration because edge_0001 may already be present in a running cell. These
// caps make a corrupt or compromised writer fail at the database boundary before a poll can
// materialize an unbounded JSON value.
const DEVICE_AUTHORIZATION_APPROVAL_BOUNDS_DDL: &str = r#"
ALTER TABLE auth_device_authorization
  ADD CONSTRAINT auth_device_authorization_principal_size
  CHECK (approved_principal IS NULL OR pg_column_size(approved_principal) <= 8192);
ALTER TABLE auth_device_authorization
  ADD CONSTRAINT auth_device_authorization_authority_shape
  CHECK (approved_authority IS NULL OR
         (jsonb_typeof(approved_authority) = 'array' AND
          pg_column_size(approved_authority) <= 32768));
"#;

pub fn device_authorization_migrations() -> Migrations {
    Migrations::of([
        Migration::plain(
            "edge_0001_auth_device_authorization",
            DEVICE_AUTHORIZATION_DDL,
        ),
        Migration::plain(
            "edge_0002_auth_device_approval_bounds",
            DEVICE_AUTHORIZATION_APPROVAL_BOUNDS_DDL,
        ),
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceAuthorizationStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DeviceApproval {
    pub principal: Principal,
    pub authority: Vec<String>,
    pub source_expires_at_unix: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingAuthorization {
    device_digest: [u8; 32],
    user_code_digest: [u8; 32],
    verifier_challenge: [u8; 32],
    expires_at_unix: i64,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredAuthorization {
    pending: PendingAuthorization,
    approval: Option<DeviceApproval>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApprovalOutcome {
    Approved,
    AlreadyApproved,
    Expired,
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClaimOutcome {
    Pending,
    Approved(DeviceApproval),
    Expired,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceAuthorizationStoreError(String);

impl DeviceAuthorizationStoreError {
    fn database(error: sqlx::Error) -> Self {
        Self(format!(
            "device authorization database operation failed: {error}"
        ))
    }

    fn corrupt(detail: impl Into<String>) -> Self {
        Self(format!(
            "stored device authorization is invalid: {}",
            detail.into()
        ))
    }
}

impl core::fmt::Display for DeviceAuthorizationStoreError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DeviceAuthorizationStoreError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeviceAuthorizationError {
    InvalidInput(&'static str),
    Store(DeviceAuthorizationStoreError),
}

impl core::fmt::Display for DeviceAuthorizationError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for DeviceAuthorizationError {}

trait DeviceAuthorizationStore: Send + Sync {
    fn issue(
        &self,
        authorization: PendingAuthorization,
        now_unix: i64,
    ) -> Result<bool, DeviceAuthorizationStoreError>;

    fn approve(
        &self,
        user_code_digest: [u8; 32],
        approval: DeviceApproval,
        now_unix: i64,
    ) -> Result<ApprovalOutcome, DeviceAuthorizationStoreError>;

    fn claim(
        &self,
        device_digest: [u8; 32],
        verifier_challenge: [u8; 32],
        now_unix: i64,
    ) -> Result<ClaimOutcome, DeviceAuthorizationStoreError>;
}

#[cfg(test)]
#[derive(Default)]
struct MemoryDeviceAuthorizationStore {
    authorizations: Mutex<HashMap<[u8; 32], StoredAuthorization>>,
}

#[cfg(test)]
impl DeviceAuthorizationStore for MemoryDeviceAuthorizationStore {
    fn issue(
        &self,
        authorization: PendingAuthorization,
        now_unix: i64,
    ) -> Result<bool, DeviceAuthorizationStoreError> {
        let mut authorizations = self
            .authorizations
            .lock()
            .expect("device store mutex poisoned");
        authorizations.retain(|_, stored| stored.pending.expires_at_unix > now_unix);
        if authorizations.contains_key(&authorization.device_digest)
            || authorizations
                .values()
                .any(|stored| stored.pending.user_code_digest == authorization.user_code_digest)
        {
            return Ok(false);
        }
        authorizations.insert(
            authorization.device_digest,
            StoredAuthorization {
                pending: authorization,
                approval: None,
            },
        );
        Ok(true)
    }

    fn approve(
        &self,
        user_code_digest: [u8; 32],
        approval: DeviceApproval,
        now_unix: i64,
    ) -> Result<ApprovalOutcome, DeviceAuthorizationStoreError> {
        let mut authorizations = self
            .authorizations
            .lock()
            .expect("device store mutex poisoned");
        let device_digest = authorizations.iter().find_map(|(device_digest, stored)| {
            (stored.pending.user_code_digest == user_code_digest).then_some(*device_digest)
        });
        let Some(device_digest) = device_digest else {
            return Ok(ApprovalOutcome::NotFound);
        };
        let stored = authorizations
            .get_mut(&device_digest)
            .expect("device digest came from this map");
        if stored.pending.expires_at_unix <= now_unix {
            authorizations.remove(&device_digest);
            return Ok(ApprovalOutcome::Expired);
        }
        if let Some(existing) = &stored.approval {
            return Ok(if existing == &approval {
                ApprovalOutcome::AlreadyApproved
            } else {
                ApprovalOutcome::NotFound
            });
        }
        stored.approval = Some(approval);
        Ok(ApprovalOutcome::Approved)
    }

    fn claim(
        &self,
        device_digest: [u8; 32],
        verifier_challenge: [u8; 32],
        now_unix: i64,
    ) -> Result<ClaimOutcome, DeviceAuthorizationStoreError> {
        let mut authorizations = self
            .authorizations
            .lock()
            .expect("device store mutex poisoned");
        let Some(stored) = authorizations.get(&device_digest) else {
            return Ok(ClaimOutcome::Invalid);
        };
        if stored.pending.verifier_challenge != verifier_challenge {
            return Ok(ClaimOutcome::Invalid);
        }
        if stored.pending.expires_at_unix <= now_unix {
            authorizations.remove(&device_digest);
            return Ok(ClaimOutcome::Expired);
        }
        let Some(approval) = stored.approval.clone() else {
            return Ok(ClaimOutcome::Pending);
        };
        authorizations.remove(&device_digest);
        Ok(ClaimOutcome::Approved(approval))
    }
}

#[derive(Clone)]
struct PgDeviceAuthorizationStore {
    pool: PgPool,
    runtime: Handle,
}

impl PgDeviceAuthorizationStore {
    fn new(pool: PgPool, runtime: Handle) -> Self {
        Self { pool, runtime }
    }

    fn drive<F, T>(&self, future: F) -> Result<T, DeviceAuthorizationStoreError>
    where
        F: Future<Output = Result<T, DeviceAuthorizationStoreError>>,
    {
        match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.runtime.block_on(future))
            }
            Ok(_) => Err(DeviceAuthorizationStoreError(
                "device authorization requires the Edge multi-thread runtime".into(),
            )),
            Err(_) => self.runtime.block_on(future),
        }
    }

    async fn issue_async(
        &self,
        authorization: PendingAuthorization,
        now_unix: i64,
    ) -> Result<bool, DeviceAuthorizationStoreError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(DeviceAuthorizationStoreError::database)?;
        // This table is a deliberately tenant-neutral, opaque login rendezvous. Expiry cleanup is
        // cross-scope but exposes no application data and is bounded to this one ephemeral table.
        sqlx::query("DELETE FROM auth_device_authorization WHERE expires_at_unix <= $1")
            .bind(now_unix)
            .execute(&mut *transaction)
            .await
            .map_err(DeviceAuthorizationStoreError::database)?;
        let inserted = sqlx::query(
            "INSERT INTO auth_device_authorization \
             (device_digest, user_code_digest, verifier_challenge, expires_at_unix) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(authorization.device_digest.as_slice())
        .bind(authorization.user_code_digest.as_slice())
        .bind(authorization.verifier_challenge.as_slice())
        .bind(authorization.expires_at_unix)
        .execute(&mut *transaction)
        .await
        .map_err(DeviceAuthorizationStoreError::database)?
        .rows_affected()
            == 1;
        transaction
            .commit()
            .await
            .map_err(DeviceAuthorizationStoreError::database)?;
        Ok(inserted)
    }

    async fn approve_async(
        &self,
        user_code_digest: [u8; 32],
        approval: DeviceApproval,
        now_unix: i64,
    ) -> Result<ApprovalOutcome, DeviceAuthorizationStoreError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(DeviceAuthorizationStoreError::database)?;
        let row = sqlx::query(
            "SELECT device_digest, expires_at_unix, approved_principal, approved_authority, \
                    source_expires_unix \
               FROM auth_device_authorization \
              WHERE user_code_digest = $1 FOR UPDATE",
        )
        .bind(user_code_digest.as_slice())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DeviceAuthorizationStoreError::database)?;
        let Some(row) = row else {
            transaction
                .commit()
                .await
                .map_err(DeviceAuthorizationStoreError::database)?;
            return Ok(ApprovalOutcome::NotFound);
        };
        let device_digest: Vec<u8> = row.get("device_digest");
        let expires_at_unix: i64 = row.get("expires_at_unix");
        if expires_at_unix <= now_unix {
            sqlx::query("DELETE FROM auth_device_authorization WHERE device_digest = $1")
                .bind(&device_digest)
                .execute(&mut *transaction)
                .await
                .map_err(DeviceAuthorizationStoreError::database)?;
            transaction
                .commit()
                .await
                .map_err(DeviceAuthorizationStoreError::database)?;
            return Ok(ApprovalOutcome::Expired);
        }
        let existing_principal: Option<serde_json::Value> = row.get("approved_principal");
        if let Some(existing_principal) = existing_principal {
            let existing_authority: Option<serde_json::Value> = row.get("approved_authority");
            let existing_expiry: Option<i64> = row.get("source_expires_unix");
            let existing = approval_from_json(
                existing_principal,
                existing_authority.ok_or_else(|| {
                    DeviceAuthorizationStoreError::corrupt("approval authority is missing")
                })?,
                existing_expiry.ok_or_else(|| {
                    DeviceAuthorizationStoreError::corrupt("approval expiry is missing")
                })?,
            )?;
            transaction
                .commit()
                .await
                .map_err(DeviceAuthorizationStoreError::database)?;
            return Ok(if existing == approval {
                ApprovalOutcome::AlreadyApproved
            } else {
                ApprovalOutcome::NotFound
            });
        }
        let principal = serde_json::to_value(&approval.principal)
            .map_err(|error| DeviceAuthorizationStoreError::corrupt(error.to_string()))?;
        let authority = serde_json::to_value(&approval.authority)
            .map_err(|error| DeviceAuthorizationStoreError::corrupt(error.to_string()))?;
        sqlx::query(
            "UPDATE auth_device_authorization \
                SET approved_at_unix = $2, approved_principal = $3, approved_authority = $4, \
                    source_expires_unix = $5 \
              WHERE device_digest = $1",
        )
        .bind(&device_digest)
        .bind(now_unix)
        .bind(principal)
        .bind(authority)
        .bind(approval.source_expires_at_unix)
        .execute(&mut *transaction)
        .await
        .map_err(DeviceAuthorizationStoreError::database)?;
        transaction
            .commit()
            .await
            .map_err(DeviceAuthorizationStoreError::database)?;
        Ok(ApprovalOutcome::Approved)
    }

    async fn claim_async(
        &self,
        device_digest: [u8; 32],
        verifier_challenge: [u8; 32],
        now_unix: i64,
    ) -> Result<ClaimOutcome, DeviceAuthorizationStoreError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(DeviceAuthorizationStoreError::database)?;
        let row = sqlx::query(
            "SELECT verifier_challenge, expires_at_unix, approved_principal, approved_authority, \
                    source_expires_unix \
               FROM auth_device_authorization WHERE device_digest = $1 FOR UPDATE",
        )
        .bind(device_digest.as_slice())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DeviceAuthorizationStoreError::database)?;
        let Some(row) = row else {
            transaction
                .commit()
                .await
                .map_err(DeviceAuthorizationStoreError::database)?;
            return Ok(ClaimOutcome::Invalid);
        };
        let stored_challenge: Vec<u8> = row.get("verifier_challenge");
        if stored_challenge.as_slice() != verifier_challenge {
            transaction
                .commit()
                .await
                .map_err(DeviceAuthorizationStoreError::database)?;
            return Ok(ClaimOutcome::Invalid);
        }
        let expires_at_unix: i64 = row.get("expires_at_unix");
        if expires_at_unix <= now_unix {
            sqlx::query("DELETE FROM auth_device_authorization WHERE device_digest = $1")
                .bind(device_digest.as_slice())
                .execute(&mut *transaction)
                .await
                .map_err(DeviceAuthorizationStoreError::database)?;
            transaction
                .commit()
                .await
                .map_err(DeviceAuthorizationStoreError::database)?;
            return Ok(ClaimOutcome::Expired);
        }
        let principal: Option<serde_json::Value> = row.get("approved_principal");
        let Some(principal) = principal else {
            transaction
                .commit()
                .await
                .map_err(DeviceAuthorizationStoreError::database)?;
            return Ok(ClaimOutcome::Pending);
        };
        let authority: Option<serde_json::Value> = row.get("approved_authority");
        let source_expiry: Option<i64> = row.get("source_expires_unix");
        let approval = approval_from_json(
            principal,
            authority.ok_or_else(|| {
                DeviceAuthorizationStoreError::corrupt("approval authority is missing")
            })?,
            source_expiry.ok_or_else(|| {
                DeviceAuthorizationStoreError::corrupt("approval expiry is missing")
            })?,
        )?;
        sqlx::query("DELETE FROM auth_device_authorization WHERE device_digest = $1")
            .bind(device_digest.as_slice())
            .execute(&mut *transaction)
            .await
            .map_err(DeviceAuthorizationStoreError::database)?;
        transaction
            .commit()
            .await
            .map_err(DeviceAuthorizationStoreError::database)?;
        Ok(ClaimOutcome::Approved(approval))
    }
}

impl DeviceAuthorizationStore for PgDeviceAuthorizationStore {
    fn issue(
        &self,
        authorization: PendingAuthorization,
        now_unix: i64,
    ) -> Result<bool, DeviceAuthorizationStoreError> {
        self.drive(self.issue_async(authorization, now_unix))
    }

    fn approve(
        &self,
        user_code_digest: [u8; 32],
        approval: DeviceApproval,
        now_unix: i64,
    ) -> Result<ApprovalOutcome, DeviceAuthorizationStoreError> {
        self.drive(self.approve_async(user_code_digest, approval, now_unix))
    }

    fn claim(
        &self,
        device_digest: [u8; 32],
        verifier_challenge: [u8; 32],
        now_unix: i64,
    ) -> Result<ClaimOutcome, DeviceAuthorizationStoreError> {
        self.drive(self.claim_async(device_digest, verifier_challenge, now_unix))
    }
}

fn approval_from_json(
    principal: serde_json::Value,
    authority: serde_json::Value,
    source_expires_at_unix: i64,
) -> Result<DeviceApproval, DeviceAuthorizationStoreError> {
    let principal = serde_json::from_value(principal)
        .map_err(|error| DeviceAuthorizationStoreError::corrupt(error.to_string()))?;
    let authority = serde_json::from_value(authority)
        .map_err(|error| DeviceAuthorizationStoreError::corrupt(error.to_string()))?;
    let approval = DeviceApproval {
        principal,
        authority,
        source_expires_at_unix,
    };
    validate_approval(&approval)?;
    Ok(approval)
}

fn validate_approval(approval: &DeviceApproval) -> Result<(), DeviceAuthorizationStoreError> {
    let principal = &approval.principal;
    if !matches!(principal.kind, myelin_identity::PrincipalKind::Human)
        || principal.status != myelin_identity::PrincipalStatus::Active
        || !bounded_identity_field(&principal.tenant.0, 128)
        || !bounded_identity_field(&principal.region.0, 128)
        || !bounded_identity_field(&principal.principal_id.0, 512)
        || approval.source_expires_at_unix <= 0
        || approval.authority.is_empty()
        || approval.authority.len() > 256
    {
        return Err(DeviceAuthorizationStoreError::corrupt(
            "approval identity or authority is outside its bounds",
        ));
    }
    let mut unique = BTreeSet::new();
    if approval
        .authority
        .iter()
        .any(|grant| !bounded_identity_field(grant, 128) || !unique.insert(grant.as_str()))
    {
        return Err(DeviceAuthorizationStoreError::corrupt(
            "approval authority contains an invalid or duplicate grant",
        ));
    }
    Ok(())
}

fn bounded_identity_field(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

#[derive(Clone)]
pub struct DeviceAuthorizationBroker {
    store: Arc<dyn DeviceAuthorizationStore>,
    verification_uri: String,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl DeviceAuthorizationBroker {
    #[cfg(test)]
    pub(crate) fn memory(verification_uri: impl Into<String>) -> Result<Self, String> {
        Self::new(
            Arc::new(MemoryDeviceAuthorizationStore::default()),
            verification_uri,
        )
    }

    pub fn with_pg(
        pool: PgPool,
        runtime: Handle,
        verification_uri: impl Into<String>,
    ) -> Result<Self, String> {
        Self::new(
            Arc::new(PgDeviceAuthorizationStore::new(pool, runtime)),
            verification_uri,
        )
    }

    fn new(
        store: Arc<dyn DeviceAuthorizationStore>,
        verification_uri: impl Into<String>,
    ) -> Result<Self, String> {
        let verification_uri = validate_verification_uri(&verification_uri.into())?;
        Ok(Self {
            store,
            verification_uri,
            now: Arc::new(system_now_unix),
        })
    }

    #[cfg(test)]
    fn with_clock(mut self, now: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        self.now = Arc::new(now);
        self
    }

    pub(crate) fn begin(
        &self,
        verifier_challenge: &str,
    ) -> Result<DeviceAuthorizationStart, DeviceAuthorizationError> {
        let verifier_challenge =
            decode_secret(verifier_challenge).ok_or(DeviceAuthorizationError::InvalidInput(
                "the verifier challenge is not a canonical S256 value",
            ))?;
        let now_unix = (self.now)();
        for _ in 0..4 {
            let device_code = random_secret();
            let user_code = random_user_code();
            let pending = PendingAuthorization {
                device_digest: digest("device", &device_code),
                user_code_digest: digest("user", &user_code),
                verifier_challenge,
                expires_at_unix: now_unix.saturating_add(DEVICE_AUTHORIZATION_TTL_SECS),
            };
            if self
                .store
                .issue(pending, now_unix)
                .map_err(DeviceAuthorizationError::Store)?
            {
                return Ok(DeviceAuthorizationStart {
                    device_code,
                    user_code: user_code.clone(),
                    verification_uri: self.verification_uri.clone(),
                    verification_uri_complete: format!(
                        "{}?code={user_code}",
                        self.verification_uri
                    ),
                    expires_in: DEVICE_AUTHORIZATION_TTL_SECS as u64,
                    interval: DEVICE_AUTHORIZATION_POLL_INTERVAL_SECS,
                });
            }
        }
        Err(DeviceAuthorizationError::Store(
            DeviceAuthorizationStoreError(
                "could not allocate a unique device authorization".into(),
            ),
        ))
    }

    pub(crate) fn approve(
        &self,
        user_code: &str,
        approval: DeviceApproval,
    ) -> Result<ApprovalOutcome, DeviceAuthorizationError> {
        let user_code = canonical_user_code(user_code).ok_or(
            DeviceAuthorizationError::InvalidInput("the user code is malformed"),
        )?;
        validate_approval(&approval).map_err(DeviceAuthorizationError::Store)?;
        self.store
            .approve(digest("user", &user_code), approval, (self.now)())
            .map_err(DeviceAuthorizationError::Store)
    }

    pub(crate) fn claim(
        &self,
        device_code: &str,
        verifier: &str,
    ) -> Result<ClaimOutcome, DeviceAuthorizationError> {
        if decode_secret(device_code).is_none() || decode_secret(verifier).is_none() {
            return Ok(ClaimOutcome::Invalid);
        }
        let verifier_challenge = sha256(verifier.as_bytes());
        self.store
            .claim(
                digest("device", device_code),
                verifier_challenge,
                (self.now)(),
            )
            .map_err(DeviceAuthorizationError::Store)
    }
}

fn validate_verification_uri(value: &str) -> Result<String, String> {
    let uri = value
        .parse::<hyper::Uri>()
        .map_err(|_| "device verification URI must be a valid absolute HTTP(S) URL".to_string())?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.authority().is_none()
        || uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@'))
        || uri.query().is_some()
        || uri.path().is_empty()
        || uri.path() == "/"
    {
        return Err(
            "device verification URI must be an absolute, credential-free, query-free HTTP(S) URL with a path"
                .into(),
        );
    }
    Ok(value.trim_end_matches('/').to_string())
}

fn system_now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn random_secret() -> String {
    let mut bytes = [0_u8; SECRET_BYTES];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_user_code() -> String {
    let mut random = [0_u8; 8];
    OsRng.fill_bytes(&mut random);
    let symbols: String = random
        .iter()
        .map(|byte| USER_CODE_ALPHABET[usize::from(*byte & 31)] as char)
        .collect();
    format!("{}-{}", &symbols[..4], &symbols[4..])
}

fn canonical_user_code(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_uppercase();
    let symbols = match value.as_bytes() {
        [a, b, c, d, b'-', e, f, g, h] => [*a, *b, *c, *d, *e, *f, *g, *h],
        [a, b, c, d, e, f, g, h] => [*a, *b, *c, *d, *e, *f, *g, *h],
        _ => return None,
    };
    if !symbols
        .iter()
        .all(|symbol| USER_CODE_ALPHABET.contains(symbol))
    {
        return None;
    }
    let left = std::str::from_utf8(&symbols[..4]).ok()?;
    let right = std::str::from_utf8(&symbols[4..]).ok()?;
    Some(format!("{left}-{right}"))
}

fn decode_secret(value: &str) -> Option<[u8; SECRET_BYTES]> {
    if value.len() != SECRET_B64URL_LEN
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return None;
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .ok()?;
    let bytes: [u8; SECRET_BYTES] = decoded.try_into().ok()?;
    (base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes) == value).then_some(bytes)
}

fn sha256(value: &[u8]) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(value).into()
}

fn digest(namespace: &str, value: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"myelin.edge.device-authorization.v1\0");
    hasher.update(namespace.as_bytes());
    hasher.update(b"\0");
    hasher.update(value.as_bytes());
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_tenancy::{Region, TenantId};

    fn approval() -> DeviceApproval {
        DeviceApproval {
            principal: Principal::new(
                TenantId("acme".into()),
                Region("eu-west".into()),
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
            ),
            authority: vec!["repo.pull".into()],
            source_expires_at_unix: 1_900_000_000,
        }
    }

    fn verifier() -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 32])
    }

    fn challenge(verifier: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha256(verifier.as_bytes()))
    }

    #[test]
    fn a_human_approval_is_claimed_once_by_the_cli_that_holds_the_verifier() {
        let broker = DeviceAuthorizationBroker::memory("https://myelin.example/cli/auth")
            .unwrap()
            .with_clock(|| 1_800_000_000);
        let verifier = verifier();
        let started = broker.begin(&challenge(&verifier)).unwrap();

        assert_eq!(
            broker.claim(&started.device_code, &verifier).unwrap(),
            ClaimOutcome::Pending
        );
        assert_eq!(
            broker
                .approve(&started.user_code.to_lowercase(), approval())
                .unwrap(),
            ApprovalOutcome::Approved
        );
        assert_eq!(
            broker.claim(&started.device_code, &verifier).unwrap(),
            ClaimOutcome::Approved(approval())
        );
        assert_eq!(
            broker.claim(&started.device_code, &verifier).unwrap(),
            ClaimOutcome::Invalid,
            "the approved identity is consumed atomically"
        );
    }

    #[test]
    fn another_cli_cannot_claim_an_approved_identity() {
        let broker = DeviceAuthorizationBroker::memory("https://myelin.example/cli/auth")
            .unwrap()
            .with_clock(|| 1_800_000_000);
        let verifier = verifier();
        let started = broker.begin(&challenge(&verifier)).unwrap();
        broker.approve(&started.user_code, approval()).unwrap();
        let attacker_verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([8_u8; 32]);

        assert_eq!(
            broker
                .claim(&started.device_code, &attacker_verifier)
                .unwrap(),
            ClaimOutcome::Invalid
        );
        assert_eq!(
            broker.claim(&started.device_code, &verifier).unwrap(),
            ClaimOutcome::Approved(approval()),
            "a wrong verifier does not consume the authorization"
        );
    }

    #[test]
    fn verification_urls_are_bounded_and_user_codes_are_unambiguous() {
        assert!(DeviceAuthorizationBroker::memory("javascript:alert(1)").is_err());
        assert!(DeviceAuthorizationBroker::memory("https://user@example.com/cli/auth").is_err());
        assert!(DeviceAuthorizationBroker::memory("https://example.com/cli/auth?next=x").is_err());
        assert_eq!(canonical_user_code("abcd-efgh"), Some("ABCD-EFGH".into()));
        assert_eq!(
            canonical_user_code("ABCI-EFGH"),
            None,
            "I is intentionally excluded"
        );
    }
}
