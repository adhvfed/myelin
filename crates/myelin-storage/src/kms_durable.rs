//! # Durable PG backing for the KMS cell root + KEKs/DEKs (MR-025 / SI-006 — the software-sealed floor)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §4 (the three-level envelope
//! hierarchy — L0 per-cell root sealed/never-exported, L1 per-(tenant,region) KEK, L2 DEKs) + §7.5
//! (a crypto-shredded key stays dead across a restore). Closes census SI-006 ("`KmsEngine::new()`
//! mints a FRESH `CellRoot::generate()` per process → after a restart NO encrypted column can be
//! decrypted, because the root that wrapped every KEK is gone") at the *durable* level.
//!
//! ## Anti-duplication — this EXTENDS [`crate::kms`]; it does NOT fork a second KMS
//! All crypto lives in [`crate::kms`] (the vetted RustCrypto AES-256-GCM AEAD): the seal/unseal of
//! the root ([`CellRoot::seal`]/[`CellRoot::unseal`]), the KEK/DEK wrap, and the export/install
//! seams. This module is ONLY the durable STORE (the PG tables + the load/persist plumbing) — exactly
//! the additive shape MR-006 / SI-006 called for ("add a load-or-generate root-origin constructor +
//! extend backup_snapshot to carry the sealed root + wrapped KEKs"). There is ONE `KmsEngine`.
//!
//! ## The software-sealed root-of-trust (NOT HSM — that stays Tier-4)
//! The L0 cell root cannot rest in plaintext. On this software floor the root-of-trust is a **seal
//! key supplied at boot from the environment** (`MYELIN_KMS_SEAL_KEY`, a 32-byte key as 64 hex chars
//! — the operator-held unseal key, the software analogue of an HSM). The durable store holds the
//! **sealed cell root** (the L0 root AES-256-GCM-encrypted UNDER THE SEAL KEY, NEVER plaintext at
//! rest) + the **wrapped KEKs** (sealed under the root) + the **wrapped DEKs** (sealed under their
//! KEKs). The seal key NEVER rests in the DB (it is the env-supplied unseal key) and is never logged.
//!
//! ### `load_or_generate` — fail-closed + LOUD on a wrong/absent seal key
//! On boot the seal key is read from the environment; then for the cell's root:
//!   - **A sealed root EXISTS** → it MUST unseal under the seal key. If it does NOT, the key is
//!     WRONG/absent → [`KmsDurableError::WrongSealKey`] and the engine **refuses to start**. It NEVER
//!     generates a new root (that would silently orphan every existing ciphertext = unrecoverable
//!     data, the worst outcome). The wrapped KEKs/DEKs then load from the store.
//!   - **NO sealed root exists** (a genuine empty first boot) → generate a fresh root, seal it under
//!     the seal key, and persist it (race-safe via `INSERT … ON CONFLICT DO NOTHING` + re-read, so a
//!     concurrent first boot adopts the winner's root).
//!
//! ## Isolation posture — cell-INFRA key material, NOT a per-request tenant data store
//! The KMS holds the keys for ALL tenants in the cell; it is cell infrastructure, PII-free (key
//! material + opaque ids only), and cross-tenant by design (one engine resolves every tenant's DEK).
//! So — exactly like [`crate::placement_durable`] / [`crate::events_durable`] / [`crate::pgrelay`] —
//! it connects to the OLTP pool DIRECTLY, NOT through the per-request [`crate::tenant_tx::with_tenant_tx`]
//! / RLS convention (that convention is for per-tenant DATA stores like `principal`/`rebac_tuple`).
//! The `tenant_id` column on the KEK/DEK tables is the key-OWNER, not an RLS predicate; the
//! `kms_sealed_root` queries carry no tenant column at all. This file is therefore a NAMED, LOUD
//! `tenant-predicate` exclusion (documented in `tests/workspace_clean.rs`), never a silent skip — and
//! the lint stays FULLY live over the genuine tenant data stores (`pg.rs` / `identity_durable.rs`).
//!
//! ## Floors NAMED (in writing, per the prompt)
//! - **The HSM / Shamir-split-recovery L0 backing stays Tier-4 (P-524).** This is the software floor:
//!   the seal key is env-held, not in an HSM. The hierarchy SHAPE (root wraps KEKs, never exported)
//!   is unchanged when the HSM lands — only the seal-key custodian changes.
//! - **Production boot wiring + the kill-9/restart proof is MR-009.** This module ships the durable
//!   mechanism + proves decrypt-across-restart against live PG; wiring `load_or_generate` into the
//!   production boot spec as the non-optional default (so the in-memory `KmsEngine::new()` stops being
//!   the default) is MR-009 — at which point the `kms.rs` baseline entry flips present→removed.
//!
//! Feature-gated `integration` (it pulls the real sqlx client), like the rest of the live-PG code.

