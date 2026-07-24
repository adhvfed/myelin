//! Dedicated elected publisher for the shared PostgreSQL outbox.
//!
//! This is a terminal service leaf. Its database provider exposes only an elected relay, its NATS
//! runtime adapter publishes only to an already-provisioned stream, and its health surface is a
//! typed in-process snapshot. A real HTTP listener is intentionally a separate follow-up.

use std::future::Future;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use myelin_events::nats::JetStreamPublisherConfig;
use myelin_events::relay::EventPublisher;
use myelin_storage::elected_relay::{ElectedDrainOutcome, ElectedPgRelay, ElectedRelayError};
use myelin_storage::pgrelay::RelayValidationConfig;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};

pub const EVENT_STREAM_NAME: &str = "MYELIN_EVENTS";
pub const EVENT_SUBJECT_ROOT: &str = "myelin.events";
pub const PUBLISHER_DATABASE_URL_ENV: &str = "MYELIN_OUTBOX_PUBLISHER_DATABASE_URL";
pub const PROVISION_NATS_URL_ENV: &str = "MYELIN_OUTBOX_PROVISION_NATS_URL";
pub const PUBLISH_NATS_URL_ENV: &str = "MYELIN_OUTBOX_PUBLISH_NATS_URL";
pub const PASS_TIMEOUT_ENV: &str = "MYELIN_OUTBOX_PUBLISHER_PASS_TIMEOUT_MS";

const CAPABILITY_ROLE: &str = "myelin_outbox_publisher";

#[derive(Clone)]
struct PublisherDatabaseConfig {
    options: PgConnectOptions,
    database: String,
    publisher_role: String,
    runtime_role: String,
}

impl core::fmt::Debug for PublisherDatabaseConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PublisherDatabaseConfig")
            .field("authority", &"<redacted>")
            .finish()
    }
}

/// Publisher-local bounded configuration. Debug output never renders either NATS authority or a
/// PostgreSQL credential.
#[derive(Clone)]
pub struct PublisherConfig {
    database: PublisherDatabaseConfig,
    provision_nats_url: String,
    publish_nats_url: String,
    region: String,
    batch: i64,
    poll: Duration,
    backoff: Duration,
    statement_timeout: Duration,
    pass_timeout: Duration,
    max_envelope_bytes: usize,
    stream_max_age: Duration,
    stream_max_bytes: i64,
    stream_max_messages: i64,
    stream_replicas: usize,
    duplicate_window: Duration,
    publish_ack_timeout: Duration,
}

impl core::fmt::Debug for PublisherConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PublisherConfig")
            .field("database", &self.database)
            .field("provision_nats_url", &"<redacted>")
            .field("publish_nats_url", &"<redacted>")
            .field("region", &self.region)
            .field("batch", &self.batch)
            .field("poll", &self.poll)
            .field("backoff", &self.backoff)
            .field("statement_timeout", &self.statement_timeout)
            .field("pass_timeout", &self.pass_timeout)
            .field("max_envelope_bytes", &self.max_envelope_bytes)
            .field("stream_max_age", &self.stream_max_age)
            .field("stream_max_bytes", &self.stream_max_bytes)
            .field("stream_max_messages", &self.stream_max_messages)
            .field("stream_replicas", &self.stream_replicas)
            .field("duplicate_window", &self.duplicate_window)
            .field("publish_ack_timeout", &self.publish_ack_timeout)
            .finish()
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublisherConfigError {
    Missing,
    NonUnicode,
    InvalidDatabaseAuthority,
    DatabaseCredentialNotDistinct,
    DatabaseServerIdentityMismatch,
    EmptyValue,
    InvalidNumber,
    OutOfBounds,
    InvalidStreamPolicy,
    PassBudgetInfeasible,
}

impl core::fmt::Display for PublisherConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let message = match self {
            Self::Missing => "required outbox publisher configuration is missing",
            Self::NonUnicode => "outbox publisher configuration contains non-UTF-8 bytes",
            Self::InvalidDatabaseAuthority => "outbox publisher database authority is invalid",
            Self::DatabaseCredentialNotDistinct => {
                "outbox publisher database credential is not distinct from the application credential"
            }
            Self::DatabaseServerIdentityMismatch => {
                "outbox publisher and application credentials target different database identities"
            }
            Self::EmptyValue => "outbox publisher configuration contains an empty value",
            Self::InvalidNumber => "outbox publisher configuration contains an invalid number",
            Self::OutOfBounds => "outbox publisher configuration exceeds a bounded limit",
            Self::InvalidStreamPolicy => "outbox publisher stream policy is invalid",
            Self::PassBudgetInfeasible => {
                "outbox publisher pass budget cannot fit its conservative database and acknowledgement budget"
            }
        };
        f.write_str(message)
    }
}

impl std::error::Error for PublisherConfigError {}

