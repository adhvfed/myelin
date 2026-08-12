use sqlx::Row;

use myelin_events::EventEnvelope;

use crate::migration::{Migration, Migrations};
use crate::pg::PgStore;
use crate::pgrelay::PgRelay;
use crate::provider::{ProviderError, SubstrateProvider};

/// The JSON tokens persisted in `principal.kind` and `principal.status`.
/// Changing either requires an explicit data migration, not an incidental serde rename.
pub const HUMAN_PRINCIPAL_KIND_JSON: &str = r#""Human""#;
pub const ACTIVE_PRINCIPAL_STATUS_JSON: &str = r#""Active""#;

pub const PRINCIPAL_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS principal (
    tenant_id          text  NOT NULL,
    region             text  NOT NULL,
    principal_id       text  NOT NULL,
    kind               text  NOT NULL,
    data_role          text  NOT NULL,
    status             text  NOT NULL,
    profile_key_ref    text,
    profile_nonce      bytea,
    profile_ciphertext bytea,
    PRIMARY KEY (tenant_id, region, principal_id)
);";

pub const PRINCIPAL_RLS_POLICY: &str = "\
ALTER TABLE principal ENABLE ROW LEVEL SECURITY;
ALTER TABLE principal FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON principal;
CREATE POLICY myelin_tenant_isolation ON principal \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub const REVOCATION_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS revocation (
    tenant_id  text NOT NULL,
    region     text NOT NULL,
    kind       text NOT NULL,
    handle     text NOT NULL,
    revoked_at text NOT NULL,
    expires_at text,
    PRIMARY KEY (tenant_id, region, kind, handle)
);";

pub const REVOCATION_RLS_POLICY: &str = "\
ALTER TABLE revocation ENABLE ROW LEVEL SECURITY;
ALTER TABLE revocation FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON revocation;
CREATE POLICY myelin_tenant_isolation ON revocation \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub const RUN_TOKEN_TEARDOWN_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS run_token_teardown (
    tenant_id text NOT NULL,
    region    text NOT NULL,
    jti       text NOT NULL,
    PRIMARY KEY (tenant_id, region, jti)
);";

pub const RUN_TOKEN_TEARDOWN_RLS_POLICY: &str = "\
ALTER TABLE run_token_teardown ENABLE ROW LEVEL SECURITY;
ALTER TABLE run_token_teardown FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON run_token_teardown;
CREATE POLICY myelin_tenant_isolation ON run_token_teardown \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub const IDENTITY_PROJECT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS identity_project (
    tenant_id             text        NOT NULL,
    region                text        NOT NULL,
    project_id            uuid        NOT NULL,
    name                  text        NOT NULL,
    issue_prefix          text        NOT NULL,
    default_issue_type_id uuid        NOT NULL,
    created_by            text        NOT NULL,
    client_nonce          text        NOT NULL,
    created_at            timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, region, project_id),
    UNIQUE (tenant_id, region, issue_prefix),
    UNIQUE (tenant_id, region, client_nonce),
    CHECK (length(name) BETWEEN 1 AND 100),
    CHECK (length(issue_prefix) BETWEEN 2 AND 10),
    CHECK (length(client_nonce) BETWEEN 1 AND 128)
);
CREATE INDEX IF NOT EXISTS identity_project_keyset
    ON identity_project (tenant_id, region, project_id DESC);";

pub const IDENTITY_AGENT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS identity_agent (
    tenant_id   text        NOT NULL,
    region      text        NOT NULL,
    agent_id    uuid        NOT NULL,
    name        text        NOT NULL,
    runtime_ref text        NOT NULL,
    created_by  text        NOT NULL,
    client_nonce text       NOT NULL,
    tools       text[]      NOT NULL,
    grants      text[]      NOT NULL,
    created_at  timestamptz NOT NULL,
    PRIMARY KEY (tenant_id, region, agent_id),
    UNIQUE (tenant_id, region, name),
    UNIQUE (tenant_id, region, client_nonce),
    CHECK (length(name) BETWEEN 1 AND 80),
    CHECK (length(runtime_ref) BETWEEN 1 AND 255),
    CHECK (length(created_by) BETWEEN 1 AND 255),
    CHECK (length(client_nonce) BETWEEN 1 AND 128),
    CHECK (cardinality(tools) BETWEEN 1 AND 128),
    CHECK (cardinality(grants) BETWEEN 1 AND 512)
);
CREATE INDEX IF NOT EXISTS identity_agent_keyset
    ON identity_agent (tenant_id, region, agent_id DESC);";