use sqlx::postgres::PgPool;
use sqlx::Row;

use myelin_tenancy::{Region, TenantId};

use crate::kms::{
    CellRoot, DekId, ExportedKek, KekId, KeyClass, KmsDurableSnapshot, KmsEngine, KmsError,
    PiiKeyRef, SealKey, SealKeyError, SealedRoot, WrappedDek, NONCE_LEN,
};
use crate::pg::PgError;

// =================================================================================================
// Migrations — the sealed-root + wrapped-KEK + wrapped-DEK tables. Applied via MR-022 `apply_validated`.
// =================================================================================================

/// The **sealed cell root** table (one row per cell). PII-free: the opaque `cell_id` PK + the sealed
/// root bytes (`nonce` + `ciphertext`, AES-256-GCM under the seal key — NEVER the plaintext root).
/// Forward-only (`IF NOT EXISTS`).
pub const KMS_SEALED_ROOT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS kms_sealed_root (
    cell_id    text PRIMARY KEY,
    nonce      bytea       NOT NULL,
    ciphertext bytea       NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);";

/// The **wrapped KEK** table (the L0→L1 envelope at rest). PII-free: the `(cell_id, tenant_id,
/// region)` key + the KEK sealed UNDER THE CELL ROOT (`nonce` + `wrapped`) + its `epoch`. The KEK
/// plaintext is NEVER stored. Forward-only.
pub const KMS_WRAPPED_KEK_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS kms_wrapped_kek (
    cell_id   text   NOT NULL,
    tenant_id text   NOT NULL,
    region    text   NOT NULL,
    nonce     bytea  NOT NULL,
    wrapped   bytea  NOT NULL,
    epoch     bigint NOT NULL,
    PRIMARY KEY (cell_id, tenant_id, region)
);";

/// The **wrapped DEK** table (the L1→L2 envelope at rest). PII-free: the `(cell_id, tenant_id,
/// class)` key + the DEK sealed UNDER ITS KEK (`nonce` + `wrapped`) + the `kek_epoch` it was wrapped
/// under + the `dek_epoch`. The `class` is the [`KeyClass`] token (`tenant`/`subject:<id>`/`blob`).
/// The DEK plaintext is NEVER stored. Forward-only.
pub const KMS_WRAPPED_DEK_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS kms_wrapped_dek (
    cell_id   text   NOT NULL,
    tenant_id text   NOT NULL,
    class     text   NOT NULL,
    nonce     bytea  NOT NULL,
    wrapped   bytea  NOT NULL,
    kek_epoch bigint NOT NULL,
    dek_epoch bigint NOT NULL,
    PRIMARY KEY (cell_id, tenant_id, class)
);";

/// The forward-only migration set the durable KMS store binds to. Applied via the MR-022
/// [`crate::provider::SubstrateProvider::migrate`] (validate → execute, race-safe, version-recorded)
/// at boot. Ids are stable + idempotent on re-boot. NOTE: NO RLS policy is installed (cell-infra key
/// material, cross-tenant by design — see the module docs).
pub fn kms_durable_migrations() -> crate::migration::Migrations {
    use crate::migration::{Migration, Migrations};
    Migrations::of([
        Migration::plain("0040_kms_sealed_root", KMS_SEALED_ROOT_MIGRATION),
        Migration::plain("0041_kms_wrapped_kek", KMS_WRAPPED_KEK_MIGRATION),
        Migration::plain("0042_kms_wrapped_dek", KMS_WRAPPED_DEK_MIGRATION),
    ])
}

// =================================================================================================
// Errors
// =================================================================================================

/// The env var the operator-held seal key is supplied through at boot (the software analogue of
/// presenting the HSM unseal key).
pub const SEAL_KEY_ENV: &str = "MYELIN_KMS_SEAL_KEY";