impl PublisherConfig {
    pub fn from_env() -> Result<Self, PublisherConfigError> {
        Self::from_getter(|name| {
            std::env::var_os(name)
                .ok_or(PublisherConfigError::Missing)?
                .into_string()
                .map_err(|_| PublisherConfigError::NonUnicode)
        })
    }

    fn from_getter(
        get: impl Fn(&str) -> Result<String, PublisherConfigError>,
    ) -> Result<Self, PublisherConfigError> {
        let publisher_url = get(PUBLISHER_DATABASE_URL_ENV)?;
        // Parsed only for credential/server-identity comparison. The app DSN is never retained,
        // connected, or exposed through the publisher provider.
        let runtime_url = get("DATABASE_URL")?;
        let publisher = PgConnectOptions::from_str(&publisher_url)
            .map_err(|_| PublisherConfigError::InvalidDatabaseAuthority)?;
        let runtime = PgConnectOptions::from_str(&runtime_url)
            .map_err(|_| PublisherConfigError::InvalidDatabaseAuthority)?;
        let publisher_database = publisher
            .get_database()
            .unwrap_or(publisher.get_username())
            .to_owned();
        let runtime_database = runtime
            .get_database()
            .unwrap_or(runtime.get_username())
            .to_owned();
        if publisher_url == runtime_url || publisher.get_username() == runtime.get_username() {
            return Err(PublisherConfigError::DatabaseCredentialNotDistinct);
        }
        if publisher.get_host() != runtime.get_host()
            || publisher.get_port() != runtime.get_port()
            || publisher_database != runtime_database
        {
            return Err(PublisherConfigError::DatabaseServerIdentityMismatch);
        }
        let publisher_role = publisher.get_username().to_owned();

        let provision_nats_url = required(&get, PROVISION_NATS_URL_ENV)?;
        let publish_nats_url = required(&get, PUBLISH_NATS_URL_ENV)?;
        let region = required(&get, "MYELIN_REGION")?;
        let batch = bounded_i64(&get, "MYELIN_OUTBOX_PUBLISHER_BATCH", 1, 1000)?;
        let poll = millis(&get, "MYELIN_OUTBOX_PUBLISHER_POLL_MS", 10, 60_000)?;
        let backoff = millis(&get, "MYELIN_OUTBOX_PUBLISHER_BACKOFF_MS", 10, 60_000)?;
        let statement_timeout = millis(
            &get,
            "MYELIN_OUTBOX_PUBLISHER_STATEMENT_TIMEOUT_MS",
            100,
            60_000,
        )?;
        let max_envelope_bytes = bounded_u64(
            &get,
            "MYELIN_OUTBOX_PUBLISHER_MAX_ENVELOPE_BYTES",
            1,
            16 * 1024 * 1024,
        )? as usize;
        let stream_max_age = seconds(
            &get,
            "MYELIN_OUTBOX_STREAM_MAX_AGE_SECONDS",
            1,
            365 * 24 * 60 * 60,
        )?;
        let stream_max_bytes = bounded_i64(&get, "MYELIN_OUTBOX_STREAM_MAX_BYTES", 1, i64::MAX)?;
        let stream_max_messages =
            bounded_i64(&get, "MYELIN_OUTBOX_STREAM_MAX_MESSAGES", 1, i64::MAX)?;
        let stream_replicas = bounded_u64(&get, "MYELIN_OUTBOX_STREAM_REPLICAS", 1, 5)? as usize;
        let duplicate_window = seconds(
            &get,
            "MYELIN_OUTBOX_STREAM_DEDUP_SECONDS",
            1,
            stream_max_age.as_secs(),
        )?;
        let publish_ack_timeout = millis(&get, "MYELIN_OUTBOX_PUBLISH_ACK_TIMEOUT_MS", 1, 60_000)?;
        let pass_timeout = millis(&get, PASS_TIMEOUT_ENV, 100, 5 * 60 * 1000)?;
        let batch_count = u32::try_from(batch).map_err(|_| PublisherConfigError::OutOfBounds)?;
        let ack_budget = publish_ack_timeout
            .checked_mul(batch_count)
            .ok_or(PublisherConfigError::PassBudgetInfeasible)?;
        // Conservative feasibility model: election + claim + commit, plus one quarantine/update
        // statement per claimed row, and one full publish-ack timeout per row. The outer runtime
        // timeout remains the absolute bound even when an operation finishes outside this model.
        let database_budget = statement_timeout
            .checked_mul(batch_count.saturating_add(3))
            .ok_or(PublisherConfigError::PassBudgetInfeasible)?;
        let required_pass_budget = database_budget
            .checked_add(ack_budget)
            .ok_or(PublisherConfigError::PassBudgetInfeasible)?;
        if required_pass_budget > pass_timeout {
            return Err(PublisherConfigError::PassBudgetInfeasible);
        }

        let config = Self {
            database: PublisherDatabaseConfig {
                options: publisher,
                database: publisher_database,
                publisher_role,
                runtime_role: runtime.get_username().to_owned(),
            },
            provision_nats_url,
            publish_nats_url,
            region,
            batch,
            poll,
            backoff,
            statement_timeout,
            pass_timeout,
            max_envelope_bytes,
            stream_max_age,
            stream_max_bytes,
            stream_max_messages,
            stream_replicas,
            duplicate_window,
            publish_ack_timeout,
        };
        config
            .publish_nats_config()
            .validate()
            .map_err(|_| PublisherConfigError::InvalidStreamPolicy)?;
        Ok(config)
    }