pub const IDENTITY_AGENT_RLS_POLICY: &str = "\
ALTER TABLE identity_agent ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity_agent FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON identity_agent;
CREATE POLICY myelin_tenant_isolation ON identity_agent \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub const IDENTITY_PROJECT_RLS_POLICY: &str = "\
ALTER TABLE identity_project ENABLE ROW LEVEL SECURITY;
ALTER TABLE identity_project FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON identity_project;
CREATE POLICY myelin_tenant_isolation ON identity_project \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub const AUTH_REPLAY_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS auth_replay (
    tenant_id text   NOT NULL,
    region    text   NOT NULL,
    namespace text   NOT NULL,
    replay_id text   NOT NULL,
    expires_at bigint NOT NULL,
    PRIMARY KEY (tenant_id, region, namespace, replay_id)
);
CREATE INDEX IF NOT EXISTS auth_replay_expiry_idx
    ON auth_replay (tenant_id, region, expires_at);";

pub const AUTH_REPLAY_RLS_POLICY: &str = "\
ALTER TABLE auth_replay ENABLE ROW LEVEL SECURITY;
ALTER TABLE auth_replay FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON auth_replay;
CREATE POLICY myelin_tenant_isolation ON auth_replay \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub const CREDENTIAL_LINK_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS credential_link (
    tenant_id    text NOT NULL,
    region       text NOT NULL,
    link_key     text NOT NULL,
    principal_id text NOT NULL,
    PRIMARY KEY (tenant_id, region, link_key)
);";

pub const CREDENTIAL_LINK_RLS_POLICY: &str = "\
ALTER TABLE credential_link ENABLE ROW LEVEL SECURITY;
ALTER TABLE credential_link FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON credential_link;
CREATE POLICY myelin_tenant_isolation ON credential_link \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub const REBAC_TUPLE_RLS_POLICY: &str = "\
ALTER TABLE rebac_tuple ENABLE ROW LEVEL SECURITY;
ALTER TABLE rebac_tuple FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON rebac_tuple;
CREATE POLICY myelin_tenant_isolation ON rebac_tuple \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

pub fn identity_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0010_rebac_tuple", crate::pg::REBAC_TUPLE_MIGRATION),
        Migration::plain("0011_rebac_tuple_rls", REBAC_TUPLE_RLS_POLICY),
        Migration::plain("0012_principal", PRINCIPAL_MIGRATION),
        Migration::plain("0013_principal_rls", PRINCIPAL_RLS_POLICY),
        Migration::plain("0014_credential_link", CREDENTIAL_LINK_MIGRATION),
        Migration::plain("0015_credential_link_rls", CREDENTIAL_LINK_RLS_POLICY),
        Migration::plain("0016_revocation", REVOCATION_MIGRATION),
        Migration::plain("0017_revocation_rls", REVOCATION_RLS_POLICY),
        Migration::plain("0018_run_token_teardown", RUN_TOKEN_TEARDOWN_MIGRATION),
        Migration::plain("0019_run_token_teardown_rls", RUN_TOKEN_TEARDOWN_RLS_POLICY),
    ])
}

pub fn identity_project_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0082_identity_project", IDENTITY_PROJECT_MIGRATION),
        Migration::plain("0083_identity_project_rls", IDENTITY_PROJECT_RLS_POLICY),
    ])
}

pub fn identity_agent_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0084_identity_agent", IDENTITY_AGENT_MIGRATION),
        Migration::plain("0085_identity_agent_rls", IDENTITY_AGENT_RLS_POLICY),
    ])
}

pub fn auth_replay_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0070_auth_replay", AUTH_REPLAY_MIGRATION),
        Migration::plain("0071_auth_replay_rls", AUTH_REPLAY_RLS_POLICY),
    ])
}

#[derive(Clone, Debug)]
pub enum TupleEdgeOp {
    Add,
    Remove,
}

#[derive(Clone)]
pub struct DurableTupleBacking {
    provider: SubstrateProvider,
}

impl DurableTupleBacking {
    pub fn new(provider: SubstrateProvider) -> DurableTupleBacking {
        DurableTupleBacking { provider }
    }