/// A durable-KMS boot/operation failure. Loud + typed; NEVER carries the seal key or any key
/// material (only the structural fault), so it is safe to log.
#[derive(Debug)]
pub enum KmsDurableError {
    /// A sealed cell root EXISTS for this cell but did NOT unseal under the supplied seal key — a
    /// WRONG or absent (or tampered) seal key. **Fail-closed + LOUD: the engine refuses to start and
    /// NEVER generates a fresh root** (that would orphan every existing ciphertext = unrecoverable
    /// data, §7.5). Carries only the opaque `cell_id`.
    WrongSealKey { cell_id: String },
    /// `MYELIN_KMS_SEAL_KEY` was not set at boot (fail-closed — never a default/zero key).
    SealKeyMissing,
    /// `MYELIN_KMS_SEAL_KEY` was set but malformed (the structural decode fault — no key bytes).
    SealKeyDecode(SealKeyError),
    /// A KMS engine op failed (e.g. `ensure_dek` before its KEK exists).
    Kms(KmsError),
    /// A durable-store DB error (the write/read did NOT succeed).
    Db(PgError),
}

impl core::fmt::Display for KmsDurableError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KmsDurableError::WrongSealKey { cell_id } => write!(
                f,
                "KMS REFUSED TO START for cell {cell_id}: a sealed cell root exists but did NOT \
                 unseal under the supplied seal key (wrong/absent MYELIN_KMS_SEAL_KEY) — fail-closed, \
                 NEVER generating a new root (that would orphan every existing ciphertext)"
            ),
            KmsDurableError::SealKeyMissing => write!(
                f,
                "KMS seal key is not set: {SEAL_KEY_ENV} must supply the 256-bit unseal key (64 hex \
                 chars) at boot — fail-closed, never a default key"
            ),
            KmsDurableError::SealKeyDecode(e) => write!(f, "KMS seal key from {SEAL_KEY_ENV}: {e}"),
            KmsDurableError::Kms(e) => write!(f, "KMS engine error: {e}"),
            KmsDurableError::Db(e) => write!(f, "durable KMS store error: {e}"),
        }
    }
}

impl std::error::Error for KmsDurableError {}

impl From<PgError> for KmsDurableError {
    fn from(e: PgError) -> Self {
        KmsDurableError::Db(e)
    }
}

/// Read the operator-held seal key from the environment (`MYELIN_KMS_SEAL_KEY`, a 32-byte key as 64
/// hex chars) — the at-boot unseal-key supply. Absent → [`KmsDurableError::SealKeyMissing`];
/// malformed → [`KmsDurableError::SealKeyDecode`]. Fail-closed at boot — NEVER a default key.
pub fn seal_key_from_env() -> Result<SealKey, KmsDurableError> {
    let raw = std::env::var(SEAL_KEY_ENV).map_err(|_| KmsDurableError::SealKeyMissing)?;
    SealKey::from_encoded(&raw).map_err(KmsDurableError::SealKeyDecode)
}

// =================================================================================================
// DurableKmsBacking — the sealed-root + wrapped-KEK + wrapped-DEK tables over the OLTP pool.
// =================================================================================================

/// The REAL durable KMS backing over the OLTP `PgPool`, scoped to one `cell_id` (the cell whose root
/// it holds). Cloneable (the pool is an `Arc`-backed handle). [`load_or_generate`](Self::load_or_generate)
/// recovers a working [`KmsEngine`] across a restart; the in-memory [`KmsEngine::new`] is the explicit
/// test-double. Connects to the pool DIRECTLY (cell-infra, cross-tenant key material — no
/// `with_tenant_tx`/RLS; see the module docs).
#[derive(Clone)]
pub struct DurableKmsBacking {
    pool: PgPool,
    cell_id: String,
}

impl DurableKmsBacking {
    /// Wrap a pool as the durable KMS backing for a given cell. The caller must have applied
    /// [`kms_durable_migrations`] (via the MR-022 provider's `migrate`) so the tables exist.
    pub fn new(pool: PgPool, cell_id: impl Into<String>) -> DurableKmsBacking {
        DurableKmsBacking {
            pool,
            cell_id: cell_id.into(),
        }
    }

    /// The cell this backing holds the root for.
    pub fn cell_id(&self) -> &str {
        &self.cell_id
    }

