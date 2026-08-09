use std::collections::BTreeMap;

use sqlx::Row;

use crate::migration::{Migration, Migrations};
use crate::pg::PgError;
use crate::provider::{ProviderError, SubstrateProvider};

const AGENT: &str = "agent";
const DELEGATION: &str = "delegation";
const TENANT: &str = "tenant";
const TRIGGER_ACTOR: &str = "trigger_actor";
const ACTIVE: &str = "active";
const REVOKED: &str = "revoked";

pub const MAX_DELEGATION_POLICY_GRANTS: usize = 512;
pub const MAX_DELEGATION_POLICY_GRANT_BYTES: usize = 1_024;

pub const DELEGATION_POLICY_VERSION_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS delegation_policy_version (
    tenant_id       text NOT NULL,
    region          text NOT NULL,
    policy_kind     text NOT NULL CHECK (policy_kind IN ('agent', 'delegation', 'tenant', 'trigger_actor')),
    subject_id      text NOT NULL,
    trigger_actor_id text NOT NULL,
    version         bigint NOT NULL CHECK (version > 0),
    revision        bigint GENERATED ALWAYS AS IDENTITY,
    status          text NOT NULL CHECK (status IN ('active', 'revoked')),
    grants          text[] NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (tenant_id, region, policy_kind, subject_id, trigger_actor_id, version),
    UNIQUE (revision),
    CHECK (cardinality(grants) <= 512),
    CHECK (status <> 'revoked' OR cardinality(grants) = 0),
    CHECK (
        (policy_kind = 'tenant' AND subject_id = '' AND trigger_actor_id = '') OR
        (policy_kind = 'agent' AND subject_id <> '' AND trigger_actor_id = '') OR
        (policy_kind = 'trigger_actor' AND subject_id <> '' AND trigger_actor_id = '') OR
        (policy_kind = 'delegation' AND subject_id <> '' AND trigger_actor_id <> '')
    )
);"#;

pub const DELEGATION_POLICY_HEAD_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS delegation_policy_head (
    tenant_id        text NOT NULL,
    region           text NOT NULL,
    policy_kind      text NOT NULL,
    subject_id       text NOT NULL,
    trigger_actor_id text NOT NULL,
    version          bigint NOT NULL,
    revision         bigint NOT NULL,
    PRIMARY KEY (tenant_id, region, policy_kind, subject_id, trigger_actor_id),
    FOREIGN KEY (tenant_id, region, policy_kind, subject_id, trigger_actor_id, version)
        REFERENCES delegation_policy_version
            (tenant_id, region, policy_kind, subject_id, trigger_actor_id, version)
);"#;

pub const DELEGATION_RUN_SNAPSHOT_MIGRATION: &str = r#"
CREATE TABLE IF NOT EXISTS delegation_run_snapshot (
    tenant_id                text NOT NULL,
    region                   text NOT NULL,
    run_id                   text NOT NULL,
    agent_id                 text NOT NULL,
    trigger_actor_id         text NOT NULL,
    agent_version            bigint NOT NULL,
    agent_revision           bigint NOT NULL,
    delegation_version       bigint NOT NULL,
    delegation_revision      bigint NOT NULL,
    tenant_version           bigint NOT NULL,
    tenant_revision          bigint NOT NULL,
    trigger_actor_version    bigint NOT NULL,
    trigger_actor_revision   bigint NOT NULL,
    snapshot_cursor          bigint GENERATED ALWAYS AS IDENTITY,
    created_at               timestamptz NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (tenant_id, region, run_id),
    UNIQUE (snapshot_cursor)
);"#;

const VERSION_RLS: &str = r#"
ALTER TABLE delegation_policy_version ENABLE ROW LEVEL SECURITY;
ALTER TABLE delegation_policy_version FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON delegation_policy_version;
CREATE POLICY myelin_tenant_isolation ON delegation_policy_version
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));"#;

const HEAD_RLS: &str = r#"
ALTER TABLE delegation_policy_head ENABLE ROW LEVEL SECURITY;
ALTER TABLE delegation_policy_head FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON delegation_policy_head;
CREATE POLICY myelin_tenant_isolation ON delegation_policy_head
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));"#;

