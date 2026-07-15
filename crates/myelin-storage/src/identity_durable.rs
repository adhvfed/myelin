//! # Durable PG backings for the identity S1 principal + S3 tuple stores (MR-007, the P-522 floor)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §2 (the S1 principal
//! store: principals + KMS-encrypted profile PII + SSO/SCIM credential links, `(tenant, region)`
//! shard, RLS one-tenant blast radius) + §6 (the S3 ReBAC tuple store). Closes census SI-018/SI-019
//! at the *production* level: the identity-layer `PrincipalStore`/`TupleStore` were in-memory
//! `HashMap`s (the make-it-real shortcut); this module is the REAL durable backing they delegate to.
//!
//! ## What this is (the reuse story — EXTEND, never fork)
//! - **Tuples** reuse the EXISTING `rebac_tuple` table + ops ([`crate::pg::PgStore`],
//!   [`crate::pg::REBAC_TUPLE_MIGRATION`], the `myelin_tenant_isolation` FORCE-RLS policy). The
//!   [`DurableTupleBacking`] drives the conn-bound rebac ops (`insert_tuple_on_conn` /
//!   `delete_tuple_on_conn` / `tuples_on_conn`) THROUGH the MR-022 [`crate::provider::SubstrateProvider::with_tenant_tx`]
//!   convention (acquire → BEGIN → `SET LOCAL (tenant, region)` → op → COMMIT, reset-on-release) —
//!   so RLS applies and no GUC bleeds. There is NO second tuple table or pool.
//! - **Principals** are NEW (the `principal` + `credential_link` tables did not exist in `pg.rs`).
//!   This module adds them as forward-only migrations following the SAME RLS form `pg.rs` uses
//!   (`tenant_id`/`region`, ENABLE + FORCE ROW LEVEL SECURITY, the `myelin_tenant_isolation`
//!   policy keyed on `current_setting('myelin.tenant_id'/'myelin.region', true)`). The
//!   [`DurablePrincipalBacking`] persists/reads them through the SAME `with_tenant_tx` convention.
//!
//! ## The KMS boundary (MR-025, respected)
//! Profile PII is KMS-encrypted by the identity layer; this backing persists only the OPAQUE
//! ciphertext blob (`profile_key_ref` + `profile_nonce` + `profile_ciphertext`). It never touches a
//! key. Decrypt-across-process-restart depends on the durable KMS root (MR-025), which is not done
//! yet — so this module's durability is scoped to the principal ROW + the ciphertext bytes; the
//! full profile-decrypt-across-restart proof is MR-009's job (after MR-025).
//!
//! Feature-gated `integration` (it pulls the real sqlx client), like the rest of the live-PG code.

use sqlx::Row;

use myelin_events::EventEnvelope;

use crate::migration::{Migration, Migrations};
use crate::pg::PgStore;
use crate::pgrelay::PgRelay;
use crate::provider::{ProviderError, SubstrateProvider};

// =================================================================================================
// Migrations — the identity durable stores' schema (rebac_tuple reused; principal/credential NEW).
// =================================================================================================

/// The S1 `principal` table — `(tenant, region)`-scoped, RLS-ready, following the EXACT `rebac_tuple`
/// form. The opaque-stable `principal_id` is the PK attribution key; `kind`/`data_role`/`status` are
/// the §3/§2.1/§11 governance columns (stored as the identity layer's serde-JSON text so the
/// polymorphic `Agent{..}` kind round-trips). The profile PII is the at-rest KMS-sealed blob
/// (`profile_key_ref` names the per-subject DEK; `profile_nonce`/`profile_ciphertext` are the
/// AES-256-GCM seal) — all NULL for a principal with no profile (e.g. a machine/service principal).
/// Forward-only / expand-only (`IF NOT EXISTS`); `tenant_id`/`region` are what the RLS policy keys on.
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

/// The `(tenant, region)` FORCE-RLS policy on `principal` — the SAME shape `pg.rs` installs on
/// `rebac_tuple` (`myelin_tenant_isolation`, USING + WITH CHECK keyed on the session GUCs). The
/// `DROP POLICY IF EXISTS` makes the CREATE idempotent (forward-only-legal: it drops a POLICY, never
/// a table/column). Under the migrator's advisory lock this runs serialized + exactly once.
pub const PRINCIPAL_RLS_POLICY: &str = "\
ALTER TABLE principal ENABLE ROW LEVEL SECURITY;
ALTER TABLE principal FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON principal;
CREATE POLICY myelin_tenant_isolation ON principal \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