    pub async fn apply_deltas_co_commit(
        &self,
        tenant: &str,
        region: &str,
        deltas: Vec<(TupleEdgeOp, String, String, String)>,
        aggregate: &str,
        envelope: &EventEnvelope,
    ) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region_owned = region.to_string();
        let aggregate_owned = aggregate.to_string();
        let envelope_owned = envelope.clone();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    for (op, object, relation, subject) in &deltas {
                        match op {
                            TupleEdgeOp::Add => {
                                PgStore::insert_tuple_on_conn(
                                    conn,
                                    &tenant_owned,
                                    &region_owned,
                                    object,
                                    relation,
                                    subject,
                                )
                                .await?
                            }
                            TupleEdgeOp::Remove => {
                                PgStore::delete_tuple_on_conn(
                                    conn,
                                    &tenant_owned,
                                    &region_owned,
                                    object,
                                    relation,
                                    subject,
                                )
                                .await?
                            }
                        }
                    }
                    PgRelay::co_commit_in_tx(conn, &aggregate_owned, &envelope_owned).await?;
                    Ok(())
                })
            })
            .await
    }

    pub async fn edges_in(
        &self,
        tenant: &str,
        region: &str,
    ) -> Result<Vec<(String, String, String)>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region_owned = region.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    PgStore::tuples_on_conn(conn, &tenant_owned, &region_owned).await
                })
            })
            .await
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableProfileBlob {
    pub key_ref: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurablePrincipalRow {
    pub principal_id: String,
    pub kind: String,
    pub data_role: String,
    pub status: String,
    pub profile: Option<DurableProfileBlob>,
}

#[derive(Clone)]
pub struct DurablePrincipalBacking {
    provider: SubstrateProvider,
}

impl DurablePrincipalBacking {
    pub fn new(provider: SubstrateProvider) -> DurablePrincipalBacking {
        DurablePrincipalBacking { provider }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    pub async fn put_principal(
        &self,
        tenant: &str,
        row: DurablePrincipalRow,
    ) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let (key_ref, nonce, ciphertext) = match &row.profile {
                        Some(b) => (
                            Some(b.key_ref.clone()),
                            Some(b.nonce.clone()),
                            Some(b.ciphertext.clone()),
                        ),
                        None => (None, None, None),
                    };
                    sqlx::query(
                        "INSERT INTO principal \
                           (tenant_id, region, principal_id, kind, data_role, status, \
                            profile_key_ref, profile_nonce, profile_ciphertext) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                         ON CONFLICT (tenant_id, region, principal_id) DO UPDATE SET \
                           kind = EXCLUDED.kind, data_role = EXCLUDED.data_role, \
                           status = EXCLUDED.status, profile_key_ref = EXCLUDED.profile_key_ref, \
                           profile_nonce = EXCLUDED.profile_nonce, \
                           profile_ciphertext = EXCLUDED.profile_ciphertext",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&row.principal_id)
                    .bind(&row.kind)
                    .bind(&row.data_role)
                    .bind(&row.status)
                    .bind(key_ref)
                    .bind(nonce)
                    .bind(ciphertext)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(())
                })
            })
            .await
    }

    pub async fn put_principal_and_link_credential(
        &self,
        tenant: &str,
        row: DurablePrincipalRow,
        link_key: &str,
    ) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let link_key = link_key.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let (key_ref, nonce, ciphertext) = match &row.profile {
                        Some(blob) => (
                            Some(blob.key_ref.clone()),
                            Some(blob.nonce.clone()),
                            Some(blob.ciphertext.clone()),
                        ),
                        None => (None, None, None),
                    };
                    sqlx::query(
                        "INSERT INTO principal \
                           (tenant_id, region, principal_id, kind, data_role, status, \
                            profile_key_ref, profile_nonce, profile_ciphertext) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                         ON CONFLICT (tenant_id, region, principal_id) DO UPDATE SET \
                           kind = EXCLUDED.kind, data_role = EXCLUDED.data_role, \
                           status = EXCLUDED.status, profile_key_ref = EXCLUDED.profile_key_ref, \
                           profile_nonce = EXCLUDED.profile_nonce, \
                           profile_ciphertext = EXCLUDED.profile_ciphertext",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&row.principal_id)
                    .bind(&row.kind)
                    .bind(&row.data_role)
                    .bind(&row.status)
                    .bind(key_ref)
                    .bind(nonce)
                    .bind(ciphertext)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    sqlx::query(
                        "INSERT INTO credential_link (tenant_id, region, link_key, principal_id) \
                         VALUES ($1, $2, $3, $4) \
                         ON CONFLICT (tenant_id, region, link_key) DO UPDATE SET \
                           principal_id = EXCLUDED.principal_id",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&link_key)
                    .bind(&row.principal_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(())
                })
            })
            .await
    }

    pub async fn get_principal(
        &self,
        tenant: &str,
        principal_id: &str,
    ) -> Result<Option<DurablePrincipalRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let pid = principal_id.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT principal_id, kind, data_role, status, \
                                profile_key_ref, profile_nonce, profile_ciphertext \
                         FROM principal \
                         WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&pid)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    row.map(|row| row_to_principal(&row)).transpose()
                })
            })
            .await
    }

    pub async fn principals_in(
        &self,
        tenant: &str,
    ) -> Result<Vec<DurablePrincipalRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT principal_id, kind, data_role, status, \
                                profile_key_ref, profile_nonce, profile_ciphertext \
                         FROM principal WHERE tenant_id = $1 AND region = $2 \
                         ORDER BY principal_id",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    rows.iter().map(row_to_principal).collect()
                })
            })
            .await
    }

    pub async fn link_credential(
        &self,
        tenant: &str,
        link_key: &str,
        principal_id: &str,
    ) -> Result<bool, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let link = link_key.to_string();
        let pid = principal_id.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM principal \
                         WHERE tenant_id = $1 AND region = $2 AND principal_id = $3)",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&pid)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    if !exists {
                        return Ok(false);
                    }
                    sqlx::query(
                        "INSERT INTO credential_link (tenant_id, region, link_key, principal_id) \
                         VALUES ($1, $2, $3, $4) \
                         ON CONFLICT (tenant_id, region, link_key) DO UPDATE SET \
                           principal_id = EXCLUDED.principal_id",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&link)
                    .bind(&pid)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(true)
                })
            })
            .await
    }

    pub async fn resolve_credential(
        &self,
        tenant: &str,
        link_key: &str,
    ) -> Result<Option<DurablePrincipalRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let link = link_key.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT p.principal_id, p.kind, p.data_role, p.status, \
                                p.profile_key_ref, p.profile_nonce, p.profile_ciphertext \
                         FROM credential_link c \
                         JOIN principal p ON p.tenant_id = c.tenant_id \
                              AND p.region = c.region AND p.principal_id = c.principal_id \
                         WHERE c.tenant_id = $1 AND c.region = $2 AND c.link_key = $3",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&link)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    row.map(|row| row_to_principal(&row)).transpose()
                })
            })
            .await
    }
}

