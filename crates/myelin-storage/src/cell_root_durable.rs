//! # Durable PG backing for the capability-token CELL AUTHORITY ROOT (R4.0 / P-527 / MR-025 follow-on)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md` §4 (capability tokens are
//! Ed25519-signed PASETO v4.public envelopes whose macaroon caveat chain is seeded from a cell-held
//! `K_mac`; a production cell **loads these two secrets from the KMS-sealed cell root**). This is the
//! durable persistence the `capability_crypto` doc-comment names as the P-527 / MR-025 follow-on: the
//! `CellTokenAuthority` was ephemeral (`CellTokenAuthority::generate()` per boot), so **every token
//! minted before a restart failed to verify after it** (the public key the verifier trusts changed).
//! No mint path could exist against the real `edge` binary. This module makes the cell authority root
//! DURABLE, sealed under the SAME operator-held seal key the KMS root uses.
//!
//! ## Anti-duplication — this MIRRORS [`crate::kms_durable`]; it does NOT fork a second seal mechanism
//! The seal/unseal is the SAME vetted AES-256-GCM AEAD keyed by the SAME operator-held [`SealKey`]
//! ([`SealKey::seal_bytes`]/[`SealKey::open_bytes`], the generic analogue of [`crate::kms::CellRoot::seal`]).
//! This module is ONLY the durable STORE (the one PG table + the load/persist plumbing) for the
//! 64-byte capability-token root material (the Ed25519 seed ‖ the macaroon MAC key). It does NOT know
//! about `CellTokenAuthority` (that type lives in `myelin-identity-service`, above this crate in the
//! DAG): it returns the raw [`CellRootMaterial`] and the identity-service constructs the authority via
//! `CellTokenAuthority::from_material` — exactly the split the durable identity/KMS backings already
//! use (all the PG/sqlx code lives HERE; the domain type is built one layer up).
//!
//! ## The software-sealed root-of-trust (identical posture to the KMS root)
//! The cell-authority secrets cannot rest in plaintext. The durable store holds the **sealed material**
//! (the 64-byte seed‖mac AES-256-GCM-encrypted UNDER THE SEAL KEY — NEVER plaintext at rest), keyed by
//! the opaque `cell_id`. The seal key is supplied at boot from `MYELIN_KMS_SEAL_KEY` (the SAME key that
//! unseals the KMS root — one operator secret, one blast radius) and NEVER rests in the DB, is NEVER
//! logged.
//!
//! ### `load_or_generate` — fail-closed + LOUD on a wrong/absent seal key
//!   - **Sealed material EXISTS** → it MUST unseal under the seal key. If it does NOT, the key is
//!     WRONG/absent → [`CellRootError::WrongSealKey`] and the caller **refuses to start**. It NEVER
//!     generates fresh material (that would ORPHAN EVERY TOKEN ever minted under the old root — every
//!     operator/CI/agent credential would silently stop verifying, the worst outcome).
//!   - **NO sealed material exists** (a genuine empty first boot) → generate a fresh Ed25519 seed +
//!     MAC key from the OS CSPRNG, seal them, persist (`INSERT … ON CONFLICT DO NOTHING` + re-read, so
//!     a concurrent first boot adopts the winner's root — two boots can NEVER end up on two different
//!     cell roots), and return.
//!
//! ## Isolation posture — cell-INFRA key material, NOT a per-request tenant data store
//! Exactly like [`crate::kms_durable`]: the cell authority signs tokens for ALL tenants in the cell; it
//! is cell infrastructure, PII-free (key material + an opaque `cell_id` only), and connects to the OLTP
//! pool DIRECTLY — NOT through the per-request [`crate::tenant_tx`] / RLS convention (that convention is
//! for per-tenant DATA stores like `principal`/`rebac_tuple`). The `cell_token_root` table carries NO
//! tenant column. This file is therefore a NAMED, LOUD `tenant-predicate` exclusion (registered in
//! `crates/myelin-lints/tests/workspace_clean.rs`, alongside `kms_durable.rs`), never a silent skip —
//! and the lint stays FULLY live over the genuine tenant data stores (`pg.rs` / `identity_durable.rs`).
//!
//! ## Floors NAMED
//! - **The HSM / Shamir-split-recovery L0 backing stays Tier-4 (P-524)** — the SAME floor the KMS root
//!   names. This is the software floor: the seal key is env-held, not in an HSM. Only the seal-key
//!   custodian changes when the HSM lands; the sealed-material SHAPE is unchanged.
//! - **Key rotation of the cell authority (a second anchor + a footer key-id) is the MR-011 named
//!   follow-on** (`capability_crypto` documents the single-anchor floor). This module persists ONE
//!   current root per cell; a rotation layer would add an epoch column without reshaping the row.