    pub fn provision_nats_config(&self) -> JetStreamPublisherConfig {
        self.nats_config(self.provision_nats_url.clone())
    }

    pub fn publish_nats_config(&self) -> JetStreamPublisherConfig {
        self.nats_config(self.publish_nats_url.clone())
    }

    fn nats_config(&self, nats_url: String) -> JetStreamPublisherConfig {
        JetStreamPublisherConfig {
            nats_url,
            stream_name: EVENT_STREAM_NAME.into(),
            subject_root: EVENT_SUBJECT_ROOT.into(),
            max_age: self.stream_max_age,
            max_bytes: self.stream_max_bytes,
            max_messages: self.stream_max_messages,
            replicas: self.stream_replicas,
            duplicate_window: self.duplicate_window,
            publish_ack_timeout: self.publish_ack_timeout,
        }
    }

    pub fn batch(&self) -> i64 {
        self.batch
    }
    pub fn poll(&self) -> Duration {
        self.poll
    }
    pub fn backoff(&self) -> Duration {
        self.backoff
    }
    pub fn region(&self) -> &str {
        &self.region
    }
    pub fn max_envelope_bytes(&self) -> usize {
        self.max_envelope_bytes
    }
    pub fn pass_timeout(&self) -> Duration {
        self.pass_timeout
    }
}

fn required(
    get: &impl Fn(&str) -> Result<String, PublisherConfigError>,
    name: &str,
) -> Result<String, PublisherConfigError> {
    let value = get(name)?;
    if value.trim().is_empty() {
        Err(PublisherConfigError::EmptyValue)
    } else {
        Ok(value)
    }
}

fn bounded_u64(
    get: &impl Fn(&str) -> Result<String, PublisherConfigError>,
    name: &str,
    min: u64,
    max: u64,
) -> Result<u64, PublisherConfigError> {
    let value = required(get, name)?
        .parse::<u64>()
        .map_err(|_| PublisherConfigError::InvalidNumber)?;
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(PublisherConfigError::OutOfBounds)
    }
}

fn bounded_i64(
    get: &impl Fn(&str) -> Result<String, PublisherConfigError>,
    name: &str,
    min: i64,
    max: i64,
) -> Result<i64, PublisherConfigError> {
    let value = required(get, name)?
        .parse::<i64>()
        .map_err(|_| PublisherConfigError::InvalidNumber)?;
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(PublisherConfigError::OutOfBounds)
    }
}

fn millis(
    get: &impl Fn(&str) -> Result<String, PublisherConfigError>,
    name: &str,
    min: u64,
    max: u64,
) -> Result<Duration, PublisherConfigError> {
    bounded_u64(get, name, min, max).map(Duration::from_millis)
}

fn seconds(
    get: &impl Fn(&str) -> Result<String, PublisherConfigError>,
    name: &str,
    min: u64,
    max: u64,
) -> Result<Duration, PublisherConfigError> {
    bounded_u64(get, name, min, max).map(Duration::from_secs)
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublisherDbError {
    ConnectionFailed,
    ProbeFailed,
    DatabaseIdentityMismatch,
    RolesNotDistinct,
    LoginRequired,
    IdentityChanged,
    Superuser,
    BypassRls,
    CreateDatabase,
    CreateRole,
    DatabaseCreate,
    SchemaUsageMissing,
    SchemaCreate,
    SchemaOwnerMembership,
    CapabilityMembershipMissing,
    CapabilityNotInherited,
    CapabilityCanSetRole,
    UnexpectedMembership,
    InsufficientPrivileges,
    ExcessPrivileges,
}

impl core::fmt::Display for PublisherDbError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let message = match self {
            Self::ConnectionFailed => "outbox publisher database connection failed",
            Self::ProbeFailed => "outbox publisher database authorization probe failed",
            Self::DatabaseIdentityMismatch => "outbox publisher database identity mismatch",
            Self::RolesNotDistinct => "outbox publisher database role is not distinct",
            Self::LoginRequired => "outbox publisher credential is not a login role",
            Self::IdentityChanged => "outbox publisher database identity changed",
            Self::Superuser => "outbox publisher database role is a superuser",
            Self::BypassRls => "outbox publisher database role can bypass row-level security",
            Self::CreateDatabase => "outbox publisher database role can create databases",
            Self::CreateRole => "outbox publisher database role can create roles",
            Self::DatabaseCreate => "outbox publisher database role can create schemas",
            Self::SchemaUsageMissing => "outbox publisher cannot use the application schema",
            Self::SchemaCreate => "outbox publisher can create application schema objects",
            Self::SchemaOwnerMembership => "outbox publisher is a member of the schema owner",
            Self::CapabilityMembershipMissing => {
                "outbox publisher capability membership is missing"
            }
            Self::CapabilityNotInherited => "outbox publisher capability is not inherited",
            Self::CapabilityCanSetRole => "outbox publisher can assume its capability role",
            Self::UnexpectedMembership => "outbox publisher has unexpected role membership",
            Self::InsufficientPrivileges => "outbox publisher lacks required relay privileges",
            Self::ExcessPrivileges => "outbox publisher has privileges outside the relay boundary",
        };
        f.write_str(message)
    }
}