fn row_to_principal(r: &sqlx::postgres::PgRow) -> Result<DurablePrincipalRow, crate::pg::PgError> {
    let key_ref = r
        .try_get::<Option<String>, _>("profile_key_ref")
        .map_err(principal_row_decode)?;
    let nonce = r
        .try_get::<Option<Vec<u8>>, _>("profile_nonce")
        .map_err(principal_row_decode)?;
    let ciphertext = r
        .try_get::<Option<Vec<u8>>, _>("profile_ciphertext")
        .map_err(principal_row_decode)?;
    let profile = decode_profile(key_ref, nonce, ciphertext)?;
    Ok(DurablePrincipalRow {
        principal_id: r.try_get("principal_id").map_err(principal_row_decode)?,
        kind: r.try_get("kind").map_err(principal_row_decode)?,
        data_role: r.try_get("data_role").map_err(principal_row_decode)?,
        status: r.try_get("status").map_err(principal_row_decode)?,
        profile,
    })
}

fn decode_profile(
    key_ref: Option<String>,
    nonce: Option<Vec<u8>>,
    ciphertext: Option<Vec<u8>>,
) -> Result<Option<DurableProfileBlob>, crate::pg::PgError> {
    match (key_ref, nonce, ciphertext) {
        (None, None, None) => Ok(None),
        (Some(key_ref), Some(nonce), Some(ciphertext)) => Ok(Some(DurableProfileBlob {
            key_ref,
            nonce,
            ciphertext,
        })),
        _ => Err(crate::pg::PgError::Query(
            "principal row has an incomplete encrypted profile".to_string(),
        )),
    }
}

fn principal_row_decode(error: sqlx::Error) -> crate::pg::PgError {
    crate::pg::PgError::Query(format!("principal row decode failed: {error}"))
}

#[cfg(test)]
mod principal_decode_tests {
    use super::{decode_profile, ACTIVE_PRINCIPAL_STATUS_JSON, HUMAN_PRINCIPAL_KIND_JSON};

    #[test]
    fn durable_principal_tokens_match_the_identity_wire_contract() {
        assert_eq!(
            serde_json::to_string(&myelin_identity::PrincipalKind::Human).unwrap(),
            HUMAN_PRINCIPAL_KIND_JSON,
        );
        assert_eq!(
            serde_json::to_string(&myelin_identity::PrincipalStatus::Active).unwrap(),
            ACTIVE_PRINCIPAL_STATUS_JSON,
        );
    }