    /// **`load_or_generate` — the durable root-origin constructor (MR-025).** Recover a working
    /// [`KmsEngine`] for this cell from the store under `seal_key`, or — on a genuine empty first boot
    /// — generate + persist a fresh root. See the module docs for the fail-closed-on-wrong-key logic:
    /// a sealed root that exists but does NOT unseal is a LOUD [`KmsDurableError::WrongSealKey`] and
    /// the engine refuses to start (NEVER a new root). On success the wrapped KEKs + DEKs are loaded
    /// from the store into the engine, so a DEK provisioned before a restart resolves + decrypts after.
    pub async fn load_or_generate(&self, seal_key: &SealKey) -> Result<KmsEngine, KmsDurableError> {
        let root = match self.read_sealed_root().await? {
            Some(sealed) => {
                // A root EXISTS → it MUST unseal under the seal key. Fail-closed + LOUD otherwise;
                // NEVER generate a new root (that would orphan every existing ciphertext).
                CellRoot::unseal(seal_key, &sealed).ok_or_else(|| KmsDurableError::WrongSealKey {
                    cell_id: self.cell_id.clone(),
                })?
            }
            None => {
                // Genuine first boot: generate, seal, persist (race-safe — the loser of a concurrent
                // first boot adopts the winner's root via ON CONFLICT DO NOTHING + re-read).
                let fresh = CellRoot::generate();
                self.insert_sealed_root_if_absent(&fresh.seal(seal_key))
                    .await?;
                let stored = self.read_sealed_root().await?.ok_or_else(|| {
                    KmsDurableError::Db(PgError::Query(
                        "sealed cell root vanished immediately after insert".into(),
                    ))
                })?;
                CellRoot::unseal(seal_key, &stored).ok_or_else(|| KmsDurableError::WrongSealKey {
                    cell_id: self.cell_id.clone(),
                })?
            }
        };
        let engine = KmsEngine::from_root(root);
        self.load_keks(&engine).await?;
        self.load_deks(&engine).await?;
        Ok(engine)
    }

    // ---- sealed root ----