/// The S7 `revocation` mirror table (MR-008, SI-020) — `(tenant, region)`-scoped, RLS-ready, the
/// durable recovery source of truth the identity `RevocationStore` mirror models. Holds revoked
/// `jti`s + suspended/disabled principals + per-run agent-token TTLs. `(kind, handle)` is the entry
/// identity an idempotent re-revoke collapses onto (`ON CONFLICT DO NOTHING` preserves the FIRST
/// `revoked_at`). `expires_at` is the per-run TTL (`expires_at == run-life`) that survives restart —
/// the auto-expire (defence-in-depth for revoke-on-crash). Timestamps are RFC3339 text (lexical
/// order == chronological, the same convention the tuple-store zookie + audit chain use).
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

/// The `(tenant, region)` FORCE-RLS policy on `revocation` (same form as `principal`).
pub const REVOCATION_RLS_POLICY: &str = "\
ALTER TABLE revocation ENABLE ROW LEVEL SECURITY;
ALTER TABLE revocation FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON revocation;
CREATE POLICY myelin_tenant_isolation ON revocation \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

/// The S7 `run_token_teardown` table (MR-008) — the per-run-token EXPLICIT-teardown set, kept
/// distinct from the TTL `revocation` row so the two run-token deaths stay distinguishable:
/// **torn-down** (an explicit teardown landed → the immediate deny) vs **expired** (the TTL passed
/// with no teardown → the auto-expire). `(tenant, region)`-scoped + RLS. PII-free (an opaque `jti`).
pub const RUN_TOKEN_TEARDOWN_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS run_token_teardown (
    tenant_id text NOT NULL,
    region    text NOT NULL,
    jti       text NOT NULL,
    PRIMARY KEY (tenant_id, region, jti)
);";

/// The `(tenant, region)` FORCE-RLS policy on `run_token_teardown` (same form).
pub const RUN_TOKEN_TEARDOWN_RLS_POLICY: &str = "\
ALTER TABLE run_token_teardown ENABLE ROW LEVEL SECURITY;
ALTER TABLE run_token_teardown FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON run_token_teardown;
CREATE POLICY myelin_tenant_isolation ON run_token_teardown \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

/// The S1 `credential_link` table — the verified-credential `(scheme, subject_key)` → `principal_id`
/// index `authenticate` keys on (identity §2 "SSO/SCIM links"). `(tenant, region)`-scoped + RLS so a
/// credential verified for tenant A can never resolve a principal in tenant B. `link_key` is the
/// identity layer's `"<scheme>\x1f<subject_key>"` join key.
pub const CREDENTIAL_LINK_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS credential_link (
    tenant_id    text NOT NULL,
    region       text NOT NULL,
    link_key     text NOT NULL,
    principal_id text NOT NULL,
    PRIMARY KEY (tenant_id, region, link_key)
);";

/// The `(tenant, region)` FORCE-RLS policy on `credential_link` (same form as `principal`).
pub const CREDENTIAL_LINK_RLS_POLICY: &str = "\
ALTER TABLE credential_link ENABLE ROW LEVEL SECURITY;
ALTER TABLE credential_link FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON credential_link;
CREATE POLICY myelin_tenant_isolation ON credential_link \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

/// The rebac_tuple RLS policy — re-stated here as a const so the identity durable-store migration set
/// is self-contained (the SAME policy `pg.rs::PgStore::migrate` installs inline; identical DDL ⇒
/// idempotent under the version table + advisory lock — never a second policy).
pub const REBAC_TUPLE_RLS_POLICY: &str = "\
ALTER TABLE rebac_tuple ENABLE ROW LEVEL SECURITY;
ALTER TABLE rebac_tuple FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON rebac_tuple;
CREATE POLICY myelin_tenant_isolation ON rebac_tuple \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

/// The full forward-only migration set the identity durable stores bind to: the (reused)
/// `rebac_tuple` table + RLS, and the (new) `principal` + `credential_link` tables + RLS. Applied via
/// the MR-022 [`crate::provider::SubstrateProvider::migrate`] (validate → execute, race-safe,
/// version-recorded) at boot. The ids are stable (`0010_*`..`0015_*`) and idempotent on re-boot.
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

// =================================================================================================
// DurableTupleBacking — the S3 edge set over rebac_tuple via with_tenant_tx (reuse, MR-007).
// =================================================================================================