const SNAPSHOT_RLS: &str = r#"
ALTER TABLE delegation_run_snapshot ENABLE ROW LEVEL SECURITY;
ALTER TABLE delegation_run_snapshot FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON delegation_run_snapshot;
CREATE POLICY myelin_tenant_isolation ON delegation_run_snapshot
  USING (tenant_id = current_setting('myelin.tenant_id', true)
         AND region = current_setting('myelin.region', true))
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true)
              AND region = current_setting('myelin.region', true));"#;

pub fn delegation_policy_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain(
            "0061_delegation_policy_version",
            DELEGATION_POLICY_VERSION_MIGRATION,
        ),
        Migration::plain("0062_delegation_policy_version_rls", VERSION_RLS),
        Migration::plain(
            "0063_delegation_policy_head",
            DELEGATION_POLICY_HEAD_MIGRATION,
        ),
        Migration::plain("0064_delegation_policy_head_rls", HEAD_RLS),
        Migration::plain(
            "0065_delegation_run_snapshot",
            DELEGATION_RUN_SNAPSHOT_MIGRATION,
        ),
        Migration::plain("0066_delegation_run_snapshot_rls", SNAPSHOT_RLS),
    ])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableDelegationPolicyBundle {
    pub agent_policy: Vec<String>,
    pub delegation: Vec<String>,
    pub tenant_policy: Vec<String>,
    pub trigger_actor_held: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurableDelegationPolicyHeadCursor {
    pub version: i64,
    pub revision: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurableDelegationPolicyVersions {
    pub agent: i64,
    pub delegation: i64,
    pub tenant: i64,
    pub trigger_actor: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DurableDelegationPolicyRevisions {
    pub agent: i64,
    pub delegation: i64,
    pub tenant: i64,
    pub trigger_actor: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableDelegationPolicySnapshot {
    pub grants: DurableDelegationPolicyBundle,
    pub versions: DurableDelegationPolicyVersions,
    pub revisions: DurableDelegationPolicyRevisions,
    pub snapshot_cursor: i64,
}

#[derive(Debug)]
pub enum DurableDelegationPolicyError {
    Provider(ProviderError),
    InvalidGrantSet,
    VersionConflict,
    MissingPolicy(&'static str),
    RevokedPolicy(&'static str),
    StaleSnapshot,
    SnapshotBindingMismatch,
    CorruptSnapshot,
    IncompatiblePolicy(&'static str),
}

impl core::fmt::Display for DurableDelegationPolicyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Provider(error) => write!(f, "delegation policy storage failed: {error}"),
            Self::InvalidGrantSet => {
                f.write_str("delegation policy grant set is invalid or exceeds its bound")
            }
            Self::VersionConflict => f.write_str("delegation policy provisioning cursor is stale"),
            Self::MissingPolicy(slot) => write!(f, "delegation policy conjunct is missing: {slot}"),
            Self::RevokedPolicy(slot) => write!(f, "delegation policy conjunct is revoked: {slot}"),
            Self::StaleSnapshot => {
                f.write_str("delegation run snapshot is stale after a policy update")
            }
            Self::SnapshotBindingMismatch => {
                f.write_str("run id is already bound to different delegation principals")
            }
            Self::CorruptSnapshot => {
                f.write_str("delegation policy snapshot failed an integrity check")
            }
            Self::IncompatiblePolicy(slot) => write!(
                f,
                "delegation policy conjunct conflicts with the requested agent activation: {slot}"
            ),
        }
    }
}

/// Ensures the four policy heads needed by one external-agent activation in the caller's
/// transaction. Missing tenant/trigger ceilings are seeded from the supplied defaults; existing
/// ceilings are never widened. Agent and pairwise-delegation policies are exact and idempotent.
///
/// The inner error is a policy conflict discovered before any policy row is written. Database
/// errors use the outer result so the caller's transaction rolls back.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_agent_policy_bundle_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    agent_id: &str,
    trigger_actor_id: &str,
    mut agent_grants: Vec<String>,
    mut tenant_grants_if_missing: Vec<String>,
    mut trigger_actor_grants_if_missing: Vec<String>,
) -> Result<
    Result<
        (
            DurableDelegationPolicyVersions,
            DurableDelegationPolicyRevisions,
        ),
        DurableDelegationPolicyError,
    >,
    PgError,
> {
    for grants in [
        &mut agent_grants,
        &mut tenant_grants_if_missing,
        &mut trigger_actor_grants_if_missing,
    ] {
        if let Err(error) = canonicalize_grants(grants) {
            return Ok(Err(error));
        }
    }
    if agent_grants.is_empty()
        || !contains_every(&tenant_grants_if_missing, &agent_grants)
        || !contains_every(&trigger_actor_grants_if_missing, &agent_grants)
    {
        return Ok(Err(DurableDelegationPolicyError::IncompatiblePolicy(
            "new policy ceiling",
        )));
    }

    let keys = PolicyKey::all(agent_id, trigger_actor_id);
    lock_policy_keys(conn, tenant, region, &keys, false).await?;
    let mut heads = BTreeMap::new();
    for key in &keys {
        if let Some(row) = load_policy_head_for_update(conn, tenant, region, key).await? {
            heads.insert(key.kind.to_string(), row);
        }
    }
    if let Err(error) = validate_activation_heads(&heads, &agent_grants) {
        return Ok(Err(error));
    }

    for (key, grants) in [
        (PolicyKey::tenant(), tenant_grants_if_missing),
        (PolicyKey::agent(agent_id), agent_grants.clone()),
        (
            PolicyKey::trigger_actor(trigger_actor_id),
            trigger_actor_grants_if_missing,
        ),
        (
            PolicyKey::delegation(agent_id, trigger_actor_id),
            agent_grants,
        ),
    ] {
        if !heads.contains_key(key.kind) {
            let revision =
                append_and_advance_head(conn, tenant, region, &key, 1, ACTIVE, grants.clone())
                    .await?;
            heads.insert(
                key.kind.to_string(),
                PolicyRow {
                    kind: key.kind.to_string(),
                    version: 1,
                    revision,
                    status: ACTIVE.to_string(),
                    grants,
                },
            );
        }
    }

    let versions = versions_of(&heads).ok_or_else(|| {
        PgError::Query("agent activation did not produce four policy heads".into())
    })?;
    let revisions = revisions_of(&heads).ok_or_else(|| {
        PgError::Query("agent activation did not produce four policy revisions".into())
    })?;
    Ok(Ok((versions, revisions)))
}

fn validate_activation_heads(
    heads: &BTreeMap<String, PolicyRow>,
    agent_grants: &[String],
) -> Result<(), DurableDelegationPolicyError> {
    for kind in [TENANT, TRIGGER_ACTOR] {
        let Some(row) = heads.get(kind) else {
            continue;
        };
        require_active(row, kind)?;
        if !contains_every(&row.grants, agent_grants) {
            return Err(DurableDelegationPolicyError::IncompatiblePolicy(kind));
        }
    }
    for kind in [AGENT, DELEGATION] {
        let Some(row) = heads.get(kind) else {
            continue;
        };
        require_active(row, kind)?;
        if row.grants != agent_grants {
            return Err(DurableDelegationPolicyError::IncompatiblePolicy(kind));
        }
    }
    Ok(())
}

fn require_active(row: &PolicyRow, kind: &'static str) -> Result<(), DurableDelegationPolicyError> {
    match row.status.as_str() {
        ACTIVE => Ok(()),
        REVOKED => Err(DurableDelegationPolicyError::RevokedPolicy(kind)),
        _ => Err(DurableDelegationPolicyError::CorruptSnapshot),
    }
}

fn contains_every(haystack: &[String], needles: &[String]) -> bool {
    needles
        .iter()
        .all(|needle| haystack.binary_search(needle).is_ok())
}

impl std::error::Error for DurableDelegationPolicyError {}

impl From<ProviderError> for DurableDelegationPolicyError {
    fn from(value: ProviderError) -> Self {
        Self::Provider(value)
    }
}

#[derive(Clone, Debug)]
struct PolicyRow {
    kind: String,
    version: i64,
    revision: i64,
    status: String,
    grants: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
struct SnapshotRow {
    agent_version: i64,
    agent_revision: i64,
    delegation_version: i64,
    delegation_revision: i64,
    tenant_version: i64,
    tenant_revision: i64,
    trigger_actor_version: i64,
    trigger_actor_revision: i64,
    snapshot_cursor: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PolicyKey {
    kind: &'static str,
    subject: String,
    actor: String,
}

impl PolicyKey {
    fn tenant() -> Self {
        Self {
            kind: TENANT,
            subject: String::new(),
            actor: String::new(),
        }
    }

    fn agent(agent: &str) -> Self {
        Self {
            kind: AGENT,
            subject: agent.to_string(),
            actor: String::new(),
        }
    }

    fn trigger_actor(actor: &str) -> Self {
        Self {
            kind: TRIGGER_ACTOR,
            subject: actor.to_string(),
            actor: String::new(),
        }
    }

    fn delegation(agent: &str, actor: &str) -> Self {
        Self {
            kind: DELEGATION,
            subject: agent.to_string(),
            actor: actor.to_string(),
        }
    }

    fn all(agent: &str, actor: &str) -> Vec<Self> {
        vec![
            Self::tenant(),
            Self::agent(agent),
            Self::trigger_actor(actor),
            Self::delegation(agent, actor),
        ]
    }

    fn lock_identity(&self, region: &str) -> String {
        format!(
            "{}:{region}{}:{}{}:{}{}:{}",
            region.len(),
            self.kind.len(),
            self.kind,
            self.subject.len(),
            self.subject,
            self.actor.len(),
            self.actor
        )
    }
}

#[derive(Clone)]
pub struct DurableDelegationPolicyBacking {
    provider: SubstrateProvider,
}

impl DurableDelegationPolicyBacking {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self { provider }
    }

    pub fn region(&self) -> &str {
        &self.provider.config().region
    }

    pub async fn provision_tenant_policy(
        &self,
        tenant: &str,
        expected: Option<DurableDelegationPolicyHeadCursor>,
        grants: Vec<String>,
    ) -> Result<DurableDelegationPolicyHeadCursor, DurableDelegationPolicyError> {
        self.provision_policy(tenant, PolicyKey::tenant(), expected, grants, ACTIVE)
            .await
    }

    pub async fn provision_agent_policy(
        &self,
        tenant: &str,
        agent_id: &str,
        expected: Option<DurableDelegationPolicyHeadCursor>,
        grants: Vec<String>,
    ) -> Result<DurableDelegationPolicyHeadCursor, DurableDelegationPolicyError> {
        self.provision_policy(tenant, PolicyKey::agent(agent_id), expected, grants, ACTIVE)
            .await
    }

    pub async fn provision_trigger_actor_policy(
        &self,
        tenant: &str,
        trigger_actor_id: &str,
        expected: Option<DurableDelegationPolicyHeadCursor>,
        grants: Vec<String>,
    ) -> Result<DurableDelegationPolicyHeadCursor, DurableDelegationPolicyError> {
        self.provision_policy(
            tenant,
            PolicyKey::trigger_actor(trigger_actor_id),
            expected,
            grants,
            ACTIVE,
        )
        .await
    }

    pub async fn provision_delegation(
        &self,
        tenant: &str,
        agent_id: &str,
        trigger_actor_id: &str,
        expected: Option<DurableDelegationPolicyHeadCursor>,
        grants: Vec<String>,
    ) -> Result<DurableDelegationPolicyHeadCursor, DurableDelegationPolicyError> {
        self.provision_policy(
            tenant,
            PolicyKey::delegation(agent_id, trigger_actor_id),
            expected,
            grants,
            ACTIVE,
        )
        .await
    }

    async fn provision_policy(
        &self,
        tenant: &str,
        key: PolicyKey,
        expected: Option<DurableDelegationPolicyHeadCursor>,
        mut grants: Vec<String>,
        status: &'static str,
    ) -> Result<DurableDelegationPolicyHeadCursor, DurableDelegationPolicyError> {
        canonicalize_grants(&mut grants)?;
        let tenant = tenant.to_string();
        let region = self.region().to_string();
        let result = self
            .provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    lock_policy_keys(conn, &tenant, &region, std::slice::from_ref(&key), false)
                        .await?;
                    let current = load_policy_head_for_update(conn, &tenant, &region, &key).await?;
                    let current_cursor =
                        current
                            .as_ref()
                            .map(|row| DurableDelegationPolicyHeadCursor {
                                version: row.version,
                                revision: row.revision,
                            });
                    let admitted = match (expected, current_cursor) {
                        (None, None) => true,
                        (Some(want), Some(have)) => want == have,
                        _ => false,
                    };
                    if !admitted {
                        return Ok(Err(DurableDelegationPolicyError::VersionConflict));
                    }

                    let version = expected.map_or(1, |cursor| cursor.version + 1);
                    let revision = append_and_advance_head(
                        conn, &tenant, &region, &key, version, status, grants,
                    )
                    .await?;
                    Ok(Ok(DurableDelegationPolicyHeadCursor { version, revision }))
                })
            })
            .await?;
        result
    }

    pub async fn revoke_delegation(
        &self,
        tenant: &str,
        agent_id: &str,
        trigger_actor_id: &str,
        expected: DurableDelegationPolicyHeadCursor,
    ) -> Result<DurableDelegationPolicyHeadCursor, DurableDelegationPolicyError> {
        self.provision_policy(
            tenant,
            PolicyKey::delegation(agent_id, trigger_actor_id),
            Some(expected),
            Vec::new(),
            REVOKED,
        )
        .await
    }

    pub async fn resolve_snapshot(
        &self,
        tenant: &str,
        run_id: &str,
        agent_id: &str,
        trigger_actor_id: &str,
    ) -> Result<DurableDelegationPolicySnapshot, DurableDelegationPolicyError> {
        let tenant = tenant.to_string();
        let region = self.region().to_string();
        let run_id = run_id.to_string();
        let agent_id = agent_id.to_string();
        let trigger_actor_id = trigger_actor_id.to_string();
        let result = self
            .provider
            .with_tenant_tx(&tenant.clone(), move |conn| {
                Box::pin(async move {
                    let keys = PolicyKey::all(&agent_id, &trigger_actor_id);
                    lock_policy_keys(conn, &tenant, &region, &keys, true).await?;
                    let existing = load_snapshot_row(
                        conn,
                        &tenant,
                        &region,
                        &run_id,
                        &agent_id,
                        &trigger_actor_id,
                    )
                    .await?;
                    let heads =
                        load_heads_for_share(conn, &tenant, &region, &agent_id, &trigger_actor_id)
                            .await?;
                    if let Err(error) = require_complete_active(&heads) {
                        return Ok(Err(error));
                    }
                    let versions = versions_of(&heads).ok_or_else(|| {
                        PgError::Query("complete policy heads lacked versions".into())
                    })?;
                    let revisions = revisions_of(&heads).ok_or_else(|| {
                        PgError::Query("complete policy heads lacked revisions".into())
                    })?;

                    let snapshot = match existing {
                        ExistingSnapshot::Bound(row) => {
                            if row_versions(row) != versions || row_revisions(row) != revisions {
                                return Ok(Err(DurableDelegationPolicyError::StaleSnapshot));
                            }
                            row
                        }
                        ExistingSnapshot::BindingMismatch => {
                            return Ok(Err(DurableDelegationPolicyError::SnapshotBindingMismatch));
                        }
                        ExistingSnapshot::Missing => {
                            match insert_snapshot(
                                conn,
                                &tenant,
                                &region,
                                &run_id,
                                &agent_id,
                                &trigger_actor_id,
                                versions,
                                revisions,
                            )
                            .await?
                            {
                                Some(row) => row,
                                None => match load_snapshot_row(
                                    conn,
                                    &tenant,
                                    &region,
                                    &run_id,
                                    &agent_id,
                                    &trigger_actor_id,
                                )
                                .await?
                                {
                                    ExistingSnapshot::Bound(row)
                                        if row_versions(row) == versions
                                            && row_revisions(row) == revisions =>
                                    {
                                        row
                                    }
                                    ExistingSnapshot::Bound(_) => {
                                        return Ok(Err(
                                            DurableDelegationPolicyError::StaleSnapshot,
                                        ));
                                    }
                                    ExistingSnapshot::BindingMismatch => {
                                        return Ok(Err(
                                            DurableDelegationPolicyError::SnapshotBindingMismatch,
                                        ));
                                    }
                                    ExistingSnapshot::Missing => {
                                        return Ok(Err(
                                            DurableDelegationPolicyError::CorruptSnapshot,
                                        ));
                                    }
                                },
                            }
                        }
                    };

                    let grants = bundle_of(&heads).ok_or_else(|| {
                        PgError::Query("complete policy heads lacked grants".into())
                    })?;
                    Ok(Ok(DurableDelegationPolicySnapshot {
                        grants,
                        versions,
                        revisions,
                        snapshot_cursor: snapshot.snapshot_cursor,
                    }))
                })
            })
            .await?;
        result
    }
}