impl std::error::Error for PublisherDbError {}

/// Validated one-connection provider that yields only an elected relay capability.
pub struct PublisherDbProvider {
    pool: PgPool,
}

impl PublisherDbProvider {
    pub async fn connect(config: &PublisherConfig) -> Result<Self, PublisherDbError> {
        let timeout_ms = config.statement_timeout.as_millis().to_string();
        let options = config
            .database
            .options
            .clone()
            .application_name("myelin:outbox-publisher")
            .options([("statement_timeout", timeout_ms.as_str())]);
        // @residency-cell-pinned: this cell-local authority is required to match the application
        // database identity; relay validation separately pins every envelope to `config.region`.
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(config.statement_timeout)
            .connect_with(options)
            .await
            .map_err(|_| PublisherDbError::ConnectionFailed)?;
        if let Err(error) = validate_publisher_pool(&pool, &config.database).await {
            pool.close().await;
            return Err(error);
        }
        Ok(Self { pool })
    }

    pub fn elected_relay(
        &self,
        region: &str,
        max_envelope_bytes: usize,
    ) -> Result<ElectedPgRelay, PublisherConfigError> {
        let validation =
            RelayValidationConfig::new(myelin_events::Region(region.into()), max_envelope_bytes)
                .map_err(|_| PublisherConfigError::OutOfBounds)?;
        ElectedPgRelay::new(self.pool.clone(), validation)
            .map_err(|_| PublisherConfigError::OutOfBounds)
    }
}