use sqlx::postgres::PgPool;
use sqlx::Row;

use aes_gcm::aead::OsRng;
use aes_gcm::{Aes256Gcm, KeyInit};

use crate::kms::{SealKey, KEY_LEN, NONCE_LEN};
use crate::pg::PgError;

// =================================================================================================
// Migration — the sealed cell-authority-root table. Applied via the MR-022 durable aggregate.
// =================================================================================================

/// The **sealed capability-token cell-authority root** table (one row per cell). PII-free: the opaque
/// `cell_id` PK + the sealed root bytes (`nonce` + `ciphertext`, AES-256-GCM under the seal key —
/// NEVER the plaintext seed/MAC). Forward-only (`IF NOT EXISTS`). Mirrors `kms_sealed_root` exactly.
pub const CELL_TOKEN_ROOT_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS cell_token_root (
    cell_id    text PRIMARY KEY,
    nonce      bytea       NOT NULL,
    ciphertext bytea       NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);";

/// The forward-only migration group the durable cell-authority-root store binds to (`0060`). Folded
/// into [`crate::provider::all_durable_migrations`] via `durable_migration_groups`. Idempotent + stable
/// on re-boot. NO RLS policy is installed (cell-infra key material, cross-tenant by design — module docs).
pub fn cell_root_durable_migrations() -> crate::migration::Migrations {
    use crate::migration::{Migration, Migrations};
    Migrations::of([Migration::plain(
        "0060_cell_token_root",
        CELL_TOKEN_ROOT_MIGRATION,
    )])
}

// =================================================================================================
// Material + errors
// =================================================================================================

/// The size of the persisted cell-authority root material: the 32-byte Ed25519 seed followed by the
/// 32-byte macaroon MAC key.
const MATERIAL_LEN: usize = KEY_LEN * 2;

/// **The recovered capability-token cell-authority root material — the 32-byte Ed25519 seed + the
/// 32-byte macaroon MAC key.** The SEAM between this storage backing (which owns the PG/seal code) and
/// `myelin-identity-service` (which builds the `CellTokenAuthority` via `from_material`). It carries
/// SECRET key material: it derives NO `Debug`/`Clone`/`serde` (a redacted `Debug` is provided) so it
/// can never leak into a log or be serialized. It exists only transiently in the composition root.
pub struct CellRootMaterial {
    /// The Ed25519 signing seed (the cell's PASETO signing key — its PUBLIC half is the verifier's
    /// trust anchor).
    pub ed25519_seed: [u8; KEY_LEN],
    /// The macaroon root MAC key `K_mac` (seeds the caveat chain; the holder never sees it).
    pub mac_key: [u8; KEY_LEN],
}

impl core::fmt::Debug for CellRootMaterial {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Redacted — the cell-authority secrets NEVER enter a log/Debug output (a leak is a total
        // token-forgery compromise: anyone holding the seed can mint any token in any tenant).
        f.write_str("CellRootMaterial(<redacted seed + mac key>)")
    }
}