async fn lock_policy_keys(
    conn: &mut sqlx::PgConnection,
    tenant_id: &str,
    region: &str,
    keys: &[PolicyKey],
    shared: bool,
) -> Result<(), PgError> {
    let mut identities: Vec<String> = keys.iter().map(|key| key.lock_identity(region)).collect();
    identities.sort();
    identities.dedup();
    for identity in identities {
        let result = if shared {
            sqlx::query(
                "SELECT pg_advisory_xact_lock_shared( \
                    hashtextextended(length($2)::text || ':' || $2 || ':' || $1, 0))",
            )
            .bind(&identity)
            .bind(tenant_id)
            .execute(&mut *conn)
            .await
        } else {
            sqlx::query(
                "SELECT pg_advisory_xact_lock( \
                    hashtextextended(length($2)::text || ':' || $2 || ':' || $1, 0))",
            )
            .bind(&identity)
            .bind(tenant_id)
            .execute(&mut *conn)
            .await
        };
        result.map_err(|error| PgError::Query(error.to_string()))?;
    }
    Ok(())
}

fn canonicalize_grants(grants: &mut Vec<String>) -> Result<(), DurableDelegationPolicyError> {
    grants.sort();
    grants.dedup();
    if grants.len() > MAX_DELEGATION_POLICY_GRANTS
        || grants
            .iter()
            .any(|grant| grant.is_empty() || grant.len() > MAX_DELEGATION_POLICY_GRANT_BYTES)
    {
        return Err(DurableDelegationPolicyError::InvalidGrantSet);
    }
    Ok(())
}