async fn validate_publisher_pool(
    pool: &PgPool,
    expected: &PublisherDatabaseConfig,
) -> Result<(), PublisherDbError> {
    // @tenant-cross-scope: this authorization probe reads only PostgreSQL identity/catalog grants
    // and named outbox capabilities; it never reads tenant-owned rows.
    let row = sqlx::query(
        "SELECT current_database()::text AS database,
                session_user::text AS session_user,
                current_user::text AS current_user,
                login.rolcanlogin AS login,
                login.rolinherit AS inherit,
                login.rolsuper AS superuser,
                login.rolbypassrls AS bypass_rls,
                login.rolcreatedb AS create_database,
                login.rolcreaterole AS create_role,
                pg_catalog.has_database_privilege(session_user, current_database(), 'CREATE') AS database_create,
                pg_catalog.has_schema_privilege(session_user, 'public', 'USAGE') AS schema_usage,
                pg_catalog.has_schema_privilege(session_user, 'public', 'CREATE') AS schema_create,
                pg_catalog.pg_has_role(session_user,
                  (SELECT nspowner FROM pg_catalog.pg_namespace WHERE nspname = 'public'), 'MEMBER') AS schema_owner_member,
                pg_catalog.pg_has_role(session_user, $1, 'MEMBER') AS capability_member,
                pg_catalog.pg_has_role(session_user, $1, 'USAGE') AS capability_usage,
                COALESCE(membership.inherit_option, false) AS membership_inherit,
                COALESCE(membership.set_option, false) AS membership_set,
                EXISTS (
                  SELECT 1 FROM pg_catalog.pg_roles granted
                   WHERE granted.rolname <> session_user
                     AND granted.rolname <> $1
                     AND pg_catalog.pg_has_role(session_user, granted.oid, 'MEMBER')
                ) AS unexpected_membership,
                pg_catalog.has_table_privilege(session_user, 'public.outbox', 'SELECT') AS outbox_select,
                pg_catalog.has_column_privilege(session_user, 'public.outbox', 'published_at', 'UPDATE') AS published_update,
                pg_catalog.has_table_privilege(session_user, 'public.outbox_quarantine', 'SELECT') AS quarantine_select,
                pg_catalog.has_table_privilege(session_user, 'public.outbox_quarantine', 'INSERT') AS quarantine_insert,
                (
                  pg_catalog.has_table_privilege(session_user, 'public.outbox', 'INSERT')
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox', 'UPDATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox', 'DELETE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox', 'TRIGGER')
                  OR EXISTS (
                    SELECT 1 FROM pg_catalog.pg_attribute column_grant
                     WHERE column_grant.attrelid = 'public.outbox'::regclass
                       AND column_grant.attnum > 0 AND NOT column_grant.attisdropped
                       AND column_grant.attname <> 'published_at'
                       AND pg_catalog.has_column_privilege(
                         session_user, column_grant.attrelid, column_grant.attnum, 'UPDATE')
                  )
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox_quarantine', 'UPDATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox_quarantine', 'DELETE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox_quarantine', 'TRUNCATE')
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox_quarantine', 'REFERENCES')
                  OR pg_catalog.has_table_privilege(session_user, 'public.outbox_quarantine', 'TRIGGER')
                  OR EXISTS (
                    SELECT 1 FROM pg_catalog.pg_class unrelated
                    JOIN pg_catalog.pg_namespace namespace ON namespace.oid = unrelated.relnamespace
                     WHERE unrelated.relkind IN ('r', 'p', 'f', 'v', 'm')
                       AND namespace.nspname NOT IN ('pg_catalog', 'information_schema')
                       AND namespace.nspname NOT LIKE 'pg_toast%'
                       AND unrelated.oid NOT IN ('public.outbox'::regclass, 'public.outbox_quarantine'::regclass)
                       AND (
                         pg_catalog.has_table_privilege(session_user, unrelated.oid, 'SELECT')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'INSERT')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'UPDATE')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'DELETE')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'TRUNCATE')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'REFERENCES')
                         OR pg_catalog.has_table_privilege(session_user, unrelated.oid, 'TRIGGER')
                       )
                  )
                ) AS excess_privilege
           FROM pg_catalog.pg_roles login
           LEFT JOIN (
             SELECT member_role.rolname AS member_name,
                    granted_role.rolname AS granted_name,
                    auth.inherit_option,
                    auth.set_option
               FROM pg_catalog.pg_auth_members auth
               JOIN pg_catalog.pg_roles member_role ON member_role.oid = auth.member
               JOIN pg_catalog.pg_roles granted_role ON granted_role.oid = auth.roleid
           ) membership
             ON membership.member_name = session_user AND membership.granted_name = $1
          WHERE login.rolname = session_user",
    )
    .bind(CAPABILITY_ROLE)
    .fetch_one(pool)
    .await
    .map_err(|_| PublisherDbError::ProbeFailed)?;

    let session_user: String = row.get("session_user");
    if row.get::<String, _>("database") != expected.database {
        return Err(PublisherDbError::DatabaseIdentityMismatch);
    }
    if session_user == expected.runtime_role || session_user != expected.publisher_role {
        return Err(PublisherDbError::RolesNotDistinct);
    }
    if session_user != row.get::<String, _>("current_user") {
        return Err(PublisherDbError::IdentityChanged);
    }
    for (field, error) in [
        ("superuser", PublisherDbError::Superuser),
        ("bypass_rls", PublisherDbError::BypassRls),
        ("create_database", PublisherDbError::CreateDatabase),
        ("create_role", PublisherDbError::CreateRole),
        ("database_create", PublisherDbError::DatabaseCreate),
        ("schema_create", PublisherDbError::SchemaCreate),
        (
            "schema_owner_member",
            PublisherDbError::SchemaOwnerMembership,
        ),
        ("membership_set", PublisherDbError::CapabilityCanSetRole),
        (
            "unexpected_membership",
            PublisherDbError::UnexpectedMembership,
        ),
        ("excess_privilege", PublisherDbError::ExcessPrivileges),
    ] {
        if row.get::<bool, _>(field) {
            return Err(error);
        }
    }
    if !row.get::<bool, _>("login") {
        return Err(PublisherDbError::LoginRequired);
    }
    if !row.get::<bool, _>("schema_usage") {
        return Err(PublisherDbError::SchemaUsageMissing);
    }
    if !row.get::<bool, _>("capability_member") {
        return Err(PublisherDbError::CapabilityMembershipMissing);
    }
    if !row.get::<bool, _>("inherit")
        || !row.get::<bool, _>("capability_usage")
        || !row.get::<bool, _>("membership_inherit")
    {
        return Err(PublisherDbError::CapabilityNotInherited);
    }
    if !(row.get::<bool, _>("outbox_select")
        && row.get::<bool, _>("published_update")
        && row.get::<bool, _>("quarantine_select")
        && row.get::<bool, _>("quarantine_insert"))
    {
        return Err(PublisherDbError::InsufficientPrivileges);
    }
    Ok(())
}