    #[test]
    fn encrypted_profile_columns_are_all_or_nothing() {
        assert_eq!(decode_profile(None, None, None).unwrap(), None);
        let profile = decode_profile(
            Some("kms://tenant/profile".to_string()),
            Some(vec![1, 2]),
            Some(vec![3, 4]),
        )
        .expect("a complete encrypted profile decodes")
        .expect("profile is present");
        assert_eq!(profile.nonce, vec![1, 2]);
        assert_eq!(profile.ciphertext, vec![3, 4]);

        for partial in [
            (Some("secret-key-ref".to_string()), None, None),
            (None, Some(vec![1]), Some(vec![2])),
            (Some("secret-key-ref".to_string()), Some(vec![1]), None),
        ] {
            let error = decode_profile(partial.0, partial.1, partial.2)
                .expect_err("a partial encrypted profile must fail closed");
            assert!(error
                .to_string()
                .contains("principal row has an incomplete encrypted profile"));
            assert!(!error.to_string().contains("secret-key-ref"));
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableRevocationRow {
    pub revoked_at: String,
    pub expires_at: Option<String>,
}

#[derive(Clone)]
pub struct DurableRevocationBacking {
    provider: SubstrateProvider,
}

impl DurableRevocationBacking {
    pub fn new(provider: SubstrateProvider) -> DurableRevocationBacking {
        DurableRevocationBacking { provider }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    pub async fn insert_revocation(
        &self,
        tenant: &str,
        kind: &str,
        handle: &str,
        revoked_at: &str,
        expires_at: Option<&str>,
    ) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let kind = kind.to_string();
        let handle = handle.to_string();
        let revoked_at = revoked_at.to_string();
        let expires_at = expires_at.map(|s| s.to_string());
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO revocation \
                           (tenant_id, region, kind, handle, revoked_at, expires_at) \
                         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&kind)
                    .bind(&handle)
                    .bind(&revoked_at)
                    .bind(expires_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(())
                })
            })
            .await
    }

    pub async fn insert_teardown(&self, tenant: &str, jti: &str) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let jti = jti.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO run_token_teardown (tenant_id, region, jti) \
                         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&jti)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(())
                })
            })
            .await
    }

    pub async fn get_revocation(
        &self,
        tenant: &str,
        kind: &str,
        handle: &str,
    ) -> Result<Option<DurableRevocationRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let kind = kind.to_string();
        let handle = handle.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT revoked_at, expires_at FROM revocation \
                         WHERE tenant_id = $1 AND region = $2 AND kind = $3 AND handle = $4",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&kind)
                    .bind(&handle)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    row.map(|row| {
                        Ok(DurableRevocationRow {
                            revoked_at: row.try_get("revoked_at").map_err(revocation_row_decode)?,
                            expires_at: row.try_get("expires_at").map_err(revocation_row_decode)?,
                        })
                    })
                    .transpose()
                })
            })
            .await
    }

    pub async fn is_teardown(&self, tenant: &str, jti: &str) -> Result<bool, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let jti = jti.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM run_token_teardown \
                         WHERE tenant_id = $1 AND region = $2 AND jti = $3)",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&jti)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(exists)
                })
            })
            .await
    }

    pub async fn count(&self, tenant: &str) -> Result<i64, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let n: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM revocation WHERE tenant_id = $1 AND region = $2",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(n)
                })
            })
            .await
    }
}

fn revocation_row_decode(error: sqlx::Error) -> crate::pg::PgError {
    crate::pg::PgError::Query(format!("revocation row decode failed: {error}"))
}

#[derive(Clone)]
pub struct DurableReplayBacking {
    provider: SubstrateProvider,
}

impl DurableReplayBacking {
    pub fn new(provider: SubstrateProvider) -> DurableReplayBacking {
        DurableReplayBacking { provider }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    pub async fn consume(
        &self,
        tenant: &str,
        namespace: &str,
        replay_id: &str,
        expires_at: i64,
        now: i64,
    ) -> Result<bool, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let namespace = namespace.to_string();
        let replay_id = replay_id.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "DELETE FROM auth_replay \
                         WHERE tenant_id = $1 AND region = $2 AND expires_at < $3",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(now)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;

                    let result = sqlx::query(
                        "INSERT INTO auth_replay \
                           (tenant_id, region, namespace, replay_id, expires_at) \
                         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&namespace)
                    .bind(&replay_id)
                    .bind(expires_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(result.rows_affected() == 1)
                })
            })
            .await
    }
}