async fn append_and_advance_head(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    key: &PolicyKey,
    version: i64,
    status: &str,
    grants: Vec<String>,
) -> Result<i64, PgError> {
    let revision: i64 = sqlx::query_scalar(
        "INSERT INTO delegation_policy_version \
         (tenant_id, region, policy_kind, subject_id, trigger_actor_id, version, status, grants) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING revision",
    )
    .bind(tenant)
    .bind(region)
    .bind(key.kind)
    .bind(&key.subject)
    .bind(&key.actor)
    .bind(version)
    .bind(status)
    .bind(grants)
    .fetch_one(&mut *conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    sqlx::query(
        "INSERT INTO delegation_policy_head \
         (tenant_id, region, policy_kind, subject_id, trigger_actor_id, version, revision) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (tenant_id, region, policy_kind, subject_id, trigger_actor_id) \
         DO UPDATE SET version = EXCLUDED.version, revision = EXCLUDED.revision",
    )
    .bind(tenant)
    .bind(region)
    .bind(key.kind)
    .bind(&key.subject)
    .bind(&key.actor)
    .bind(version)
    .bind(revision)
    .execute(&mut *conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    Ok(revision)
}

async fn load_policy_head_for_update(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    key: &PolicyKey,
) -> Result<Option<PolicyRow>, PgError> {
    let row = sqlx::query(
        "SELECT h.policy_kind, h.version, h.revision, v.status, v.grants \
         FROM delegation_policy_head h \
         JOIN delegation_policy_version v \
           ON v.tenant_id = h.tenant_id AND v.region = h.region \
          AND v.policy_kind = h.policy_kind AND v.subject_id = h.subject_id \
          AND v.trigger_actor_id = h.trigger_actor_id AND v.version = h.version \
         WHERE h.tenant_id = $1 AND h.region = $2 AND h.policy_kind = $3 \
           AND h.subject_id = $4 AND h.trigger_actor_id = $5 \
         FOR UPDATE OF h",
    )
    .bind(tenant)
    .bind(region)
    .bind(key.kind)
    .bind(&key.subject)
    .bind(&key.actor)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    Ok(row.map(|row| PolicyRow {
        kind: row.get("policy_kind"),
        version: row.get("version"),
        revision: row.get("revision"),
        status: row.get("status"),
        grants: row.get("grants"),
    }))
}

async fn load_heads_for_share(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    agent: &str,
    actor: &str,
) -> Result<BTreeMap<String, PolicyRow>, PgError> {
    let rows = sqlx::query(
        "SELECT h.policy_kind, h.version, h.revision, v.status, v.grants \
         FROM delegation_policy_head h \
         JOIN delegation_policy_version v \
           ON v.tenant_id = h.tenant_id AND v.region = h.region \
          AND v.policy_kind = h.policy_kind AND v.subject_id = h.subject_id \
          AND v.trigger_actor_id = h.trigger_actor_id AND v.version = h.version \
         WHERE h.tenant_id = $1 AND h.region = $2 AND ( \
              (h.policy_kind = 'agent' AND h.subject_id = $3 AND h.trigger_actor_id = '') OR \
              (h.policy_kind = 'delegation' AND h.subject_id = $3 AND h.trigger_actor_id = $4) OR \
              (h.policy_kind = 'tenant' AND h.subject_id = '' AND h.trigger_actor_id = '') OR \
              (h.policy_kind = 'trigger_actor' AND h.subject_id = $4 AND h.trigger_actor_id = '') \
         ) FOR SHARE OF h, v",
    )
    .bind(tenant)
    .bind(region)
    .bind(agent)
    .bind(actor)
    .fetch_all(&mut *conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let policy = PolicyRow {
                kind: row.get("policy_kind"),
                version: row.get("version"),
                revision: row.get("revision"),
                status: row.get("status"),
                grants: row.get("grants"),
            };
            (policy.kind.clone(), policy)
        })
        .collect())
}