pub struct ElectedPublisher<P> {
    relay: ElectedPgRelay,
    publisher: P,
    batch: i64,
}

impl<P> ElectedPublisher<P> {
    pub fn new(relay: ElectedPgRelay, publisher: P, batch: i64) -> Self {
        Self {
            relay,
            publisher,
            batch,
        }
    }
}

pub trait DrainPass: Send + Sync {
    fn drain_once(
        &self,
    ) -> impl Future<Output = Result<ElectedDrainOutcome, ElectedRelayError>> + Send;
}

impl<P: EventPublisher> DrainPass for ElectedPublisher<P> {
    async fn drain_once(&self) -> Result<ElectedDrainOutcome, ElectedRelayError> {
        self.relay.drain_once(&self.publisher, self.batch).await
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublisherReadiness {
    Ready,
    NotReady,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublisherRuntimeState {
    Starting,
    Standby,
    Publishing,
    Degraded,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublisherSnapshot {
    pub readiness: PublisherReadiness,
    pub state: PublisherRuntimeState,
    pub passes: u64,
    pub published: u64,
}

impl Default for PublisherSnapshot {
    fn default() -> Self {
        Self {
            readiness: PublisherReadiness::NotReady,
            state: PublisherRuntimeState::Starting,
            passes: 0,
            published: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassResult {
    Standby,
    Published(usize),
    Unavailable,
}

pub struct PublisherRuntime<D> {
    drain: D,
    poll: Duration,
    backoff: Duration,
    pass_timeout: Duration,
    snapshot: Arc<Mutex<PublisherSnapshot>>,
}

impl<D: DrainPass> PublisherRuntime<D> {
    pub fn new(drain: D, poll: Duration, backoff: Duration, pass_timeout: Duration) -> Self {
        Self {
            drain,
            poll,
            backoff,
            pass_timeout,
            snapshot: Arc::new(Mutex::new(PublisherSnapshot::default())),
        }
    }

    pub fn snapshot(&self) -> PublisherSnapshot {
        *self
            .snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub async fn run_pass(&self) -> PassResult {
        {
            let mut state = self
                .snapshot
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.state = PublisherRuntimeState::Publishing;
        }
        let result = match tokio::time::timeout(self.pass_timeout, self.drain.drain_once()).await {
            Ok(Ok(ElectedDrainOutcome::Standby)) => PassResult::Standby,
            Ok(Ok(ElectedDrainOutcome::Published(count))) => PassResult::Published(count),
            Ok(Err(_)) | Err(_) => PassResult::Unavailable,
        };
        let mut state = self
            .snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.passes = state.passes.saturating_add(1);
        match result {
            PassResult::Standby => {
                state.readiness = PublisherReadiness::Ready;
                state.state = PublisherRuntimeState::Standby;
            }
            PassResult::Published(count) => {
                state.readiness = PublisherReadiness::Ready;
                state.state = PublisherRuntimeState::Publishing;
                state.published = state.published.saturating_add(count as u64);
            }
            PassResult::Unavailable => {
                state.readiness = PublisherReadiness::NotReady;
                state.state = PublisherRuntimeState::Degraded;
            }
        }
        result
    }

    /// Run bounded passes until shutdown. Shutdown never starts a new pass and never drains to
    /// empty; it waits only for the already-started, batch/statement/ack-bounded pass.
    pub async fn serve_until(&self, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            let result = self.run_pass().await;
            if stop.load(Ordering::SeqCst) {
                break;
            }
            sleep_until_stop(
                stop,
                match result {
                    PassResult::Unavailable => self.backoff,
                    PassResult::Standby | PassResult::Published(_) => self.poll,
                },
            )
            .await;
        }
        let mut state = self
            .snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.readiness = PublisherReadiness::NotReady;
        state.state = PublisherRuntimeState::Stopped;
    }
}

async fn sleep_until_stop(stop: &AtomicBool, duration: Duration) {
    let deadline = tokio::time::Instant::now() + duration;
    while !stop.load(Ordering::SeqCst) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(10))).await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};

    use myelin_storage::pg::PgError;

    use super::*;

    fn values() -> HashMap<&'static str, String> {
        HashMap::from([
            (
                PUBLISHER_DATABASE_URL_ENV,
                "postgres://publisher:SECRET@db:5432/myelin".into(),
            ),
            (
                "DATABASE_URL",
                "postgres://app:APP_SECRET@db:5432/myelin".into(),
            ),
            (
                PROVISION_NATS_URL_ENV,
                "nats://admin:NATS_ADMIN@broker:4222".into(),
            ),
            (
                PUBLISH_NATS_URL_ENV,
                "nats://publisher:NATS_RUNTIME@broker:4222".into(),
            ),
            ("MYELIN_REGION", "fr-par".into()),
            ("MYELIN_OUTBOX_PUBLISHER_BATCH", "4".into()),
            ("MYELIN_OUTBOX_PUBLISHER_POLL_MS", "100".into()),
            ("MYELIN_OUTBOX_PUBLISHER_BACKOFF_MS", "500".into()),
            (
                "MYELIN_OUTBOX_PUBLISHER_STATEMENT_TIMEOUT_MS",
                "1000".into(),
            ),
            (
                "MYELIN_OUTBOX_PUBLISHER_MAX_ENVELOPE_BYTES",
                "262144".into(),
            ),
            ("MYELIN_OUTBOX_STREAM_MAX_AGE_SECONDS", "86400".into()),
            ("MYELIN_OUTBOX_STREAM_MAX_BYTES", "67108864".into()),
            ("MYELIN_OUTBOX_STREAM_MAX_MESSAGES", "100000".into()),
            ("MYELIN_OUTBOX_STREAM_REPLICAS", "1".into()),
            ("MYELIN_OUTBOX_STREAM_DEDUP_SECONDS", "120".into()),
            ("MYELIN_OUTBOX_PUBLISH_ACK_TIMEOUT_MS", "2000".into()),
            (PASS_TIMEOUT_ENV, "20000".into()),
        ])
    }

    fn config(values: &HashMap<&str, String>) -> Result<PublisherConfig, PublisherConfigError> {
        PublisherConfig::from_getter(|name| {
            values
                .get(name)
                .cloned()
                .ok_or(PublisherConfigError::Missing)
        })
    }

    #[test]
    fn config_is_bounded_canonical_and_redacted() {
        let mut bounded = values();
        let cfg = config(&bounded).unwrap();
        assert_eq!(cfg.publish_nats_config().stream_name, EVENT_STREAM_NAME);
        assert_eq!(cfg.publish_nats_config().subject_root, EVENT_SUBJECT_ROOT);
        assert_eq!(cfg.pass_timeout(), Duration::from_secs(20));
        let debug = format!("{cfg:?}");
        for sentinel in [
            "SECRET",
            "APP_SECRET",
            "NATS_ADMIN",
            "NATS_RUNTIME",
            "postgres://",
            "nats://",
        ] {
            assert!(!debug.contains(sentinel));
        }

        bounded.insert("MYELIN_OUTBOX_PUBLISHER_BATCH", "0".into());
        assert!(matches!(
            config(&bounded),
            Err(PublisherConfigError::OutOfBounds)
        ));

        let mut impossible = values();
        impossible.insert("MYELIN_OUTBOX_PUBLISHER_BATCH", "5".into());
        impossible.insert(PASS_TIMEOUT_ENV, "17000".into());
        assert!(matches!(
            config(&impossible),
            Err(PublisherConfigError::PassBudgetInfeasible)
        ));
    }

    #[test]
    fn database_credential_must_be_distinct_but_target_the_same_server() {
        let mut same = values();
        same.insert(PUBLISHER_DATABASE_URL_ENV, same["DATABASE_URL"].clone());
        assert!(matches!(
            config(&same),
            Err(PublisherConfigError::DatabaseCredentialNotDistinct)
        ));

        let mut other = values();
        other.insert(
            PUBLISHER_DATABASE_URL_ENV,
            "postgres://publisher:SECRET@other:5432/myelin".into(),
        );
        assert!(matches!(
            config(&other),
            Err(PublisherConfigError::DatabaseServerIdentityMismatch)
        ));
    }

    struct FakeDrain {
        outcomes: Mutex<VecDeque<Result<ElectedDrainOutcome, ElectedRelayError>>>,
        calls: std::sync::atomic::AtomicUsize,
        stop: Option<Arc<AtomicBool>>,
    }

    impl DrainPass for FakeDrain {
        async fn drain_once(&self) -> Result<ElectedDrainOutcome, ElectedRelayError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(stop) = &self.stop {
                stop.store(true, Ordering::SeqCst);
            }
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(ElectedDrainOutcome::Standby))
        }
    }

    struct UnresponsiveDrain {
        calls: std::sync::atomic::AtomicUsize,
    }

    impl DrainPass for UnresponsiveDrain {
        async fn drain_once(&self) -> Result<ElectedDrainOutcome, ElectedRelayError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        }
    }