/// A durable-cell-root boot/operation failure. Loud + typed; NEVER carries the seal key or any key
/// material (only the structural fault), so it is safe to log.
#[derive(Debug)]
pub enum CellRootError {
    /// A sealed cell-authority root EXISTS for this cell but did NOT unseal under the supplied seal
    /// key — a WRONG or absent (or tampered) seal key. **Fail-closed + LOUD: the caller refuses to
    /// start and NEVER generates a fresh root** (that would orphan every token ever minted under the
    /// old root). Carries only the opaque `cell_id`.
    WrongSealKey {
        /// The opaque cell id whose sealed root did not unseal.
        cell_id: String,
    },
    /// A durable-store DB error (the write/read did NOT succeed) — a LOUD typed value, never a silent
    /// partial write.
    Db(PgError),
}

impl core::fmt::Display for CellRootError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CellRootError::WrongSealKey { cell_id } => write!(
                f,
                "capability-token cell authority REFUSED TO START for cell {cell_id}: a sealed cell \
                 root exists but did NOT unseal under the supplied seal key (wrong/absent \
                 MYELIN_KMS_SEAL_KEY) — fail-closed, NEVER generating a new root (that would orphan \
                 every token ever minted under the old root)"
            ),
            CellRootError::Db(e) => write!(f, "durable cell-root store error: {e}"),
        }
    }
}

impl std::error::Error for CellRootError {}

impl From<PgError> for CellRootError {
    fn from(e: PgError) -> Self {
        CellRootError::Db(e)
    }
}

// =================================================================================================
// DurableCellRootBacking — the sealed cell-authority-root row over the OLTP pool.
// =================================================================================================

/// The REAL durable cell-authority-root backing over the OLTP `PgPool`, scoped to one `cell_id`.
/// Cloneable (the pool is an `Arc`-backed handle). [`load_or_generate`](Self::load_or_generate)
/// recovers the cell-authority material across a restart (so a token minted before a kill-9 verifies
/// after it). Connects to the pool DIRECTLY (cell-infra, cross-tenant key material — no
/// `with_tenant_tx`/RLS; see the module docs). Named `…Backing` (not a durable role suffix) + carries
/// a `PgPool` — so the `no-in-memory-durable-store` scanner correctly reads it as a durable store.
#[derive(Clone)]
pub struct DurableCellRootBacking {
    pool: PgPool,
    cell_id: String,
}

impl DurableCellRootBacking {
    /// Wrap a pool as the durable cell-authority-root backing for a given cell. The caller must have
    /// applied [`cell_root_durable_migrations`] (via the MR-022 provider's `migrate`, folded into the
    /// aggregate) so the table exists.
    pub fn new(pool: PgPool, cell_id: impl Into<String>) -> DurableCellRootBacking {
        DurableCellRootBacking {
            pool,
            cell_id: cell_id.into(),
        }
    }

    /// The cell this backing holds the authority root for.
    pub fn cell_id(&self) -> &str {
        &self.cell_id
    }

    /// **`load_or_generate` — the durable cell-authority-root origin.** Recover the cell-authority
    /// material for this cell from the store under `seal_key`, or — on a genuine empty first boot —
    /// generate + persist fresh material. See the module docs for the fail-closed-on-wrong-key logic:
    /// a sealed root that exists but does NOT unseal is a LOUD [`CellRootError::WrongSealKey`] and the
    /// caller must refuse to start (NEVER fresh material). On a fresh generate the material is sealed +
    /// persisted race-safely (`ON CONFLICT DO NOTHING` + re-read), so a concurrent first boot adopts
    /// the winner's root — two boots can never diverge onto two cell authorities.
    pub async fn load_or_generate(
        &self,
        seal_key: &SealKey,
    ) -> Result<CellRootMaterial, CellRootError> {
        if let Some((nonce, ciphertext)) = self.read_sealed_root().await? {
            // A root EXISTS → it MUST unseal under the seal key. Fail-closed + LOUD otherwise; NEVER
            // generate a new root (that would orphan every token ever minted under the old root).
            let plain = seal_key.open_bytes(&nonce, &ciphertext).ok_or_else(|| {
                CellRootError::WrongSealKey {
                    cell_id: self.cell_id.clone(),
                }
            })?;
            return Self::material_from_plain(&plain, &self.cell_id);
        }
        // Genuine first boot: generate 32-byte Ed25519 seed + 32-byte MAC key from the OS CSPRNG,
        // seal the 64-byte concatenation, persist race-safely (the loser of a concurrent first boot
        // adopts the winner's root via ON CONFLICT DO NOTHING + re-read).
        let mut plain = [0u8; MATERIAL_LEN];
        plain[..KEY_LEN].copy_from_slice(random_key().as_slice());
        plain[KEY_LEN..].copy_from_slice(random_key().as_slice());
        let (nonce, ciphertext) = seal_key.seal_bytes(&plain);
        self.insert_sealed_root_if_absent(&nonce, &ciphertext).await?;
        // Re-read the WINNER's row (may be ours or a racing boot's) and unseal it — so every boot
        // converges on the one persisted root.
        let (nonce, ciphertext) = self.read_sealed_root().await?.ok_or_else(|| {
            CellRootError::Db(PgError::Query(
                "sealed cell-authority root vanished immediately after insert".into(),
            ))
        })?;
        let plain = seal_key.open_bytes(&nonce, &ciphertext).ok_or_else(|| {
            CellRootError::WrongSealKey {
                cell_id: self.cell_id.clone(),
            }
        })?;
        Self::material_from_plain(&plain, &self.cell_id)
    }

