//! # Durable PG backings for the identity S2 pseudonym map + the PII-free erasure ledger (MR-009b W6a)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §2 (the **S2** row: the
//! `real_identity ↔ per-tenant pseudonym` map; per-SUBJECT key = the erasure lever; `(tenant, region)`
//! shard; **tightest RLS**) + §11/§12 (the **P-ID-20** crypto-shred + the **ID-D8** post-restore
//! re-erasure the PII-free erasure ledger drives). Closes the W6a leg of the census SI-018 cluster at
//! the *production* level: the S2 [`PseudonymStore`](../../myelin-identity-service) + the
//! [`PseudonymErasureLedger`] were in-memory `HashMap`/`BTreeMap`s (the make-it-real shortcut); this
//! module is the REAL durable backing they delegate to.
//!
//! ## The two tables (opposite erasure postures — the load-bearing distinction)
//! - **`pseudonym_map`** (migrations `0020`/`0021`) — the S2 store's `(tenant, region)`-partitioned,
//!   **tightest-RLS** mapping row: `principal_id` PK, the PUBLIC `pseudonym_render` (PII-free,
//!   survives erasure), and the at-rest **KMS-sealed** real-identity link (`real_id_key_ref` +
//!   `nonce` + `ciphertext`). This module persists ONLY the opaque ciphertext (the per-subject DEK
//!   stays in the KMS — MR-025 boundary); a crypto-shred DELETEs the whole row (row + sealed link +
//!   the reverse-lookup path in one shot). A reverse index on `(tenant_id, region, pseudonym_render)`
//!   serves the `resolve_pseudonym` reverse direction. RLS is the SAME FORCE-RLS form `pg.rs`
//!   installs on `rebac_tuple`/`principal`.
//! - **`identity_pseudonym_erasure_ledger`** (migration `0022`) — the PII-free record of every
//!   per-subject erasure (contract 10.8), keyed `(tenant, region, subject)`. **NON-shred-erasable by
//!   construction:** it holds ONLY the OPAQUE principal id + the dek class + the date — never the real
//!   identity — so it is PII-free and needs NO crypto-shred lever, and — crucially — it has **NO RLS
//!   policy**: it must SURVIVE the very key destruction it records AND survive a restore, so the
//!   post-restore re-erasure pass (ID-D8) can replay it. Idempotent upsert (`ON CONFLICT … DO UPDATE
//!   SET erased_at`) — a re-erase updates the date, never duplicates. Partition isolation is the
//!   explicit `(tenant_id, region)` predicate on every statement (there is no cross-tenant read path).
//!
//! Compiled UNCONDITIONALLY (durable-by-default, MR-009b): this module is the always-compiled
//! production backing; `integration` remains a test-selector only.

use sqlx::Row;

use crate::migration::{Migration, Migrations};
use crate::provider::{ProviderError, SubstrateProvider};

// =================================================================================================
// Migrations — the S2 pseudonym-map (0020/0021, RLS) + the PII-free erasure ledger (0022, NO RLS).
// =================================================================================================

/// The S2 `pseudonym_map` table — `(tenant, region)`-scoped, RLS-ready, following the EXACT
/// `principal`/`rebac_tuple` form. `principal_id` is the opaque subject PK; `pseudonym_render` is the
/// PUBLIC per-tenant handle (the frozen `<pseudonym>@<tenant>.noreply` grammar — PII-free, survives
/// erasure); `real_id_key_ref`/`nonce`/`ciphertext` are the at-rest KMS-sealed real-identity link (the
/// per-subject-DEK crypto-shred unit — only the ciphertext rests here, never a key). Forward-only /
/// expand-only (`IF NOT EXISTS`); `tenant_id`/`region` are what the RLS policy keys on.
pub const PSEUDONYM_MAP_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS pseudonym_map (
    tenant_id        text  NOT NULL,
    region           text  NOT NULL,
    principal_id     text  NOT NULL,
    pseudonym_render text  NOT NULL,
    real_id_key_ref  text  NOT NULL,
    nonce            bytea NOT NULL,
    ciphertext       bytea NOT NULL,
    PRIMARY KEY (tenant_id, region, principal_id)
);
CREATE INDEX IF NOT EXISTS pseudonym_map_reverse \
  ON pseudonym_map (tenant_id, region, pseudonym_render);";