fn require_complete_active(
    rows: &BTreeMap<String, PolicyRow>,
) -> Result<(), DurableDelegationPolicyError> {
    for kind in [AGENT, DELEGATION, TENANT, TRIGGER_ACTOR] {
        let row = rows
            .get(kind)
            .ok_or(DurableDelegationPolicyError::MissingPolicy(kind))?;
        if row.status == REVOKED {
            return Err(DurableDelegationPolicyError::RevokedPolicy(kind));
        }
        if row.status != ACTIVE {
            return Err(DurableDelegationPolicyError::CorruptSnapshot);
        }
    }
    Ok(())
}

fn versions_of(rows: &BTreeMap<String, PolicyRow>) -> Option<DurableDelegationPolicyVersions> {
    Some(DurableDelegationPolicyVersions {
        agent: rows.get(AGENT)?.version,
        delegation: rows.get(DELEGATION)?.version,
        tenant: rows.get(TENANT)?.version,
        trigger_actor: rows.get(TRIGGER_ACTOR)?.version,
    })
}

fn revisions_of(rows: &BTreeMap<String, PolicyRow>) -> Option<DurableDelegationPolicyRevisions> {
    Some(DurableDelegationPolicyRevisions {
        agent: rows.get(AGENT)?.revision,
        delegation: rows.get(DELEGATION)?.revision,
        tenant: rows.get(TENANT)?.revision,
        trigger_actor: rows.get(TRIGGER_ACTOR)?.revision,
    })
}