    async fn read_sealed_root(&self) -> Result<Option<SealedRoot>, PgError> {
        let row = sqlx::query("SELECT nonce, ciphertext FROM kms_sealed_root WHERE cell_id = $1")
            .bind(&self.cell_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(row.map(|r| {
            let nonce: Vec<u8> = r.get("nonce");
            let ciphertext: Vec<u8> = r.get("ciphertext");
            SealedRoot {
                nonce: nonce_from(&nonce),
                ciphertext,
            }
        }))
    }

    async fn insert_sealed_root_if_absent(&self, sealed: &SealedRoot) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO kms_sealed_root (cell_id, nonce, ciphertext) VALUES ($1, $2, $3) \
             ON CONFLICT (cell_id) DO NOTHING",
        )
        .bind(&self.cell_id)
        .bind(sealed.nonce.as_slice())
        .bind(&sealed.ciphertext)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    async fn upsert_sealed_root(&self, sealed: &SealedRoot) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO kms_sealed_root (cell_id, nonce, ciphertext) VALUES ($1, $2, $3) \
             ON CONFLICT (cell_id) DO UPDATE SET nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext",
        )
        .bind(&self.cell_id)
        .bind(sealed.nonce.as_slice())
        .bind(&sealed.ciphertext)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    // ---- KEKs ----

    async fn load_keks(&self, engine: &KmsEngine) -> Result<(), PgError> {
        let rows = sqlx::query(
            "SELECT tenant_id, region, nonce, wrapped, epoch FROM kms_wrapped_kek WHERE cell_id = $1",
        )
        .bind(&self.cell_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        for r in rows {
            let tenant: String = r.get("tenant_id");
            let region: String = r.get("region");
            let nonce: Vec<u8> = r.get("nonce");
            let wrapped: Vec<u8> = r.get("wrapped");
            let epoch: i64 = r.get("epoch");
            engine.install_wrapped_kek(
                KekId::new(TenantId(tenant), Region(region)),
                nonce_from(&nonce),
                wrapped,
                epoch as u64,
            );
        }
        Ok(())
    }

    async fn upsert_kek_row(&self, id: &KekId, k: &ExportedKek) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO kms_wrapped_kek (cell_id, tenant_id, region, nonce, wrapped, epoch) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (cell_id, tenant_id, region) DO UPDATE SET \
               nonce = EXCLUDED.nonce, wrapped = EXCLUDED.wrapped, epoch = EXCLUDED.epoch",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.region.as_str())
        .bind(k.nonce.as_slice())
        .bind(&k.wrapped)
        .bind(k.epoch as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    /// Persist (upsert) one KEK's wrapped form — the write-through after `ensure_kek`/`rotate_kek`.
    pub async fn persist_kek(&self, engine: &KmsEngine, id: &KekId) -> Result<(), KmsDurableError> {
        let k = engine.export_kek(id).ok_or_else(|| {
            KmsDurableError::Db(PgError::Query(format!(
                "no KEK to persist for tenant={} region={}",
                id.tenant.as_str(),
                id.region.as_str()
            )))
        })?;
        self.upsert_kek_row(id, &k).await?;
        Ok(())
    }

    async fn delete_kek_row(&self, id: &KekId) -> Result<(), PgError> {
        sqlx::query("DELETE FROM kms_wrapped_kek WHERE cell_id = $1 AND tenant_id = $2 AND region = $3")
            .bind(&self.cell_id)
            .bind(id.tenant.as_str())
            .bind(id.region.as_str())
            .execute(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    // ---- DEKs ----

    async fn load_deks(&self, engine: &KmsEngine) -> Result<(), PgError> {
        let rows = sqlx::query(
            "SELECT tenant_id, class, nonce, wrapped, kek_epoch, dek_epoch FROM kms_wrapped_dek \
             WHERE cell_id = $1",
        )
        .bind(&self.cell_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        for r in rows {
            let tenant: String = r.get("tenant_id");
            let class_token: String = r.get("class");
            let class = KeyClass::parse_token(&class_token).ok_or_else(|| {
                PgError::Query(format!("corrupt DEK class token in kms_wrapped_dek: {class_token}"))
            })?;
            let nonce: Vec<u8> = r.get("nonce");
            let wrapped: Vec<u8> = r.get("wrapped");
            let kek_epoch: i64 = r.get("kek_epoch");
            let dek_epoch: i64 = r.get("dek_epoch");
            engine.install_wrapped_dek(
                DekId::new(TenantId(tenant), class),
                WrappedDek {
                    nonce: nonce_from(&nonce),
                    wrapped,
                    kek_epoch: kek_epoch as u64,
                },
                dek_epoch as u64,
            );
        }
        Ok(())
    }

    async fn upsert_dek_row(
        &self,
        id: &DekId,
        w: &WrappedDek,
        dek_epoch: u64,
    ) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO kms_wrapped_dek \
               (cell_id, tenant_id, class, nonce, wrapped, kek_epoch, dek_epoch) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (cell_id, tenant_id, class) DO UPDATE SET \
               nonce = EXCLUDED.nonce, wrapped = EXCLUDED.wrapped, \
               kek_epoch = EXCLUDED.kek_epoch, dek_epoch = EXCLUDED.dek_epoch",
        )
        .bind(&self.cell_id)
        .bind(id.tenant.as_str())
        .bind(id.class.as_token())
        .bind(w.nonce.as_slice())
        .bind(&w.wrapped)
        .bind(w.kek_epoch as i64)
        .bind(dek_epoch as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    /// Persist (upsert) one DEK's wrapped form — the write-through after `ensure_dek`.
    pub async fn persist_dek(&self, engine: &KmsEngine, id: &DekId) -> Result<(), KmsDurableError> {
        let (w, dek_epoch) = engine.export_dek(id).ok_or_else(|| {
            KmsDurableError::Db(PgError::Query(format!(
                "no DEK to persist for tenant={} class={}",
                id.tenant.as_str(),
                id.class.as_token()
            )))
        })?;
        self.upsert_dek_row(id, &w, dek_epoch).await?;
        Ok(())
    }

    async fn delete_dek_row(&self, id: &DekId) -> Result<(), PgError> {
        sqlx::query("DELETE FROM kms_wrapped_dek WHERE cell_id = $1 AND tenant_id = $2 AND class = $3")
            .bind(&self.cell_id)
            .bind(id.tenant.as_str())
            .bind(id.class.as_token())
            .execute(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    // ---- write-through convenience (the engine op + the durable persist, together) ----

    /// `ensure_kek` + persist (the L1 KEK is durable from the moment it is minted).
    pub async fn ensure_kek(&self, engine: &KmsEngine, id: &KekId) -> Result<u64, KmsDurableError> {
        let epoch = engine.ensure_kek(id);
        self.persist_kek(engine, id).await?;
        Ok(epoch)
    }

    /// `ensure_dek` + persist (the L2 DEK is durable from the moment it is minted).
    pub async fn ensure_dek(
        &self,
        engine: &KmsEngine,
        tenant: &TenantId,
        region: &Region,
        class: KeyClass,
    ) -> Result<PiiKeyRef, KmsDurableError> {
        let key_ref = engine
            .ensure_dek(tenant, region, class)
            .map_err(KmsDurableError::Kms)?;
        self.persist_dek(engine, &DekId::new(tenant.clone(), key_ref.class.clone()))
            .await?;
        Ok(key_ref)
    }

    /// `rotate_kek` + re-persist the KEK AND every re-wrapped DEK for that tenant (rotation re-wraps
    /// the DEKs under the new KEK — the wrapped forms changed, so the durable rows must too).
    pub async fn rotate_kek(&self, engine: &KmsEngine, id: &KekId) -> Result<u64, KmsDurableError> {
        let epoch = engine.rotate_kek(id).map_err(KmsDurableError::Kms)?;
        self.persist_kek(engine, id).await?;
        for (dek_id, w, dek_epoch) in engine
            .export_deks()
            .into_iter()
            .filter(|(d, _, _)| d.tenant == id.tenant)
        {
            self.upsert_dek_row(&dek_id, &w, dek_epoch).await?;
        }
        Ok(epoch)
    }

    /// `destroy_kek` (crypto-shred L1) + DELETE the durable row — the shred reaches the store, so a
    /// restart can never resurrect the offboarded tenant's KEK.
    pub async fn destroy_kek(&self, engine: &KmsEngine, id: &KekId) -> Result<bool, KmsDurableError> {
        let removed = engine.destroy_kek(id);
        self.delete_kek_row(id).await?;
        Ok(removed)
    }

    /// `destroy_dek` (crypto-shred L2 / GD-4 individual erasure) + DELETE the durable row.
    pub async fn destroy_dek(&self, engine: &KmsEngine, id: &DekId) -> Result<bool, KmsDurableError> {
        let removed = engine.destroy_dek(id);
        self.delete_dek_row(id).await?;
        Ok(removed)
    }

    // ---- snapshot / restore ----

    /// **Restore a [`KmsDurableSnapshot`] into THIS store** — write the sealed root + every wrapped
    /// KEK + wrapped DEK, so a fresh [`load_or_generate`](Self::load_or_generate) over this store
    /// (WITH the same seal key the snapshot was sealed under) recovers every encrypted column.
    /// Idempotent upserts; a crypto-shredded key is ABSENT from the snapshot, so it is NOT restored
    /// (it stays dead, §7.5).
    pub async fn restore(&self, snap: &KmsDurableSnapshot) -> Result<(), KmsDurableError> {
        self.upsert_sealed_root(&snap.sealed_root).await?;
        for (id, k) in &snap.keks {
            self.upsert_kek_row(id, k).await?;
        }
        for (id, w, dek_epoch) in &snap.deks {
            self.upsert_dek_row(id, w, *dek_epoch).await?;
        }
        Ok(())
    }

    /// Mirror the engine's ENTIRE current state into this store (the sealed root + all live KEKs +
    /// all live DEKs) — a convenience full write-through. Upserts only (crypto-shred reaches the store
    /// via the per-key `destroy_*` deletes, never a blanket wipe).
    pub async fn persist(
        &self,
        engine: &KmsEngine,
        seal_key: &SealKey,
    ) -> Result<(), KmsDurableError> {
        self.restore(&engine.backup_snapshot_durable(seal_key)).await
    }
}

/// Convert stored bytes into a fixed-size AES-GCM nonce. A wrong length (corruption) yields a nonce
/// that will not authenticate — a SAFE fail-closed (unseal/unwrap returns the loud failure, never a
/// wrong-key success).
fn nonce_from(bytes: &[u8]) -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    let len = bytes.len().min(NONCE_LEN);
    n[..len].copy_from_slice(&bytes[..len]);
    n
}