/// The `(tenant, region)` FORCE-RLS policy on `pseudonym_map` — the SAME shape `pg.rs` installs on
/// `rebac_tuple` (`myelin_tenant_isolation`, USING + WITH CHECK keyed on the session GUCs). This is
/// the TIGHTEST-RLS table (§2): a read for one tenant structurally cannot reach another's mappings.
/// The `DROP POLICY IF EXISTS` makes the CREATE idempotent (forward-only-legal: it drops a POLICY,
/// never a table/column). Under the migrator's advisory lock this runs serialized + exactly once.
pub const PSEUDONYM_MAP_RLS_POLICY: &str = "\
ALTER TABLE pseudonym_map ENABLE ROW LEVEL SECURITY;
ALTER TABLE pseudonym_map FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS myelin_tenant_isolation ON pseudonym_map;
CREATE POLICY myelin_tenant_isolation ON pseudonym_map \
  USING (tenant_id = current_setting('myelin.tenant_id', true) \
         AND region = current_setting('myelin.region', true)) \
  WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
              AND region = current_setting('myelin.region', true));";

/// The PII-free per-subject erasure ledger (contract 10.8) — `(tenant, region, subject)`-keyed. Holds
/// ONLY the OPAQUE principal id + the per-subject DEK class token (`subject:<id>`) + the erase date —
/// **never** the real identity (which is the thing being shredded). **NON-shred-erasable + NO RLS by
/// construction:** it must SURVIVE the key destruction it records AND survive a restore, so
/// post-restore re-erasure (ID-D8) can replay it against a resurrected pre-erasure backup — a
/// crypto-shred/RLS lever on THIS table would defeat that. Partition isolation is the explicit
/// `(tenant_id, region)` predicate on every statement. Forward-only (`IF NOT EXISTS`).
pub const PSEUDONYM_ERASURE_LEDGER_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS identity_pseudonym_erasure_ledger (
    tenant_id text NOT NULL,
    region    text NOT NULL,
    subject   text NOT NULL,
    dek_class text NOT NULL,
    erased_at text NOT NULL,
    PRIMARY KEY (tenant_id, region, subject)
);";

/// The forward-only migration set the S2 pseudonym durable stores bind to: the (new) `pseudonym_map`
/// table + its tightest-RLS policy, and the (new) PII-free `identity_pseudonym_erasure_ledger` (NO
/// RLS — non-shred-erasable). Applied via the MR-022 [`crate::provider::SubstrateProvider::migrate`]
/// (validate → execute, race-safe, version-recorded) at boot. The ids are stable (`0020_*`..`0022_*`,
/// in the free identity range) and idempotent on re-boot.
pub fn pseudonym_durable_migrations() -> Migrations {
    Migrations::of([
        Migration::plain("0020_pseudonym_map", PSEUDONYM_MAP_MIGRATION),
        Migration::plain("0021_pseudonym_map_rls", PSEUDONYM_MAP_RLS_POLICY),
        Migration::plain(
            "0022_identity_pseudonym_erasure_ledger",
            PSEUDONYM_ERASURE_LEDGER_MIGRATION,
        ),
    ])
}

// =================================================================================================
// DurablePseudonymBacking — the S2 pseudonym_map over with_tenant_tx (RLS-scoped, MR-009b W6a).
// =================================================================================================

/// A durable S2 mapping row. `pseudonym_render` is the PUBLIC handle rendering (PII-free); the
/// `real_id_key_ref`/`nonce`/`ciphertext` are the at-rest KMS-sealed real-identity link (the identity
/// layer owns the keys; this is only ciphertext).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurablePseudonymRow {
    /// The opaque subject id (the PK within `(tenant, region)`).
    pub principal_id: String,
    /// The PUBLIC per-tenant pseudonym rendering (`<pseudonym>@<tenant>.noreply`).
    pub pseudonym_render: String,
    /// The `PiiKeyRef` URI of the per-subject DEK that sealed the real-identity link (opaque here).
    pub real_id_key_ref: String,
    /// The AES-256-GCM nonce of the sealed real-identity link.
    pub nonce: Vec<u8>,
    /// The AES-256-GCM ciphertext of the sealed real-identity link.
    pub ciphertext: Vec<u8>,
}

/// The REAL durable S2 backing: the `pseudonym_map` table, accessed THROUGH the MR-022
/// `with_tenant_tx` convention so every op is `(tenant, region)`-RLS-scoped with no GUC bleed.
/// Cloneable (the provider/pool is an `Arc`-backed handle).
#[derive(Clone)]
pub struct DurablePseudonymBacking {
    provider: SubstrateProvider,
}