    fn outage() -> ElectedRelayError {
        ElectedRelayError::Relay(PgError::Publish("RAW_BROKER_SENTINEL".into()))
    }

    #[tokio::test]
    async fn standby_is_healthy_and_outage_recovers_without_restart() {
        let runtime = PublisherRuntime::new(
            FakeDrain {
                outcomes: Mutex::new(VecDeque::from([
                    Err(outage()),
                    Ok(ElectedDrainOutcome::Standby),
                    Ok(ElectedDrainOutcome::Published(2)),
                ])),
                calls: std::sync::atomic::AtomicUsize::new(0),
                stop: None,
            },
            Duration::from_millis(1),
            Duration::from_millis(1),
            Duration::from_secs(1),
        );
        assert_eq!(runtime.run_pass().await, PassResult::Unavailable);
        assert_eq!(runtime.snapshot().readiness, PublisherReadiness::NotReady);
        assert_eq!(runtime.run_pass().await, PassResult::Standby);
        assert_eq!(runtime.snapshot().readiness, PublisherReadiness::Ready);
        assert_eq!(runtime.run_pass().await, PassResult::Published(2));
        assert_eq!(runtime.snapshot().published, 2);
    }

    #[tokio::test]
    async fn shutdown_never_starts_a_second_pass_or_drains_to_empty() {
        let stop = Arc::new(AtomicBool::new(false));
        let runtime = PublisherRuntime::new(
            FakeDrain {
                outcomes: Mutex::new(VecDeque::from([Ok(ElectedDrainOutcome::Published(1000))])),
                calls: std::sync::atomic::AtomicUsize::new(0),
                stop: Some(stop.clone()),
            },
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(1),
        );
        runtime.serve_until(&stop).await;
        assert_eq!(runtime.drain.calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.snapshot().state, PublisherRuntimeState::Stopped);
    }