/// A single tuple-edge delta to apply atomically (the `Add`/`Remove` of `object#relation@subject`).
#[derive(Clone, Debug)]
pub enum TupleEdgeOp {
    /// Add (idempotent) the edge.
    Add,
    /// Remove the edge.
    Remove,
}

/// The REAL durable S3 backing: the `rebac_tuple` edge set, accessed THROUGH the MR-022
/// `with_tenant_tx` convention so every op is `(tenant, region)`-RLS-scoped with no GUC bleed.
/// Cloneable (the provider/pool is an `Arc`-backed handle).
#[derive(Clone)]
pub struct DurableTupleBacking {
    provider: SubstrateProvider,
}

impl DurableTupleBacking {
    /// Build the backing over the MR-022 provider (the app-role, reset-on-release pool).
    pub fn new(provider: SubstrateProvider) -> DurableTupleBacking {
        DurableTupleBacking { provider }
    }

    /// **Apply a batch of edge deltas AND co-commit the `iam.tuple_written` outbox row in ONE
    /// tenant-scoped transaction (BUS-2 emit-iff-committed, made EXACT for the durable S3 spine —
    /// MR-009b W3b.3).** The tuple INSERT/DELETEs and the outbox-row insert
    /// ([`PgRelay::co_commit_in_tx`], the one sanctioned outbox-write site) run in the SAME open
    /// sqlx transaction the `with_tenant_tx` convention opens: either all deltas + the event commit,
    /// or the whole tx rolls back. This REPLACES the former non-atomic pattern (durable tuple write
    /// in this PG tx, then a SEPARATE — typically in-memory — `OutboxStore` emit), whose crash
    /// window could lose the event or ghost it. A crash mid-sequence now leaves EITHER the tuple +
    /// its event OR neither (0 ghost / 0 lost).
    ///
    /// The `envelope` (its causality + stable `event_id`) is DERIVED AT THE EMIT POINT in
    /// `myelin-identity-service` (the `OutboxTransaction::emit` seam mints/derives it); this backing
    /// only makes the already-derived row durable in the tuple tx — the chat `append_co_commit`
    /// precedent (`myelin-chat/src/store/pg.rs`) applied at the S3 backing layer. `aggregate` is the
    /// per-object ordering key the identity layer stamped on the draft; the `(object, relation,
    /// subject)` are owned so the future is `Send`.
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
                    // The iam.tuple_written outbox row, in the SAME transaction as the tuple deltas
                    // above — both commit or both roll back (emit-iff-committed). The relay owns the
                    // outbox table, so the INSERT lives in PgRelay (the one lint-excluded outbox-write
                    // site), never hand-rolled here.
                    PgRelay::co_commit_in_tx(conn, &aggregate_owned, &envelope_owned).await?;
                    Ok(())
                })
            })
            .await
    }

    /// Every `(object, relation, subject)` edge durably in the `(tenant, region)` partition (the
    /// `tuples_in` read), RLS-scoped through the convention — a read for one tenant structurally
    /// cannot reach another's rows.
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

// =================================================================================================
// DurablePrincipalBacking — the S1 principal + credential_link tables via with_tenant_tx (MR-007).
// =================================================================================================

/// The at-rest KMS-sealed profile blob (the identity layer owns the keys; this is only ciphertext).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableProfileBlob {
    /// The `PiiKeyRef` URI of the per-subject DEK that sealed the profile (opaque to this layer).
    pub key_ref: String,
    /// The AES-256-GCM nonce.
    pub nonce: Vec<u8>,
    /// The AES-256-GCM ciphertext.
    pub ciphertext: Vec<u8>,
}

/// A durable principal row, with governance columns as opaque text (the identity layer serdes the
/// polymorphic kind/role/status) and the profile as the optional sealed blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurablePrincipalRow {
    /// The opaque-stable attribution id (the PK within `(tenant, region)`).
    pub principal_id: String,
    /// The serde-JSON `PrincipalKind` (opaque to this layer).
    pub kind: String,
    /// The serde-JSON `DataRole`.
    pub data_role: String,
    /// The serde-JSON `PrincipalStatus`.
    pub status: String,
    /// The sealed profile PII, or `None` for a principal with no profile.
    pub profile: Option<DurableProfileBlob>,
}

/// The REAL durable S1 backing: the `principal` + `credential_link` tables, accessed through the
/// MR-022 `with_tenant_tx` convention (RLS-scoped, no GUC bleed). Cloneable.
#[derive(Clone)]
pub struct DurablePrincipalBacking {
    provider: SubstrateProvider,
}