impl DurablePseudonymBacking {
    /// Build the backing over the MR-022 provider (the app-role, reset-on-release pool).
    pub fn new(provider: SubstrateProvider) -> DurablePseudonymBacking {
        DurablePseudonymBacking { provider }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    /// Upsert a mapping row in its `(tenant, region)` partition (the `put_mapping` durable write).
    /// Re-writing the same `principal_id` updates the row (the reverse-lookup index tracks
    /// `pseudonym_render`). RLS-scoped: a write for one tenant structurally cannot land in another's.
    pub async fn put_mapping(
        &self,
        tenant: &str,
        row: DurablePseudonymRow,
    ) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO pseudonym_map \
                           (tenant_id, region, principal_id, pseudonym_render, \
                            real_id_key_ref, nonce, ciphertext) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7) \
                         ON CONFLICT (tenant_id, region, principal_id) DO UPDATE SET \
                           pseudonym_render = EXCLUDED.pseudonym_render, \
                           real_id_key_ref = EXCLUDED.real_id_key_ref, \
                           nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&row.principal_id)
                    .bind(&row.pseudonym_render)
                    .bind(&row.real_id_key_ref)
                    .bind(&row.nonce)
                    .bind(&row.ciphertext)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(())
                })
            })
            .await
    }

    /// Read a mapping row by opaque subject id in its `(tenant, region)` partition, or `None` (the
    /// forward `subject → row` direction: `mapping_of` / `resolve_subject`).
    pub async fn get_by_principal(
        &self,
        tenant: &str,
        principal_id: &str,
    ) -> Result<Option<DurablePseudonymRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let pid = principal_id.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT principal_id, pseudonym_render, real_id_key_ref, nonce, ciphertext \
                         FROM pseudonym_map \
                         WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&pid)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(row.map(|r| row_to_pseudonym(&r)))
                })
            })
            .await
    }

    /// Read a mapping row by PUBLIC pseudonym rendering in its `(tenant, region)` partition, or `None`
    /// (the reverse `pseudonym → row` direction `resolve_pseudonym` keys on; served by the reverse
    /// index). Per-partition only — a resolve for one tenant cannot reach another's handles.
    pub async fn get_by_pseudonym(
        &self,
        tenant: &str,
        pseudonym_render: &str,
    ) -> Result<Option<DurablePseudonymRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let rendering = pseudonym_render.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT principal_id, pseudonym_render, real_id_key_ref, nonce, ciphertext \
                         FROM pseudonym_map \
                         WHERE tenant_id = $1 AND region = $2 AND pseudonym_render = $3",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&rendering)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(row.map(|r| row_to_pseudonym(&r)))
                })
            })
            .await
    }

    /// Every mapping row in the `(tenant, region)` partition (the `mappings_in` directory read).
    pub async fn mappings_in(
        &self,
        tenant: &str,
    ) -> Result<Vec<DurablePseudonymRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT principal_id, pseudonym_render, real_id_key_ref, nonce, ciphertext \
                         FROM pseudonym_map WHERE tenant_id = $1 AND region = $2 \
                         ORDER BY principal_id",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(rows.iter().map(row_to_pseudonym).collect::<Vec<_>>())
                })
            })
            .await
    }

    /// **Crypto-shred a subject's mapping row (the `shred_row` durable half, P-ID-20).** DELETEs the
    /// whole row — the public row + the sealed real-identity link + (via the row's disappearance) the
    /// reverse-lookup path — in its `(tenant, region)` partition, under one tenant-scoped tx. Returns
    /// `true` iff a row was present to shred (idempotent: a re-shred deletes nothing → `false`).
    pub async fn shred(&self, tenant: &str, principal_id: &str) -> Result<bool, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let pid = principal_id.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let result = sqlx::query(
                        "DELETE FROM pseudonym_map \
                         WHERE tenant_id = $1 AND region = $2 AND principal_id = $3",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&pid)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(result.rows_affected() > 0)
                })
            })
            .await
    }
}

/// Map a `pseudonym_map`-shaped row to a [`DurablePseudonymRow`].
fn row_to_pseudonym(r: &sqlx::postgres::PgRow) -> DurablePseudonymRow {
    DurablePseudonymRow {
        principal_id: r.get("principal_id"),
        pseudonym_render: r.get("pseudonym_render"),
        real_id_key_ref: r.get("real_id_key_ref"),
        nonce: r.get("nonce"),
        ciphertext: r.get("ciphertext"),
    }
}

