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

use crate::migration::{Migration, Migrations};
use crate::pg::PgStore;
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

    /// Apply a batch of edge deltas ATOMICALLY in ONE tenant-scoped transaction (the `write_tuples`
    /// durable apply). Either all deltas commit or none (the tx rolls back on any error). The
    /// `(object, relation, subject)` are owned so the future is `Send`.
    pub async fn apply_deltas(
        &self,
        tenant: &str,
        region: &str,
        deltas: Vec<(TupleEdgeOp, String, String, String)>,
    ) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region_owned = region.to_string();
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