    #[tokio::test]
    async fn repeated_unresponsive_passes_each_stop_at_the_whole_pass_budget() {
        let runtime = PublisherRuntime::new(
            UnresponsiveDrain {
                calls: std::sync::atomic::AtomicUsize::new(0),
            },
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_millis(10),
        );
        for _ in 0..3 {
            assert_eq!(
                tokio::time::timeout(Duration::from_millis(100), runtime.run_pass())
                    .await
                    .expect("whole-pass timeout remains bounded"),
                PassResult::Unavailable
            );
        }
        assert_eq!(runtime.drain.calls.load(Ordering::SeqCst), 3);
        assert_eq!(runtime.snapshot().readiness, PublisherReadiness::NotReady);
    }

    #[tokio::test]
    async fn shutdown_interrupts_a_long_poll_sleep() {
        let stop = Arc::new(AtomicBool::new(false));
        let runtime = PublisherRuntime::new(
            FakeDrain {
                outcomes: Mutex::new(VecDeque::from([Ok(ElectedDrainOutcome::Standby)])),
                calls: std::sync::atomic::AtomicUsize::new(0),
                stop: None,
            },
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(1),
        );
        let signal = stop.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            signal.store(true, Ordering::SeqCst);
        });
        tokio::time::timeout(Duration::from_millis(200), runtime.serve_until(&stop))
            .await
            .expect("poll sleep is shutdown-interruptible");
        assert_eq!(runtime.drain.calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.snapshot().state, PublisherRuntimeState::Stopped);
    }

    #[test]
    fn every_typed_error_is_secret_free() {
        let config_errors = [
            PublisherConfigError::Missing,
            PublisherConfigError::NonUnicode,
            PublisherConfigError::InvalidDatabaseAuthority,
            PublisherConfigError::DatabaseCredentialNotDistinct,
            PublisherConfigError::DatabaseServerIdentityMismatch,
            PublisherConfigError::EmptyValue,
            PublisherConfigError::InvalidNumber,
            PublisherConfigError::OutOfBounds,
            PublisherConfigError::InvalidStreamPolicy,
            PublisherConfigError::PassBudgetInfeasible,
        ];
        let db_errors = [
            PublisherDbError::ConnectionFailed,
            PublisherDbError::ProbeFailed,
            PublisherDbError::DatabaseIdentityMismatch,
            PublisherDbError::RolesNotDistinct,
            PublisherDbError::LoginRequired,
            PublisherDbError::IdentityChanged,
            PublisherDbError::Superuser,
            PublisherDbError::BypassRls,
            PublisherDbError::CreateDatabase,
            PublisherDbError::CreateRole,
            PublisherDbError::DatabaseCreate,
            PublisherDbError::SchemaUsageMissing,
            PublisherDbError::SchemaCreate,
            PublisherDbError::SchemaOwnerMembership,
            PublisherDbError::CapabilityMembershipMissing,
            PublisherDbError::CapabilityNotInherited,
            PublisherDbError::CapabilityCanSetRole,
            PublisherDbError::UnexpectedMembership,
            PublisherDbError::InsufficientPrivileges,
            PublisherDbError::ExcessPrivileges,
        ];
        let rendered = config_errors
            .iter()
            .map(ToString::to_string)
            .chain(db_errors.iter().map(ToString::to_string))
            .collect::<String>();
        for sentinel in ["SECRET", "postgres://", "nats://", "RAW_BROKER_SENTINEL"] {
            assert!(!rendered.contains(sentinel));
        }
    }
}