fn bundle_of(rows: &BTreeMap<String, PolicyRow>) -> Option<DurableDelegationPolicyBundle> {
    Some(DurableDelegationPolicyBundle {
        agent_policy: rows.get(AGENT)?.grants.clone(),
        delegation: rows.get(DELEGATION)?.grants.clone(),
        tenant_policy: rows.get(TENANT)?.grants.clone(),
        trigger_actor_held: rows.get(TRIGGER_ACTOR)?.grants.clone(),
    })
}

enum ExistingSnapshot {
    Missing,
    Bound(SnapshotRow),
    BindingMismatch,
}

async fn load_snapshot_row(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    run_id: &str,
    agent: &str,
    actor: &str,
) -> Result<ExistingSnapshot, PgError> {
    let row = sqlx::query(
        "SELECT agent_id, trigger_actor_id, agent_version, agent_revision, \
                delegation_version, delegation_revision, tenant_version, tenant_revision, \
                trigger_actor_version, trigger_actor_revision, snapshot_cursor \
         FROM delegation_run_snapshot \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR SHARE",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    let Some(row) = row else {
        return Ok(ExistingSnapshot::Missing);
    };
    if row.get::<String, _>("agent_id") != agent
        || row.get::<String, _>("trigger_actor_id") != actor
    {
        return Ok(ExistingSnapshot::BindingMismatch);
    }
    Ok(ExistingSnapshot::Bound(snapshot_from_row(&row)))
}

#[allow(clippy::too_many_arguments)]
async fn insert_snapshot(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    run_id: &str,
    agent: &str,
    actor: &str,
    versions: DurableDelegationPolicyVersions,
    revisions: DurableDelegationPolicyRevisions,
) -> Result<Option<SnapshotRow>, PgError> {
    let row = sqlx::query(
        "INSERT INTO delegation_run_snapshot \
         (tenant_id, region, run_id, agent_id, trigger_actor_id, \
          agent_version, agent_revision, delegation_version, delegation_revision, \
          tenant_version, tenant_revision, trigger_actor_version, trigger_actor_revision) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (tenant_id, region, run_id) DO NOTHING \
         RETURNING agent_version, agent_revision, delegation_version, delegation_revision, \
                   tenant_version, tenant_revision, trigger_actor_version, \
                   trigger_actor_revision, snapshot_cursor",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(agent)
    .bind(actor)
    .bind(versions.agent)
    .bind(revisions.agent)
    .bind(versions.delegation)
    .bind(revisions.delegation)
    .bind(versions.tenant)
    .bind(revisions.tenant)
    .bind(versions.trigger_actor)
    .bind(revisions.trigger_actor)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| PgError::Query(error.to_string()))?;
    Ok(row.as_ref().map(snapshot_from_row))
}

fn snapshot_from_row(row: &sqlx::postgres::PgRow) -> SnapshotRow {
    SnapshotRow {
        agent_version: row.get("agent_version"),
        agent_revision: row.get("agent_revision"),
        delegation_version: row.get("delegation_version"),
        delegation_revision: row.get("delegation_revision"),
        tenant_version: row.get("tenant_version"),
        tenant_revision: row.get("tenant_revision"),
        trigger_actor_version: row.get("trigger_actor_version"),
        trigger_actor_revision: row.get("trigger_actor_revision"),
        snapshot_cursor: row.get("snapshot_cursor"),
    }
}

fn row_versions(row: SnapshotRow) -> DurableDelegationPolicyVersions {
    DurableDelegationPolicyVersions {
        agent: row.agent_version,
        delegation: row.delegation_version,
        tenant: row.tenant_version,
        trigger_actor: row.trigger_actor_version,
    }
}

fn row_revisions(row: SnapshotRow) -> DurableDelegationPolicyRevisions {
    DurableDelegationPolicyRevisions {
        agent: row.agent_revision,
        delegation: row.delegation_revision,
        tenant: row.tenant_revision,
        trigger_actor: row.trigger_actor_revision,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active(kind: &str, grants: &[&str]) -> PolicyRow {
        PolicyRow {
            kind: kind.into(),
            version: 1,
            revision: 1,
            status: ACTIVE.into(),
            grants: grants.iter().map(|grant| (*grant).into()).collect(),
        }
    }

    #[test]
    fn grant_sets_are_canonical_and_bounded() {
        let mut grants = vec!["b".into(), "a".into(), "a".into()];
        canonicalize_grants(&mut grants).unwrap();
        assert_eq!(grants, ["a", "b"]);

        grants = vec![String::new()];
        assert!(matches!(
            canonicalize_grants(&mut grants),
            Err(DurableDelegationPolicyError::InvalidGrantSet)
        ));
    }

    #[test]
    fn activation_reuses_ceilings_but_requires_exact_agent_and_delegation_heads() {
        let grants = vec!["repo.pull".into(), "repo.push".into()];
        let mut heads = BTreeMap::from([
            (
                TENANT.into(),
                active(TENANT, &["repo.administer", "repo.pull", "repo.push"]),
            ),
            (
                TRIGGER_ACTOR.into(),
                active(TRIGGER_ACTOR, &["repo.pull", "repo.push"]),
            ),
            (AGENT.into(), active(AGENT, &["repo.pull", "repo.push"])),
            (
                DELEGATION.into(),
                active(DELEGATION, &["repo.pull", "repo.push"]),
            ),
        ]);
        assert!(validate_activation_heads(&heads, &grants).is_ok());

        heads.insert(AGENT.into(), active(AGENT, &["repo.pull"]));
        assert!(matches!(
            validate_activation_heads(&heads, &grants),
            Err(DurableDelegationPolicyError::IncompatiblePolicy(AGENT))
        ));
        heads.insert(AGENT.into(), active(AGENT, &["repo.pull", "repo.push"]));
        heads.insert(TENANT.into(), active(TENANT, &["repo.pull"]));
        assert!(matches!(
            validate_activation_heads(&heads, &grants),
            Err(DurableDelegationPolicyError::IncompatiblePolicy(TENANT))
        ));
    }

    #[test]
    fn migration_ids_follow_cell_root_and_are_forward_only() {
        let migrations = delegation_policy_durable_migrations();
        assert_eq!(
            migrations.0.first().unwrap().id,
            "0061_delegation_policy_version"
        );
        assert_eq!(
            migrations.0.last().unwrap().id,
            "0066_delegation_run_snapshot_rls"
        );
        assert!(migrations
            .0
            .iter()
            .all(|migration| !crate::migration::is_destructive(migration.ddl)));
    }
}