impl DurablePrincipalBacking {
    /// Build the backing over the MR-022 provider (the app-role, reset-on-release pool).
    pub fn new(provider: SubstrateProvider) -> DurablePrincipalBacking {
        DurablePrincipalBacking { provider }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    /// Upsert a principal row + (optional) sealed profile blob in its `(tenant, region)` partition
    /// (the `put_principal` durable write). Re-writing the same `principal_id` updates the row.
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

    /// Read a single principal row (+ profile blob) in its `(tenant, region)` partition, or `None`.
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
                    Ok(row.map(|r| row_to_principal(&r)))
                })
            })
            .await
    }

    /// Every principal row in the `(tenant, region)` partition (the `principals_in` directory read).
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
                    Ok(rows.iter().map(row_to_principal).collect::<Vec<_>>())
                })
            })
            .await
    }

    /// Link a verified credential `link_key` → `principal_id` in the `(tenant, region)` partition.
    /// Returns `false` (and writes nothing) if the principal does not exist in this partition (a
    /// dangling SSO/SCIM link is refused). Idempotent on the same `link_key`.
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

    /// Resolve a verified credential `link_key` to its principal row in the `(tenant, region)`
    /// partition, or `None` if no such link exists.
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
                    Ok(row.map(|r| row_to_principal(&r)))
                })
            })
            .await
    }
}

/// Map a `principal`-shaped row to a [`DurablePrincipalRow`] (the profile is `Some` iff a key_ref is
/// present — the §X-7 split: the row's attribution columns are always there, the profile is separable).
fn row_to_principal(r: &sqlx::postgres::PgRow) -> DurablePrincipalRow {
    let key_ref: Option<String> = r.get("profile_key_ref");
    let profile = key_ref.map(|key_ref| DurableProfileBlob {
        key_ref,
        nonce: r.get::<Option<Vec<u8>>, _>("profile_nonce").unwrap_or_default(),
        ciphertext: r
            .get::<Option<Vec<u8>>, _>("profile_ciphertext")
            .unwrap_or_default(),
    });
    DurablePrincipalRow {
        principal_id: r.get("principal_id"),
        kind: r.get("kind"),
        data_role: r.get("data_role"),
        status: r.get("status"),
        profile,
    }
}

// =================================================================================================
// DurableRevocationBacking — the S7 revocation + run-token-TTL/teardown over with_tenant_tx (MR-008).
// =================================================================================================

/// A durable revocation entry as read back (the fields the consult needs). `expires_at` is the per-run
/// TTL (`None` for a permanent revocation / suspended principal); the consult honours `now < expires_at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableRevocationRow {
    /// When it was FIRST revoked (preserved across an idempotent re-revoke).
    pub revoked_at: String,
    /// The per-run TTL (`expires_at == run-life`), or `None` for a permanent revocation.
    pub expires_at: Option<String>,
}

/// The REAL durable S7 backing (MR-008): the `revocation` mirror + `run_token_teardown` set, accessed
/// THROUGH the MR-022 `with_tenant_tx` convention (RLS-scoped, no GUC bleed). The durable table IS the
/// recovery source of truth — on this path the "fast Redis/Valkey layer" collapses into the DB itself
/// (reads hit the durable table directly; there is nothing to lose + rebuild). Cloneable.
#[derive(Clone)]
pub struct DurableRevocationBacking {
    provider: SubstrateProvider,
}

impl DurableRevocationBacking {
    /// Build the backing over the MR-022 provider (the app-role, reset-on-release pool).
    pub fn new(provider: SubstrateProvider) -> DurableRevocationBacking {
        DurableRevocationBacking { provider }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    /// Insert a revocation entry (idempotent — `ON CONFLICT DO NOTHING` preserves the FIRST
    /// `revoked_at`, the crash-safe idempotency contract 4.7). `kind` is `"jti"`/`"principal"`.
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

    /// Record an explicit run-token teardown (idempotent).
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

    /// Read a single revocation entry (`None` if absent) in the `(tenant, region)` partition.
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
                    Ok(row.map(|r| DurableRevocationRow {
                        revoked_at: r.get("revoked_at"),
                        expires_at: r.get("expires_at"),
                    }))
                })
            })
            .await
    }

    /// Whether an explicit teardown exists for `jti` in the `(tenant, region)` partition.
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

    /// The count of distinct revocations in the `(tenant, region)` partition (the idempotency/drill
    /// assertion — a double-revoke must NOT grow this).
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