    /// Split the unsealed 64-byte plaintext into the seed + MAC key. A wrong length is a corrupt row
    /// (refused loudly — never a truncated/zero key).
    fn material_from_plain(plain: &[u8], cell_id: &str) -> Result<CellRootMaterial, CellRootError> {
        if plain.len() != MATERIAL_LEN {
            return Err(CellRootError::Db(PgError::Query(format!(
                "corrupt sealed cell-authority root for cell {cell_id}: unsealed to {} bytes \
                 (expected {MATERIAL_LEN})",
                plain.len()
            ))));
        }
        let mut ed25519_seed = [0u8; KEY_LEN];
        let mut mac_key = [0u8; KEY_LEN];
        ed25519_seed.copy_from_slice(&plain[..KEY_LEN]);
        mac_key.copy_from_slice(&plain[KEY_LEN..]);
        Ok(CellRootMaterial {
            ed25519_seed,
            mac_key,
        })
    }

    async fn read_sealed_root(&self) -> Result<Option<([u8; NONCE_LEN], Vec<u8>)>, PgError> {
        let row = sqlx::query("SELECT nonce, ciphertext FROM cell_token_root WHERE cell_id = $1")
            .bind(&self.cell_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(row.map(|r| {
            let nonce: Vec<u8> = r.get("nonce");
            let ciphertext: Vec<u8> = r.get("ciphertext");
            (nonce_from(&nonce), ciphertext)
        }))
    }

    async fn insert_sealed_root_if_absent(
        &self,
        nonce: &[u8; NONCE_LEN],
        ciphertext: &[u8],
    ) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO cell_token_root (cell_id, nonce, ciphertext) VALUES ($1, $2, $3) \
             ON CONFLICT (cell_id) DO NOTHING",
        )
        .bind(&self.cell_id)
        .bind(nonce.as_slice())
        .bind(ciphertext)
        .execute(&self.pool)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }
}

/// Generate a fresh random 256-bit key from the OS CSPRNG (the SAME `aes-gcm` `OsRng` source the KMS
/// `RawKey::generate` uses — never a hand-rolled RNG).
fn random_key() -> [u8; KEY_LEN] {
    let key = Aes256Gcm::generate_key(OsRng);
    let mut bytes = [0u8; KEY_LEN];
    bytes.copy_from_slice(key.as_slice());
    bytes
}

/// Convert stored bytes into a fixed-size AES-GCM nonce. A wrong length (corruption) yields a nonce
/// that will not authenticate — a SAFE fail-closed (unseal returns the loud WrongSealKey, never a
/// wrong-key success). Mirrors `kms_durable::nonce_from`.
fn nonce_from(bytes: &[u8]) -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    let len = bytes.len().min(NONCE_LEN);
    n[..len].copy_from_slice(&bytes[..len]);
    n
}