// =================================================================================================
// DurableErasureLedgerBacking — the PII-free erasure ledger (NO RLS, non-shred-erasable, W6a).
// =================================================================================================

/// A durable PII-free erasure-ledger row (contract 10.8). The OPAQUE subject id + the per-subject DEK
/// class token (`subject:<id>`, so re-erasure can re-destroy exactly the right key without re-reading
/// the shredded map row) + the erase date — never the real identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableErasureLedgerRow {
    /// The opaque principal id erased (PII-free).
    pub subject: String,
    /// The per-subject DEK class token to (re-)destroy (`subject:<id>`).
    pub dek_class: String,
    /// The erase timestamp (RFC3339 text, lexical == chronological).
    pub erased_at: String,
}

/// The REAL durable PII-free erasure ledger backing (contract 10.8): the
/// `identity_pseudonym_erasure_ledger` table, accessed through the MR-022 `with_tenant_tx` convention
/// for connection consistency. **NON-shred-erasable + NO RLS:** the table carries NO RLS policy and NO
/// ciphertext — it must survive the crypto-shred it records + survive a restore so the ID-D8
/// re-erasure pass can replay it. Partition isolation is the explicit `(tenant_id, region)` predicate
/// on every statement. Cloneable.
#[derive(Clone)]
pub struct DurableErasureLedgerBacking {
    provider: SubstrateProvider,
}

impl DurableErasureLedgerBacking {
    /// Build the backing over the MR-022 provider (the app-role, reset-on-release pool).
    pub fn new(provider: SubstrateProvider) -> DurableErasureLedgerBacking {
        DurableErasureLedgerBacking { provider }
    }

    fn region(&self) -> String {
        self.provider.config().region.clone()
    }

    /// Record an erasure (10.8) — PII-free, idempotent. The `ON CONFLICT (tenant, region, subject) DO
    /// UPDATE SET erased_at` collapses a re-erase onto the same row (updates the date, never
    /// duplicates — the post-restore re-erasure replay path).
    pub async fn record(
        &self,
        tenant: &str,
        subject: &str,
        dek_class: &str,
        erased_at: &str,
    ) -> Result<(), ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let subject = subject.to_string();
        let dek_class = dek_class.to_string();
        let erased_at = erased_at.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    sqlx::query(
                        "INSERT INTO identity_pseudonym_erasure_ledger \
                           (tenant_id, region, subject, dek_class, erased_at) \
                         VALUES ($1, $2, $3, $4, $5) \
                         ON CONFLICT (tenant_id, region, subject) DO UPDATE SET \
                           dek_class = EXCLUDED.dek_class, erased_at = EXCLUDED.erased_at",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&subject)
                    .bind(&dek_class)
                    .bind(&erased_at)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(())
                })
            })
            .await
    }

    /// Every erasure entry in the `(tenant, region)` partition (the re-erasure pass replays THIS).
    pub async fn entries_in(
        &self,
        tenant: &str,
    ) -> Result<Vec<DurableErasureLedgerRow>, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let rows = sqlx::query(
                        "SELECT subject, dek_class, erased_at \
                         FROM identity_pseudonym_erasure_ledger \
                         WHERE tenant_id = $1 AND region = $2 ORDER BY subject",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(rows
                        .iter()
                        .map(|r| DurableErasureLedgerRow {
                            subject: r.get("subject"),
                            dek_class: r.get("dek_class"),
                            erased_at: r.get("erased_at"),
                        })
                        .collect::<Vec<_>>())
                })
            })
            .await
    }

    /// Whether the subject is recorded erased in the `(tenant, region)` partition (the ledger
    /// remembers an erasure even after the map row + DEK are gone — the load-bearing 10.8 property).
    pub async fn is_erased(&self, tenant: &str, subject: &str) -> Result<bool, ProviderError> {
        let tenant_owned = tenant.to_string();
        let region = self.region();
        let subject = subject.to_string();
        self.provider
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM identity_pseudonym_erasure_ledger \
                         WHERE tenant_id = $1 AND region = $2 AND subject = $3)",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&subject)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| crate::pg::PgError::Query(e.to_string()))?;
                    Ok(exists)
                })
            })
            .await
    }
}
