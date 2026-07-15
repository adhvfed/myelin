//! The three-level KMS key hierarchy + the fail-static availability posture (P-ST-06 / 11.3).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §4 (the three-level
//! envelope-encryption hierarchy — L0 per-cell root HSM/sealed never exported; L1 per-(tenant,
//! region) KEK = the tenant-granularity crypto-shred lever; L2 DEKs AES-256-GCM, per-tenant for
//! bulk, per-subject for the individual-erasure classes; the
//! `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>` travels with every ciphertext; key rotation
//! = envelope re-wrap O(keys) not O(data); **KMS availability degrades fail-static, hard-down →
//! not-ready never fail-open; a crypto-shredded key is excluded from backup**), §5 (crypto-shred
//! and GD-4 granularity — per-subject vs per-tenant). Contract-index row 11.3 (the KMS hierarchy
//! half; the `KeyOrigin` trait half is the sibling P-ST-07 / P-094).
//!
//! ## What this prompt ships (P-ST-06) and what it does NOT
//! - **The three-level hierarchy:** [`CellRoot`] (L0) wraps [`Kek`]s (L1); a [`Kek`] wraps
//!   [`Dek`]s (L2). The [`KmsEngine`] is the self-hostable software engine (Vault-Transit-class)
//!   behind the [`KmsAdapter`] seam — every byte of key material at rest is a [`WrappedDek`]
//!   (the DEK ciphertext, AES-256-GCM-sealed under its KEK), never a bare plaintext key.
//! - **AES-256-GCM L2 DEKs** (§4): a DEK is 32 bytes of AEAD key material; wrap = AES-256-GCM
//!   encrypt the DEK under its KEK; unwrap = decrypt. Real crypto (vetted RustCrypto `aes-gcm`),
//!   not a stub — EI-01 §7 ("cite proven structures, do not hand-roll crypto").
//! - **The frozen `pii_key_ref`** ([`PiiKeyRef`]): `kms://<tenant>/<dek-epoch>/<class>`,
//!   `<class> ∈ {tenant, subject:<id>, blob}` — copied byte-exact from §4, round-trip
//!   parse/render tested.
//! - **GD-4 granularity** ([`KeyClass`], §5): per-tenant DEKs for bulk; per-subject DEKs for the
//!   free-text/profile/chat-body/agent-memory classes whose erasure unit is the individual; a
//!   per-subject DEK is a DISTINCT key from the tenant DEK (the GD-4 subject-granular lever).
//! - **Crypto-shred** ([`KmsEngine::destroy_kek`] / [`KmsEngine::destroy_dek`]): destroying the
//!   L1 KEK renders EVERY DEK under it unrecoverable (tenant-granularity crypto-shred, the
//!   tenant-offboard lever); destroying a per-subject DEK renders that subject's ciphertext
//!   unrecoverable (the GD-4 individual-erasure lever). A destroyed key is **excluded from
//!   backup** ([`KmsEngine::backup_snapshot`] never emits a shredded key — §7.5: it must stay
//!   dead across a restore).
//! - **Rotation = envelope re-wrap** ([`KmsEngine::rotate_kek`]): a new KEK epoch re-wraps the
//!   existing DEKs under the new KEK — `O(keys)`, NOT `O(data)`; forward-only (a new epoch never
//!   rolls an old one back; a compromised key triggers expand→backfill re-encryption, not a
//!   rollback). The DEK plaintext (and therefore every ciphertext's content) is untouched.
//! - **The fail-static availability posture** (§4.5; the STOR-D6 gate): a transient KMS outage →
//!   resolved-DEK reads survive a bounded TTL ([`KmsReadPath`] over [`FailStatic`]); a SUSTAINED
//!   hard-down → **not-ready + shed** ([`KmsReadiness::NotReady`]), and a read whose DEK cannot be
//!   resolved returns [`KmsReadError`], **never a plaintext-without-key** — we NEVER fail open.
//!
//! ## Floors named (stubbed / deferred + the filling prompt) — VISION §3, prompt DoD
//! - **The `KeyOrigin` trait** (platform-managed | BYOK | HYOK behind one trait;
//!   `wrap`/`unwrap`/`can_derive_plaintext_index`/`destroy`) is the SIBLING prompt **P-ST-07
//!   (global P-094)** — it FRONTS this engine. This prompt ships the hierarchy + the engine the
//!   trait will wrap; the trait itself (and its `can_derive_plaintext_index()=false` structural
//!   HYOK enforcement) lands there.
//! - **The OLTP / blob ENCRYPTION wiring** (classify-driven per-subject/per-tenant key choice;
//!   the real per-blob content-key wrap under the DEK) is **P-ST-08 (global P-095)** — it wires
//!   THIS engine to the [`crate::oltp`] columns + the [`crate::blob`] store. This prompt ships
//!   the key mechanism; the column/blob wiring lands there. No real tenant data is encrypted
//!   before then (the M1 STOR-D1 restore-verify gate enforces it).
//! - **The per-content-class HYOK POLICY** (which classes may be HYOK; the
//!   cross-artifact-reference-spanning case) **+ the KMIP/external-key-store adapter** **+
//!   HYOK-as-a-Schrems-III-mitigation (GD-7)** are `[OPEN → P6/LEGAL]` named follow-ons
//!   (parallel-legal). The MECHANISM + the limits ship regardless; the policy is handed to
//!   counsel/DPO. Recorded HERE in writing (storage.md §6 `[OPEN → P6/LEGAL]`).
//! - **The HSM/sealed L0 cell root** is, on this in-cell software floor, a process-held root key
//!   (the `KmsEngine` holds it sealed — never exported through any public method, see
//!   [`CellRoot`]'s private field + the `Debug` redaction). The HSM/Shamir-split-recovery
//!   backing is the production hardening follow-on; the hierarchy SHAPE (root wraps KEKs, never
//!   exported) is complete and unchanged when the HSM lands.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2; prompt TESTS field)
//! The wrap/unwrap/destroy path + the **fail-open-prevention branch** are mandatory-core: the
//! load-bearing decisions are *a destroyed key never unwraps* and *an unresolvable DEK never
//! returns plaintext* (0 fail-open). The achieved score is stated in the P-058 report
//! (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/kms.rs`).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};

use myelin_tenancy::{Region, TenantId};

use crate::kms_durable::DurableKms;

/// The size of an AES-256 key, in bytes (the L2 DEK + the L1 KEK + the L0 cell root are all
/// 256-bit AEAD keys; §4 "AES-256-GCM").
pub const KEY_LEN: usize = 32;

/// The size of the AES-GCM nonce, in bytes (96-bit, NIST SP 800-38D recommended).
pub const NONCE_LEN: usize = 12;

// ─────────────────────────────── the pii_key_ref (frozen §4 shape) ───────────────────────────

/// The `<class>` of a DEK (storage.md §4/§5; the `pii_key_ref` `<class>` field). The GD-4
/// granularity lever: bulk content is keyed [`KeyClass::Tenant`]; the free-text/profile/
/// chat-body/agent-memory classes whose erasure unit is the individual subject are keyed
/// [`KeyClass::Subject`] (one key-destroy = that person's Art. 17 erasure); a content blob's
/// per-blob content key is wrapped under [`KeyClass::Blob`].
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum KeyClass {
    /// `tenant` — the per-tenant bulk DEK (issue field values, doc structure, repo/PR metadata,
    /// run state — mostly non-personal or pseudonym-referenced; erasure here is
    /// tombstone/pseudonymise, not key-destroy).
    Tenant,
    /// `subject:<id>` — the per-SUBJECT DEK for the individual-erasure classes (free-text /
    /// profile, chat bodies, CI inline-PII log segments, agent memory). Destroying it is the
    /// GD-4 individual crypto-shred lever. Carries the subject id.
    Subject(String),
    /// `blob` — the per-blob content-key class (the object tier's per-blob content key, wrapped
    /// under the tenant/per-subject DEK so the content-address stays stable while rotation/shred
    /// operate at the DEK level, §4).
    Blob,
}

impl KeyClass {
    /// Render the `<class>` token exactly as it appears in a `pii_key_ref` (the §4 grammar:
    /// `tenant` | `subject:<id>` | `blob`).
    pub fn as_token(&self) -> String {
        match self {
            KeyClass::Tenant => "tenant".to_string(),
            KeyClass::Subject(id) => format!("subject:{id}"),
            KeyClass::Blob => "blob".to_string(),
        }
    }

    /// Parse a `<class>` token back into a [`KeyClass`]. Returns `None` for an unrecognised token
    /// (an unknown class is a malformed `pii_key_ref`, never silently coerced).
    pub fn parse_token(s: &str) -> Option<KeyClass> {
        match s {
            "tenant" => Some(KeyClass::Tenant),
            "blob" => Some(KeyClass::Blob),
            other => other.strip_prefix("subject:").and_then(|id| {
                // A subject token must carry a non-empty id (`subject:` alone is malformed).
                if id.is_empty() {
                    None
                } else {
                    Some(KeyClass::Subject(id.to_string()))
                }
            }),
        }
    }
}

/// The frozen `pii_key_ref` that travels with every ciphertext (storage.md §4, copied byte-exact):
/// `kms://<tenant>/<dek-epoch>/<class>`, `<class> ∈ {tenant, subject:<id>, blob}`. It names WHICH
/// DEK (which tenant, which epoch, which class) sealed a given ciphertext — so rotation/shred
/// operate at the key layer while the ciphertext bytes stay put.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PiiKeyRef {
    /// The tenant whose key hierarchy sealed the ciphertext.
    pub tenant: TenantId,
    /// The DEK epoch (the rotation generation; a re-wrap bumps it). `u64`.
    pub dek_epoch: u64,
    /// The DEK class — the GD-4 granularity (`tenant` / `subject:<id>` / `blob`).
    pub class: KeyClass,
}

impl PiiKeyRef {
    /// Build a `pii_key_ref` from its three fields.
    pub fn new(tenant: TenantId, dek_epoch: u64, class: KeyClass) -> PiiKeyRef {
        PiiKeyRef {
            tenant,
            dek_epoch,
            class,
        }
    }

    /// Render the canonical `kms://<tenant>/<dek-epoch>/<class>` string (§4, byte-exact).
    pub fn to_uri(&self) -> String {
        format!(
            "kms://{}/{}/{}",
            self.tenant.as_str(),
            self.dek_epoch,
            self.class.as_token()
        )
    }

    /// Parse a canonical `kms://<tenant>/<dek-epoch>/<class>` string back into a [`PiiKeyRef`].
    /// Returns `None` for any string that is not exactly that grammar — a malformed key ref is
    /// NEVER silently coerced (a wrong key ref must be a loud parse failure, not a wrong-key read).
    pub fn parse(uri: &str) -> Option<PiiKeyRef> {
        let rest = uri.strip_prefix("kms://")?;
        // Exactly three `/`-separated segments: <tenant> / <dek-epoch> / <class>. The class itself
        // may CONTAIN a `:` (subject:<id>) but never a `/`, so a splitn(3) is exact.
        let mut parts = rest.splitn(3, '/');
        let tenant = parts.next()?;
        let epoch = parts.next()?;
        let class = parts.next()?;
        if tenant.is_empty() {
            return None;
        }
        let dek_epoch: u64 = epoch.parse().ok()?;
        let class = KeyClass::parse_token(class)?;
        Some(PiiKeyRef {
            tenant: TenantId(tenant.to_string()),
            dek_epoch,
            class,
        })
    }
}

impl fmt::Display for PiiKeyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_uri())
    }
}

// ─────────────────────────────── the three key levels ───────────────────────────────

/// A 256-bit symmetric key (the raw AEAD material). Used for the L0 cell root, the L1 KEKs, and
/// the L2 DEKs alike (§4: all AES-256). The bytes are PLAINTEXT key material — they exist only
/// inside the engine, are NEVER exported through a public accessor, and the `Debug` impl redacts
/// them (a key in a log is a key compromise).
#[derive(Clone, PartialEq, Eq)]
struct RawKey([u8; KEY_LEN]);

impl RawKey {
    /// Generate a fresh random 256-bit key from the OS CSPRNG.
    fn generate() -> RawKey {
        let key = Aes256Gcm::generate_key(OsRng);
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(key.as_slice());
        RawKey(bytes)
    }

    /// The AEAD cipher keyed by this key.
    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.0))
    }
}

impl fmt::Debug for RawKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redacted — raw key material NEVER enters a log/Debug output.
        f.write_str("RawKey(<redacted 256-bit key>)")
    }
}

/// L0 — the per-cell root key (RK; §4). HSM/sealed in production; on this in-cell software floor
/// it is a process-held root the [`KmsEngine`] never exports. It wraps the L1 tenant KEKs; it is
/// the most protected key.
#[derive(Debug)]
pub struct CellRoot {
    root: RawKey,
}

impl Default for CellRoot {
    fn default() -> Self {
        Self::generate()
    }
}

impl CellRoot {
    /// Generate a fresh cell root (per-cell, never exported).
    pub fn generate() -> CellRoot {
        CellRoot {
            root: RawKey::generate(),
        }
    }

    /// Wrap (envelope-encrypt) a KEK's plaintext under the cell root — the L0→L1 seal
    /// (AES-256-GCM). The root NEVER leaves this method; only the wrapped KEK is stored.
    fn wrap_kek(&self, kek_plain: &RawKey) -> WrappedKey {
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ct = self
            .root
            .cipher()
            .encrypt(&nonce, kek_plain.0.as_slice())
            .expect("AES-256-GCM wrap KEK under root");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        WrappedKey {
            nonce: n,
            wrapped: ct,
        }
    }

    /// Unwrap a KEK sealed by [`Self::wrap_kek`] back to its plaintext. Returns `None` if it does
    /// not authenticate under the root (tamper / wrong root) — never a silent wrong key.
    fn unwrap_kek(&self, w: &WrappedKey) -> Option<RawKey> {
        let plain = self
            .root
            .cipher()
            .decrypt(Nonce::from_slice(&w.nonce), w.wrapped.as_slice())
            .ok()?;
        if plain.len() != KEY_LEN {
            return None;
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&plain);
        Some(RawKey(bytes))
    }

    /// **Seal (envelope-encrypt) this cell root UNDER THE OPERATOR-HELD SEAL KEY** — the software
    /// analogue of an HSM unseal key (the MR-025 software-sealed floor). The result ([`SealedRoot`])
    /// is the ONLY form the L0 root EVER rests in at rest: the root NEVER persists in plaintext. The
    /// seal key is supplied at BOOT from the environment/config and never rests in the DB. Reuses the
    /// same vetted AES-256-GCM AEAD — never a hand-rolled cipher.
    pub fn seal(&self, seal_key: &SealKey) -> SealedRoot {
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ct = seal_key
            .cipher()
            .encrypt(&nonce, self.root.0.as_slice())
            .expect("AES-256-GCM seal cell root under the seal key");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        SealedRoot {
            nonce: n,
            ciphertext: ct,
        }
    }

    /// Unseal a [`SealedRoot`] back into a usable cell root under the seal key. Returns `None` if it
    /// does not authenticate — a WRONG or absent seal key (or a tampered/corrupt sealed root). The
    /// caller MUST then **fail closed + loud** and NEVER generate a fresh root (that would orphan
    /// every existing ciphertext = unrecoverable data, the worst outcome, §7.5).
    pub fn unseal(seal_key: &SealKey, sealed: &SealedRoot) -> Option<CellRoot> {
        let plain = seal_key
            .cipher()
            .decrypt(
                Nonce::from_slice(&sealed.nonce),
                sealed.ciphertext.as_slice(),
            )
            .ok()?;
        if plain.len() != KEY_LEN {
            return None;
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&plain);
        Some(CellRoot {
            root: RawKey(bytes),
        })
    }
}

/// The operator-held **seal key** — the root-of-trust on this software floor (the software analogue
/// of an HSM unseal key, the MR-025 software-sealed design). 256-bit AEAD key material supplied at
/// BOOT from the environment/config ([`SealKey::from_encoded`] over e.g. `MYELIN_KMS_SEAL_KEY`); the
/// [`CellRoot`] rests ONLY sealed under it ([`CellRoot::seal`]). It NEVER rests in the database (it
/// is the env-supplied unseal key), is NEVER logged / `Debug`-printed (redacted below), and is NEVER
/// serialized (no `serde` derive) — a seal key in a log or a row is a TOTAL key compromise.
#[derive(Clone, PartialEq, Eq)]
pub struct SealKey(RawKey);

impl SealKey {
    /// Build a seal key from raw 256-bit material (the test seam / a config layer that already
    /// decoded the bytes). The material is never exported back out of the key.
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> SealKey {
        SealKey(RawKey(bytes))
    }

    /// Decode a seal key from its HEX-encoded form (64 hex chars == 32 bytes) — the at-boot
    /// env/config encoding (e.g. `MYELIN_KMS_SEAL_KEY`). A non-hex string or a wrong length is a LOUD
    /// [`SealKeyError`] (fail-closed at boot — never a silently-truncated or all-zero key).
    pub fn from_encoded(s: &str) -> Result<SealKey, SealKeyError> {
        let decoded = hex::decode(s.trim()).map_err(|e| SealKeyError::Decode(e.to_string()))?;
        if decoded.len() != KEY_LEN {
            return Err(SealKeyError::WrongLength(decoded.len()));
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&decoded);
        Ok(SealKey(RawKey(bytes)))
    }

    /// The AEAD cipher keyed by this seal key (the SAME vetted AES-256-GCM the rest of the engine
    /// uses — EI-01 §7, never a hand-rolled cipher).
    fn cipher(&self) -> Aes256Gcm {
        self.0.cipher()
    }
}

impl fmt::Debug for SealKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redacted — the seal key NEVER enters a log/Debug output (it is the unseal root-of-trust).
        f.write_str("SealKey(<redacted seal key>)")
    }
}

/// Why a [`SealKey`] could not be decoded from its env/config form (fail-closed at boot). Carries no
/// key material (only the structural fault), so it is safe to log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SealKeyError {
    /// The string was not valid hex (the underlying decode error — never includes key bytes).
    Decode(String),
    /// The decoded key was not exactly 32 bytes (256-bit).
    WrongLength(usize),
}

impl fmt::Display for SealKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SealKeyError::Decode(e) => write!(
                f,
                "KMS seal key is not valid hex (a 256-bit key as 64 hex chars is required): {e}"
            ),
            SealKeyError::WrongLength(n) => write!(
                f,
                "KMS seal key decoded to {n} bytes; a 256-bit (32-byte) key is required"
            ),
        }
    }
}

impl std::error::Error for SealKeyError {}

/// The at-rest form of the cell root: the L0 root AES-256-GCM-encrypted UNDER THE SEAL KEY
/// ([`CellRoot::seal`]). This — NEVER the plaintext root — is what the durable KMS store persists.
/// Both fields are CIPHERTEXT (safe to store / `Debug`); the root plaintext is recoverable ONLY by
/// [`CellRoot::unseal`] with the correct seal key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedRoot {
    /// The per-seal random AES-GCM nonce.
    pub nonce: [u8; NONCE_LEN],
    /// The AES-256-GCM ciphertext+tag of the 32-byte root plaintext, sealed under the seal key.
    pub ciphertext: Vec<u8>,
}

/// A key sealed (wrapped) under its parent key — the at-rest form of a KEK (under the cell root).
/// Mirrors [`WrappedDek`] one level up; kept private (the KEK-under-root wrapping is engine-
/// internal — never handed to a caller).
#[derive(Clone, Debug, PartialEq, Eq)]
struct WrappedKey {
    nonce: [u8; NONCE_LEN],
    wrapped: Vec<u8>,
}

/// An L1 tenant KEK identifier — one per `(tenant, region)` (§4). Destroying the KEK is
/// tenant-granularity crypto-shred (the tenant-offboard lever).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KekId {
    /// The tenant this KEK belongs to.
    pub tenant: TenantId,
    /// The region this KEK is pinned to (the `(tenant, region)` granularity, §4).
    pub region: Region,
}

impl KekId {
    /// Build a KEK id for a `(tenant, region)`.
    pub fn new(tenant: TenantId, region: Region) -> KekId {
        KekId { tenant, region }
    }
}

/// An L2 DEK identifier — `(tenant, class)` at the CURRENT epoch (the epoch lives in the
/// [`PiiKeyRef`] travelling with the ciphertext, so the engine resolves "the DEK that sealed THIS
/// ciphertext" by `(tenant, class, epoch)`). A per-subject DEK is a DISTINCT id from the tenant
/// DEK (the GD-4 subject-granular lever).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DekId {
    /// The tenant whose KEK wraps this DEK.
    pub tenant: TenantId,
    /// The DEK class (GD-4 granularity).
    pub class: KeyClass,
}

impl DekId {
    /// Build a DEK id for a `(tenant, class)`.
    pub fn new(tenant: TenantId, class: KeyClass) -> DekId {
        DekId { tenant, class }
    }
}

/// A DEK sealed (wrapped) under its KEK — the on-disk/at-rest form of a working key (§4: the DEK
/// is stored ENVELOPE-ENCRYPTED, never bare). `wrapped` is the AES-256-GCM ciphertext of the DEK
/// plaintext; `nonce` is the per-wrap random nonce. A [`WrappedDek`] is what a backup stores —
/// useless once its KEK is destroyed (crypto-shred reaches backups by construction, §7.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrappedDek {
    /// The wrapping nonce (96-bit, per-wrap random).
    pub nonce: [u8; NONCE_LEN],
    /// The AES-256-GCM ciphertext+tag of the 32-byte DEK plaintext.
    pub wrapped: Vec<u8>,
    /// The epoch of the KEK that sealed this DEK (so a rotation can find what to re-wrap).
    pub kek_epoch: u64,
}

/// A resolved (unwrapped) DEK handle — the plaintext working key, in memory, ready to
/// encrypt/decrypt a ciphertext. It is what the fail-static read path caches (a resolved DEK
/// survives a transient KMS hiccup for a bounded TTL). The bytes are redacted from `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct DekHandle {
    key: RawKey,
}

impl DekHandle {
    /// Build a [`DekHandle`] directly from raw 256-bit key material. Used by the [`KeyOrigin`]
    /// HYOK origin (and its customer-key-service test doubles), whose key plaintext arrives from the
    /// customer's CALL-OUT (it is the customer's key, returned transiently by the customer service —
    /// Myelin never holds it at rest). The material is NEVER exported back out of the handle.
    ///
    /// [`KeyOrigin`]: crate::key_origin::KeyOrigin
    pub fn from_raw(bytes: [u8; KEY_LEN]) -> DekHandle {
        DekHandle { key: RawKey(bytes) }
    }

    /// Encrypt `plaintext` under this DEK (AES-256-GCM), returning `(nonce, ciphertext)`.
    pub fn seal(&self, plaintext: &[u8]) -> ([u8; NONCE_LEN], Vec<u8>) {
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ct = self
            .key
            .cipher()
            .encrypt(&nonce, plaintext)
            .expect("AES-256-GCM seal");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        (n, ct)
    }

    /// Decrypt a `(nonce, ciphertext)` sealed by [`Self::seal`]. Returns `None` if the ciphertext
    /// does not authenticate under this DEK (a wrong key / tampered ciphertext is a loud failure,
    /// never silently wrong plaintext).
    pub fn open(&self, nonce: &[u8; NONCE_LEN], ciphertext: &[u8]) -> Option<Vec<u8>> {
        self.key
            .cipher()
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .ok()
    }
}

impl fmt::Debug for DekHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DekHandle(<redacted resolved DEK>)")
    }
}

// ─────────────────────────────── engine errors ───────────────────────────────

/// A KMS engine operation failure. Every variant is a LOUD typed error — a key operation NEVER
/// degrades into a silent wrong/empty result (a wrong-key read must fail, not return plaintext).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KmsError {
    /// No KEK exists for the `(tenant, region)` — it was never created, or it was DESTROYED
    /// (crypto-shred). A DEK under a destroyed KEK is unrecoverable (the tenant-offboard lever) —
    /// this is the correct, loud failure, NOT a fall-through to plaintext.
    KekUnavailable(KekId),
    /// No DEK exists for the `(tenant, class)` — never created, or DESTROYED (per-subject
    /// crypto-shred). The subject's ciphertext is unrecoverable; the read fails loudly.
    DekUnavailable(DekId),
    /// The wrapped DEK did not authenticate under its KEK (tamper / wrong KEK / a re-wrap under a
    /// destroyed-then-recreated KEK). NEVER a silent wrong-key unwrap.
    UnwrapFailed(DekId),
    /// **The durable write-through failed (MR-009b Wave 5 / SI-006).** A freshly minted key (or a
    /// rotation's re-wrap) could NOT be persisted to the durable KMS store, so the in-memory
    /// mutation was rolled back and the operation REFUSED — a key that does not survive a restart
    /// is never handed out (that would be the silent-key-loss floor this wave closes). Carries only
    /// the structural fault text (no key material) — safe to log.
    Durability(String),
}

impl fmt::Display for KmsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KmsError::KekUnavailable(id) => write!(
                f,
                "KMS: no KEK for tenant={} region={} (never created, or crypto-shredded — \
                 a DEK under a destroyed KEK is unrecoverable; this is the loud failure, NOT \
                 a plaintext fall-through)",
                id.tenant.as_str(),
                id.region.as_str()
            ),
            KmsError::DekUnavailable(id) => write!(
                f,
                "KMS: no DEK for tenant={} class={} (never created, or per-subject \
                 crypto-shredded — the subject ciphertext is unrecoverable)",
                id.tenant.as_str(),
                id.class.as_token()
            ),
            KmsError::UnwrapFailed(id) => write!(
                f,
                "KMS: wrapped DEK for tenant={} class={} failed to authenticate under its KEK \
                 (tamper / wrong KEK) — refused, NEVER a silent wrong-key unwrap",
                id.tenant.as_str(),
                id.class.as_token()
            ),
            KmsError::Durability(e) => write!(
                f,
                "KMS: durable write-through FAILED — the key operation was rolled back and refused \
                 (a key that does not survive a restart is never handed out; SI-006): {e}"
            ),
        }
    }
}

impl std::error::Error for KmsError {}

// ─────────────────────────────── the engine ───────────────────────────────

/// One stored KEK: the KEK sealed UNDER THE CELL ROOT (never bare at rest — the L0→L1 envelope),
/// plus its current epoch. To use the KEK, the engine unwraps it under the root; the plaintext
/// KEK exists only transiently inside an operation.
struct StoredKek {
    wrapped: WrappedKey,
    epoch: u64,
}

/// A wrapped KEK exported for DURABLE persistence — the L0→L1 envelope at rest (the KEK sealed under
/// the cell root) plus its epoch. The KEK plaintext is NOT here (it is recoverable only by unwrapping
/// under the root); this is the ciphertext form the durable KMS store persists, mirroring
/// [`WrappedDek`] one level up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportedKek {
    /// The wrapping nonce (96-bit, per-wrap random).
    pub nonce: [u8; NONCE_LEN],
    /// The AES-256-GCM ciphertext+tag of the 32-byte KEK plaintext, sealed under the cell root.
    pub wrapped: Vec<u8>,
    /// The KEK's current epoch (a rotation bumps it).
    pub epoch: u64,
}

/// A full DURABLE snapshot of a cell's KMS key material (MR-025): the SEALED cell root (under the
/// seal key), the wrapped KEKs (under the root), and the wrapped DEKs (under their KEKs) — everything
/// a clean target needs (WITH the same seal key) to recover EVERY encrypted column across a
/// restart/restore. A crypto-shredded key is EXCLUDED (it is absent from the engine's maps, so it
/// never enters here — it stays dead across a restore, §7.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KmsDurableSnapshot {
    /// The cell root, sealed under the seal key (the only at-rest form of the root).
    pub sealed_root: SealedRoot,
    /// Every live KEK, wrapped under the root.
    pub keks: Vec<(KekId, ExportedKek)>,
    /// Every live DEK (of a still-live tenant), wrapped under its KEK, with its DEK epoch.
    pub deks: Vec<(DekId, WrappedDek, u64)>,
}

/// The in-process CRYPTO CORE of the KMS engine — the L0 cell root + the working set of L1 KEKs
/// and L2 DEKs (stored ENVELOPE-WRAPPED, [`WrappedDek`], under their KEKs). Every operation walks
/// the hierarchy: a DEK is never resolved without its KEK, a KEK never without the cell root.
///
/// **This is NOT the durable system-of-record (MR-009b Wave 5).** The public [`KmsEngine`] wraps
/// this core behind a backend split ([`KmsBackend`]): on the PRODUCTION `Durable` backend the core
/// is the working set HYDRATED from the PG store at boot (`kms_durable::load_or_generate`) and
/// every mutation WRITES THROUGH to the `kms_sealed_root`/`kms_wrapped_kek`/`kms_wrapped_dek`
/// tables; on the `test-support`-gated `Memory` backend it is the DB-free test double (fresh root
/// per process — nothing survives a restart, which is exactly why it is a double).
///
/// `destroy_kek` / `destroy_dek` are the crypto-shred levers; a destroyed key is removed from the
/// working set AND excluded from [`backup_snapshot`](Self::backup_snapshot) (it must stay dead
/// across a restore, §7.5).
pub(crate) struct KmsCore {
    root: CellRoot,
    /// L1: one KEK per `(tenant, region)`.
    keks: Mutex<BTreeMap<KekId, StoredKek>>,
    /// L2: the wrapped DEKs, keyed by `(tenant, class)` at their current epoch. The
    /// [`PiiKeyRef`] epoch travelling with a ciphertext selects the right generation; a rotation
    /// re-wraps in place and bumps the stored epoch.
    deks: Mutex<BTreeMap<DekId, (WrappedDek, u64 /* dek_epoch */)>>,
}

impl KmsCore {
    /// Stand up a fresh core over a generated cell root (one per cell). It mints a fresh root per
    /// process, so NOTHING it sealed survives a restart — this is what makes the `Memory` backend a
    /// TEST DOUBLE. The PRODUCTION path is the durable `load_or_generate` ([`crate::kms_durable`],
    /// MR-025), which recovers the sealed root + wrapped KEKs/DEKs from the store across a restart.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fresh() -> KmsCore {
        KmsCore::from_root(CellRoot::generate())
    }

    /// Stand up a core over an EXISTING cell root — the durable load path (MR-025): the root has
    /// just been UNSEALED from the store under the seal key, or freshly generated on a genuine first
    /// boot. The KEK/DEK maps start empty; the durable loader then
    /// [`install_wrapped_kek`](Self::install_wrapped_kek) /
    /// [`install_wrapped_dek`](Self::install_wrapped_dek) the persisted wrapped key material (all
    /// wrapped under THIS same root, the durable invariant).
    pub fn from_root(root: CellRoot) -> KmsCore {
        KmsCore {
            root,
            keks: Mutex::new(BTreeMap::new()),
            deks: Mutex::new(BTreeMap::new()),
        }
    }

    /// Install a KEK loaded from the durable store — its wrapped (under-the-root) form + epoch. The
    /// wrapped bytes MUST have been wrapped under THIS engine's root (the durable invariant: the
    /// persisted KEKs are wrapped under the persisted root that was just unsealed).
    pub fn install_wrapped_kek(
        &self,
        id: KekId,
        nonce: [u8; NONCE_LEN],
        wrapped: Vec<u8>,
        epoch: u64,
    ) {
        let mut keks = self.keks.lock().expect("KMS keks poisoned");
        keks.insert(
            id,
            StoredKek {
                wrapped: WrappedKey { nonce, wrapped },
                epoch,
            },
        );
    }

    /// Install a DEK loaded from the durable store — its wrapped (under-the-KEK) form + DEK epoch.
    pub fn install_wrapped_dek(&self, id: DekId, dek: WrappedDek, dek_epoch: u64) {
        let mut deks = self.deks.lock().expect("KMS deks poisoned");
        deks.insert(id, (dek, dek_epoch));
    }

    /// Export this engine's cell root in its SEALED at-rest form (under the seal key) — what the
    /// durable store persists. The plaintext root never leaves the engine.
    pub fn export_sealed_root(&self, seal_key: &SealKey) -> SealedRoot {
        self.root.seal(seal_key)
    }

    /// Export one KEK's wrapped (under-the-root) form for persistence, or `None` if absent/destroyed.
    pub fn export_kek(&self, id: &KekId) -> Option<ExportedKek> {
        let keks = self.keks.lock().expect("KMS keks poisoned");
        keks.get(id).map(|sk| ExportedKek {
            nonce: sk.wrapped.nonce,
            wrapped: sk.wrapped.wrapped.clone(),
            epoch: sk.epoch,
        })
    }

    /// Export one DEK's wrapped (under-the-KEK) form + its DEK epoch, or `None` if absent/shredded.
    pub fn export_dek(&self, id: &DekId) -> Option<(WrappedDek, u64)> {
        let deks = self.deks.lock().expect("KMS deks poisoned");
        deks.get(id).map(|(w, e)| (w.clone(), *e))
    }

    /// Export EVERY live DEK's wrapped form + DEK epoch (for a full write-through / mirror).
    pub fn export_deks(&self) -> Vec<(DekId, WrappedDek, u64)> {
        let deks = self.deks.lock().expect("KMS deks poisoned");
        deks.iter()
            .map(|(id, (w, e))| (id.clone(), w.clone(), *e))
            .collect()
    }

    /// Provision (or fetch) the L1 KEK for a `(tenant, region)`. Idempotent: a second call for the
    /// same id returns the existing KEK's epoch (it does NOT silently rotate). The KEK material is
    /// wrapped-by-the-root conceptually; on this floor it is held sealed in-process (never
    /// exported).
    pub fn ensure_kek(&self, id: &KekId) -> u64 {
        self.ensure_kek_tracked(id).0
    }

    /// [`Self::ensure_kek`] + whether the KEK was FRESHLY minted (vs already present). The durable
    /// backend keys its write-through on the freshness bit (a fresh mint MUST persist before it is
    /// handed out; an existing KEK already has its durable row).
    pub fn ensure_kek_tracked(&self, id: &KekId) -> (u64, bool) {
        let mut keks = self.keks.lock().expect("KMS keks poisoned");
        if let Some(existing) = keks.get(id) {
            return (existing.epoch, false);
        }
        // A fresh KEK is generated and immediately sealed under the cell root (the L0→L1 envelope);
        // only the wrapped form is stored — the bare KEK never rests in the map.
        let wrapped = self.root.wrap_kek(&RawKey::generate());
        keks.insert(id.clone(), StoredKek { wrapped, epoch: 0 });
        (0, true)
    }

    /// Unwrap the KEK for `id` under the cell root into its transient plaintext. Loud failure if
    /// the KEK is unavailable (destroyed / never created) or fails to authenticate under the root.
    fn open_kek(&self, id: &KekId) -> Result<RawKey, KmsError> {
        let keks = self.keks.lock().expect("KMS keks poisoned");
        let kek = keks
            .get(id)
            .ok_or_else(|| KmsError::KekUnavailable(id.clone()))?;
        self.root
            .unwrap_kek(&kek.wrapped)
            .ok_or_else(|| KmsError::KekUnavailable(id.clone()))
    }

    /// Provision (or fetch) the L2 DEK for `(tenant, class)` in `region`, returning its
    /// [`PiiKeyRef`]. Idempotent per `(tenant, class)`. A per-SUBJECT class yields a DISTINCT DEK
    /// from the tenant class (GD-4). The DEK is generated, wrapped under the tenant KEK, and
    /// stored ONLY in wrapped form (envelope encryption — never a bare DEK at rest). Fails loudly
    /// if the KEK is unavailable (never fabricates a key).
    #[cfg(any(test, feature = "test-support"))]
    pub fn ensure_dek(
        &self,
        tenant: &TenantId,
        region: &Region,
        class: KeyClass,
    ) -> Result<PiiKeyRef, KmsError> {
        self.ensure_dek_tracked(tenant, region, class).map(|(k, _)| k)
    }

    /// [`Self::ensure_dek`] + whether the DEK was FRESHLY minted. The durable backend keys its
    /// write-through on the freshness bit (a fresh mint MUST persist before it is handed out).
    pub fn ensure_dek_tracked(
        &self,
        tenant: &TenantId,
        region: &Region,
        class: KeyClass,
    ) -> Result<(PiiKeyRef, bool), KmsError> {
        let kek_id = KekId::new(tenant.clone(), region.clone());
        let dek_id = DekId::new(tenant.clone(), class.clone());
        {
            // Fast path: the DEK already exists → return its ref at its current epoch.
            let deks = self.deks.lock().expect("KMS deks poisoned");
            if let Some((_, dek_epoch)) = deks.get(&dek_id) {
                return Ok((PiiKeyRef::new(tenant.clone(), *dek_epoch, class), false));
            }
        }
        // Generate a fresh DEK, wrap it under the tenant KEK, store the wrapped form only.
        let dek_plain = RawKey::generate();
        let wrapped = self.wrap_dek(&kek_id, &dek_plain)?;
        let mut deks = self.deks.lock().expect("KMS deks poisoned");
        // Re-check under the lock (another thread may have created it) — keep idempotent.
        if let Some((_, dek_epoch)) = deks.get(&dek_id) {
            return Ok((PiiKeyRef::new(tenant.clone(), *dek_epoch, class), false));
        }
        let dek_epoch = 0u64;
        deks.insert(dek_id, (wrapped, dek_epoch));
        Ok((PiiKeyRef::new(tenant.clone(), dek_epoch, class), true))
    }

    /// Wrap (envelope-encrypt) a DEK's plaintext under the named KEK — AES-256-GCM. Fails loudly
    /// if the KEK does not exist (never wraps under a fabricated key).
    fn wrap_dek(&self, kek_id: &KekId, dek_plain: &RawKey) -> Result<WrappedDek, KmsError> {
        let kek = self.open_kek(kek_id)?;
        let kek_epoch = {
            let keks = self.keks.lock().expect("KMS keks poisoned");
            keks.get(kek_id)
                .map(|k| k.epoch)
                .ok_or_else(|| KmsError::KekUnavailable(kek_id.clone()))?
        };
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let wrapped = kek
            .cipher()
            .encrypt(&nonce, dek_plain.0.as_slice())
            .expect("AES-256-GCM wrap");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        Ok(WrappedDek {
            nonce: n,
            wrapped,
            kek_epoch,
        })
    }

    /// Resolve (unwrap) the DEK named by a [`PiiKeyRef`] into a usable [`DekHandle`] — the read
    /// path's key-resolution step. Walks the hierarchy: find the KEK for the tenant's region,
    /// decrypt the stored [`WrappedDek`] under it. Every failure is LOUD ([`KmsError`]) — a
    /// destroyed KEK ([`KmsError::KekUnavailable`]), a shredded DEK ([`KmsError::DekUnavailable`]),
    /// or a non-authenticating unwrap ([`KmsError::UnwrapFailed`]) — **NEVER a plaintext-without-key
    /// fall-through** (the 0-fail-open invariant).
    pub fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError> {
        let kek_id = KekId::new(key_ref.tenant.clone(), region.clone());
        let dek_id = DekId::new(key_ref.tenant.clone(), key_ref.class.clone());

        let deks = self.deks.lock().expect("KMS deks poisoned");
        let (wrapped, _epoch) = deks
            .get(&dek_id)
            .ok_or_else(|| KmsError::DekUnavailable(dek_id.clone()))?;
        let wrapped = wrapped.clone();
        drop(deks);

        // Open the KEK under the cell root (the L0→L1 step), then unwrap the DEK under it
        // (the L1→L2 step) — a DEK is never resolved without walking the full hierarchy.
        let kek = self.open_kek(&kek_id)?;
        let plain = kek
            .cipher()
            .decrypt(
                Nonce::from_slice(&wrapped.nonce),
                wrapped.wrapped.as_slice(),
            )
            .map_err(|_| KmsError::UnwrapFailed(dek_id.clone()))?;
        // The decrypted plaintext MUST be exactly a 256-bit key — anything else is a corrupt/forged
        // envelope, refused (never a short/long key silently coerced).
        if plain.len() != KEY_LEN {
            return Err(KmsError::UnwrapFailed(dek_id));
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&plain);
        Ok(DekHandle { key: RawKey(bytes) })
    }

    /// Rotate a tenant's KEK = **envelope re-wrap, not bulk re-encryption** (§4; `O(keys)`, not
    /// `O(data)`). Mints a NEW KEK (new epoch), re-wraps every DEK currently under the old KEK
    /// using the new one, and bumps each DEK's epoch. Forward-only — the old KEK epoch is gone, a
    /// rotation never rolls back. The DEK PLAINTEXT (and therefore every ciphertext's content) is
    /// untouched: a ciphertext sealed before the rotation still decrypts to the same plaintext,
    /// because the DEK material did not change — only its wrapping did. Returns the new KEK epoch.
    pub fn rotate_kek(&self, id: &KekId) -> Result<u64, KmsError> {
        // First resolve every DEK plaintext under the OLD KEK (so we can re-wrap under the new one).
        let dek_ids: Vec<DekId> = {
            let deks = self.deks.lock().expect("KMS deks poisoned");
            deks.keys()
                .filter(|d| d.tenant == id.tenant)
                .cloned()
                .collect()
        };
        let mut plains: Vec<(DekId, RawKey)> = Vec::with_capacity(dek_ids.len());
        for dek_id in &dek_ids {
            let key_ref = PiiKeyRef::new(id.tenant.clone(), 0, dek_id.class.clone());
            let handle = self.resolve_dek(&key_ref, &id.region)?;
            plains.push((dek_id.clone(), handle.key));
        }

        // Mint the new KEK (new epoch) in place — generated, re-sealed under the cell root.
        let new_wrapped = self.root.wrap_kek(&RawKey::generate());
        let new_epoch = {
            let mut keks = self.keks.lock().expect("KMS keks poisoned");
            let kek = keks
                .get_mut(id)
                .ok_or_else(|| KmsError::KekUnavailable(id.clone()))?;
            kek.wrapped = new_wrapped;
            kek.epoch += 1;
            kek.epoch
        };

        // Re-wrap every DEK under the new KEK; bump each DEK epoch. The plaintext is unchanged.
        for (dek_id, plain) in plains {
            let wrapped = self.wrap_dek(id, &plain)?;
            let mut deks = self.deks.lock().expect("KMS deks poisoned");
            if let Some((slot, dek_epoch)) = deks.get_mut(&dek_id) {
                *slot = wrapped;
                *dek_epoch += 1;
            }
        }
        Ok(new_epoch)
    }

    /// **Crypto-shred at L1** (§5): destroy the tenant KEK. Every DEK under it becomes
    /// unrecoverable — its [`WrappedDek`] is ciphertext under a key that no longer exists (live
    /// AND in every backup, §7.5). This is the tenant-offboard lever (tenant-granularity erasure
    /// in ONE operation). Returns `true` if a KEK was present to destroy. The DEK ROWS remain
    /// (so a resolve fails loudly as [`KmsError::KekUnavailable`], never silently "not found") but
    /// are forever unwrappable.
    pub fn destroy_kek(&self, id: &KekId) -> bool {
        let mut keks = self.keks.lock().expect("KMS keks poisoned");
        keks.remove(id).is_some()
    }

    /// **Crypto-shred at L2** (§5): destroy a single per-subject (or per-tenant/blob) DEK. That
    /// class's ciphertext becomes unrecoverable without touching any OTHER subject's key (the GD-4
    /// individual-erasure lever — one person's Art. 17 erasure, the tenant untouched). Returns
    /// `true` if a DEK was present to destroy.
    pub fn destroy_dek(&self, id: &DekId) -> bool {
        let mut deks = self.deks.lock().expect("KMS deks poisoned");
        deks.remove(id).is_some()
    }

    /// A backup snapshot of the engine's key material — the wrapped DEKs ONLY (§7.5: a backup
    /// stores ciphertext under a KEK; a crypto-shredded key is **excluded** — it must stay dead
    /// across a restore). A DEK whose KEK has been destroyed is NOT emitted, so restoring this
    /// snapshot can never resurrect a crypto-shredded tenant. (The KEKs themselves are sealed
    /// under the cell root / HSM and backed up only while the tenant is live, §7.5 — that backing
    /// is the production-hardening follow-on; this snapshot models the wrapped-DEK exclusion the
    /// restore-verify gate STOR-D3 asserts.)
    pub fn backup_snapshot(&self) -> Vec<(DekId, WrappedDek)> {
        let keks = self.keks.lock().expect("KMS keks poisoned");
        let deks = self.deks.lock().expect("KMS deks poisoned");
        deks.iter()
            .filter(|(dek_id, _)| {
                // Exclude a DEK whose tenant has NO live KEK (crypto-shredded → stays dead). We
                // cannot know the region from the DekId alone, so a DEK is included iff SOME KEK
                // for its tenant is still live (a live tenant); a fully-offboarded tenant (all
                // KEKs destroyed) is excluded entirely.
                keks.keys().any(|k| k.tenant == dek_id.tenant)
            })
            .map(|(dek_id, (wrapped, _epoch))| (dek_id.clone(), wrapped.clone()))
            .collect()
    }

    /// **The extended backup snapshot (MR-025): the SEALED root + the wrapped KEKs + the wrapped
    /// DEKs.** This EXTENDS [`backup_snapshot`](Self::backup_snapshot) (which carries the wrapped
    /// DEKs ONLY) to ALSO carry the sealed cell root + the wrapped KEKs, so a restore to a clean
    /// target (WITH the same seal key) recovers EVERY encrypted column — not just the DEK ciphertext
    /// whose wrapping KEK + root would otherwise be gone (the very thing today's fresh-root-per-process
    /// loses). A crypto-shredded key stays excluded: a DEK whose tenant has no live KEK is omitted
    /// (the SAME §7.5 rule as `backup_snapshot`), and a destroyed KEK/DEK is already absent from the
    /// maps — so it cannot be resurrected by a restore.
    pub fn backup_snapshot_durable(&self, seal_key: &SealKey) -> KmsDurableSnapshot {
        let keks = self.keks.lock().expect("KMS keks poisoned");
        let deks = self.deks.lock().expect("KMS deks poisoned");
        let kek_list: Vec<(KekId, ExportedKek)> = keks
            .iter()
            .map(|(id, sk)| {
                (
                    id.clone(),
                    ExportedKek {
                        nonce: sk.wrapped.nonce,
                        wrapped: sk.wrapped.wrapped.clone(),
                        epoch: sk.epoch,
                    },
                )
            })
            .collect();
        let dek_list: Vec<(DekId, WrappedDek, u64)> = deks
            .iter()
            // Exclude a DEK whose tenant has NO live KEK (crypto-shredded → stays dead across
            // restore, §7.5) — the SAME rule `backup_snapshot` applies.
            .filter(|(dek_id, _)| keks.keys().any(|k| k.tenant == dek_id.tenant))
            .map(|(dek_id, (wrapped, epoch))| (dek_id.clone(), wrapped.clone(), *epoch))
            .collect();
        KmsDurableSnapshot {
            sealed_root: self.root.seal(seal_key),
            keks: kek_list,
            deks: dek_list,
        }
    }

    /// Envelope-wrap raw DEK material under a tenant's KEK (the L1→L2 seal) — the primitive the
    /// [`KeyOrigin`](crate::key_origin::KeyOrigin) platform/BYOK origins call to wrap a freshly
    /// minted DEK. Ensures the tenant KEK exists first (so a wrap never fails on a not-yet-onboarded
    /// tenant), then seals the bytes under it. The material is wrapped, never stored bare.
    pub fn wrap_dek_material(
        &self,
        tenant: &TenantId,
        region: &Region,
        material: &[u8; KEY_LEN],
    ) -> Result<WrappedDek, KmsError> {
        let kek_id = KekId::new(tenant.clone(), region.clone());
        self.ensure_kek(&kek_id);
        self.wrap_dek(&kek_id, &RawKey(*material))
    }

    /// Unwrap a [`WrappedDek`] under a tenant's KEK into a usable [`DekHandle`] — the inverse of
    /// [`Self::wrap_dek_material`], the primitive the platform/BYOK [`KeyOrigin`] origins call. A
    /// destroyed KEK / non-authenticating envelope fails LOUDLY ([`KmsError`]) — never a
    /// plaintext-without-key fall-through.
    ///
    /// [`KeyOrigin`]: crate::key_origin::KeyOrigin
    /// The `(keks, deks)` working-set counts — the [`KmsEngine`] `Debug` depth read (counts only,
    /// never key material).
    pub fn counts(&self) -> (usize, usize) {
        let keks = self.keks.lock().map(|k| k.len()).unwrap_or(0);
        let deks = self.deks.lock().map(|d| d.len()).unwrap_or(0);
        (keks, deks)
    }

    pub fn unwrap_dek_material(
        &self,
        tenant: &TenantId,
        region: &Region,
        w: &WrappedDek,
    ) -> Result<DekHandle, KmsError> {
        let kek_id = KekId::new(tenant.clone(), region.clone());
        let kek = self.open_kek(&kek_id)?;
        let plain = kek
            .cipher()
            .decrypt(Nonce::from_slice(&w.nonce), w.wrapped.as_slice())
            .map_err(|_| KmsError::UnwrapFailed(DekId::new(tenant.clone(), KeyClass::Tenant)))?;
        if plain.len() != KEY_LEN {
            return Err(KmsError::UnwrapFailed(DekId::new(
                tenant.clone(),
                KeyClass::Tenant,
            )));
        }
        let mut bytes = [0u8; KEY_LEN];
        bytes.copy_from_slice(&plain);
        Ok(DekHandle { key: RawKey(bytes) })
    }
}

// ─────────────────────────── the backend split (MR-009b Wave 5 / SI-006) ───────────────────────

/// The KMS engine backend (MR-009b Wave 5, the MR-007/008/Wave-2 backend-enum pattern). `Durable`
/// is the always-compiled PRODUCTION default: the working-set [`KmsCore`] hydrated from the
/// `kms_sealed_root`/`kms_wrapped_kek`/`kms_wrapped_dek` tables at boot
/// (`kms_durable::DurableKmsBacking::load_or_generate`, fail-closed + LOUD on a wrong/absent seal
/// key) with EVERY mutation written through to the store, so keys survive a restart (SI-006).
/// `Memory` is the in-memory TEST DOUBLE — compiled ONLY under
/// `#[cfg(any(test, feature = "test-support"))]`: a fresh root per process, nothing survives a
/// restart. The `no-in-memory-durable-store` scanner strips the `test-support`-gated `Memory` arm,
/// so the PRODUCTION-compiled engine presents only the durable backend.
enum KmsBackend {
    /// The in-memory test-double core. **MR-009b Wave 5 — TEST DOUBLE (compiled ONLY under
    /// `#[cfg(any(test, feature = "test-support"))]`).** NOT the production system-of-record.
    #[cfg(any(test, feature = "test-support"))]
    Memory(KmsCore),
    /// The durable software-sealed backend (production, always compiled): the hydrated core + the
    /// PG backing + the sync→async write-through bridge ([`crate::kms_durable::DurableKms`]).
    Durable(DurableKms),
}

/// The self-hostable software KMS engine (Vault-Transit-class) — the in-cell key store behind the
/// [`KmsAdapter`] seam (§4). It holds the L0 cell root, the L1 per-(tenant,region) KEKs, and the
/// L2 DEKs stored ENVELOPE-WRAPPED ([`WrappedDek`]) under their KEKs — via a backend split:
///
/// - **The PRODUCTION default is DURABLE (MR-009b Wave 5 / SI-006):** the engine is constructed by
///   [`crate::kms_durable::DurableKmsBacking::load_or_generate`] (the software-sealed root-of-trust,
///   MR-025 — fail-closed + LOUD on a wrong/absent `MYELIN_KMS_SEAL_KEY`), hydrating the working set
///   from the `kms_sealed_root`/`kms_wrapped_kek`/`kms_wrapped_dek` tables, and EVERY mutation
///   (`ensure_kek`/`ensure_dek`/`rotate_kek`/`destroy_kek`/`destroy_dek`/`wrap_dek_material`)
///   WRITES THROUGH to the store — a key minted through this engine survives a kill-9 restart.
/// - **The in-memory engine ([`KmsEngine::new`]) is a TEST DOUBLE**, compiled ONLY under
///   `#[cfg(any(test, feature = "test-support"))]` (downstream crates reach it via the
///   `myelin-storage/test-support` dev-dependency). It mints a fresh root per process, so nothing
///   it sealed survives a restart — never the production system-of-record.
///
/// `destroy_kek` / `destroy_dek` are the crypto-shred levers; a destroyed key is removed from the
/// store AND excluded from [`backup_snapshot`](Self::backup_snapshot) (it must stay dead across a
/// restore, §7.5). On the durable backend the shred DELETES the durable row FIRST (fail-closed:
/// a shred that cannot reach the store refuses — hard-down — rather than silently resurrecting the
/// key on the next restart).
pub struct KmsEngine {
    backend: KmsBackend,
}

/// The `Default` engine is the in-memory TEST DOUBLE — `#[cfg(any(test, feature = "test-support"))]`
/// only (MR-009b Wave 5). Production builds the durable engine through
/// [`crate::kms_durable::DurableKmsBacking::load_or_generate`].
#[cfg(any(test, feature = "test-support"))]
impl Default for KmsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl KmsEngine {
    /// Stand up a fresh IN-MEMORY engine over a generated cell root — the **test-double** (MR-009b
    /// Wave 5: compiled ONLY under `#[cfg(any(test, feature = "test-support"))]`). It mints a fresh
    /// root per process, so NOTHING it sealed survives a restart. The PRODUCTION constructor is the
    /// durable [`crate::kms_durable::DurableKmsBacking::load_or_generate`] (MR-025), which recovers
    /// the sealed root + wrapped KEKs/DEKs from the store across a restart; this `::new` is the
    /// DB-free unit-test entry point downstream crates reach via the `myelin-storage/test-support`
    /// dev-dependency.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> KmsEngine {
        KmsEngine {
            backend: KmsBackend::Memory(KmsCore::fresh()),
        }
    }

    /// Stand up an IN-MEMORY engine over an EXISTING cell root (the from-snapshot rebuild seam the
    /// restore drills exercise). **A TEST-SUPPORT seam (MR-009b Wave 5)** — the production durable
    /// load path hydrates through [`crate::kms_durable::DurableKmsBacking::load_or_generate`], not
    /// through this constructor.
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_root(root: CellRoot) -> KmsEngine {
        KmsEngine {
            backend: KmsBackend::Memory(KmsCore::from_root(root)),
        }
    }

    /// Wrap the durable backend (the hydrated core + PG backing + bridge) as the public engine —
    /// the PRODUCTION constructor, reached through
    /// [`crate::kms_durable::DurableKmsBacking::load_or_generate`].
    pub(crate) fn durable(backend: DurableKms) -> KmsEngine {
        KmsEngine {
            backend: KmsBackend::Durable(backend),
        }
    }

    /// The crypto core (the working set) behind whichever backend is wired — the read path both
    /// backends share.
    pub(crate) fn core(&self) -> &KmsCore {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core,
            KmsBackend::Durable(d) => d.core(),
        }
    }

    /// Install a KEK loaded from the durable store — its wrapped (under-the-root) form + epoch. The
    /// wrapped bytes MUST have been wrapped under THIS engine's root (the durable invariant). This
    /// is the HYDRATION seam (the durable loader / the from-snapshot rebuild) — it installs into the
    /// in-process working set only and does NOT write through.
    pub fn install_wrapped_kek(
        &self,
        id: KekId,
        nonce: [u8; NONCE_LEN],
        wrapped: Vec<u8>,
        epoch: u64,
    ) {
        self.core().install_wrapped_kek(id, nonce, wrapped, epoch);
    }

    /// Install a DEK loaded from the durable store — its wrapped (under-the-KEK) form + DEK epoch
    /// (the hydration seam; working-set only, no write-through).
    pub fn install_wrapped_dek(&self, id: DekId, dek: WrappedDek, dek_epoch: u64) {
        self.core().install_wrapped_dek(id, dek, dek_epoch);
    }

    /// Export this engine's cell root in its SEALED at-rest form (under the seal key) — what the
    /// durable store persists. The plaintext root never leaves the engine.
    pub fn export_sealed_root(&self, seal_key: &SealKey) -> SealedRoot {
        self.core().export_sealed_root(seal_key)
    }

    /// Export one KEK's wrapped (under-the-root) form for persistence, or `None` if absent/destroyed.
    pub fn export_kek(&self, id: &KekId) -> Option<ExportedKek> {
        self.core().export_kek(id)
    }

    /// Export one DEK's wrapped (under-the-KEK) form + its DEK epoch, or `None` if absent/shredded.
    pub fn export_dek(&self, id: &DekId) -> Option<(WrappedDek, u64)> {
        self.core().export_dek(id)
    }

    /// Export EVERY live DEK's wrapped form + DEK epoch (for a full write-through / mirror).
    pub fn export_deks(&self) -> Vec<(DekId, WrappedDek, u64)> {
        self.core().export_deks()
    }

    /// Provision (or fetch) the L1 KEK for a `(tenant, region)`. Idempotent: a second call for the
    /// same id returns the existing KEK's epoch (it does NOT silently rotate).
    ///
    /// **Durable backend (MR-009b Wave 5):** a FRESHLY minted KEK is written through to the
    /// `kms_wrapped_kek` table before it is handed out. A write-through failure is FAIL-STATIC
    /// HARD-DOWN (the in-memory mint is rolled back and the process panics LOUDLY): a KEK that does
    /// not survive a restart must never be handed out (SI-006 — silent key loss), and this
    /// infallible signature has no error channel. An existing KEK performs no DB write.
    pub fn ensure_kek(&self, id: &KekId) -> u64 {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.ensure_kek(id),
            KmsBackend::Durable(d) => d.ensure_kek(id),
        }
    }

    /// Provision (or fetch) the L2 DEK for `(tenant, class)` in `region`, returning its
    /// [`PiiKeyRef`]. Idempotent per `(tenant, class)`. A per-SUBJECT class yields a DISTINCT DEK
    /// from the tenant class (GD-4). Fails loudly if the KEK is unavailable (never fabricates a key).
    ///
    /// **Durable backend (MR-009b Wave 5):** a FRESHLY minted DEK (and its wrapping KEK row) is
    /// written through to the store before the ref is handed out; a write-through failure ROLLS BACK
    /// the in-memory mint and returns the loud [`KmsError::Durability`] — a key that does not
    /// survive a restart is never handed out.
    pub fn ensure_dek(
        &self,
        tenant: &TenantId,
        region: &Region,
        class: KeyClass,
    ) -> Result<PiiKeyRef, KmsError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.ensure_dek(tenant, region, class),
            KmsBackend::Durable(d) => d.ensure_dek(tenant, region, class),
        }
    }

    /// Resolve (unwrap) the DEK named by a [`PiiKeyRef`] into a usable [`DekHandle`] — the read
    /// path's key-resolution step (both backends resolve from the in-process working set; the
    /// durable backend hydrated it from the store at boot). Every failure is LOUD ([`KmsError`]) —
    /// **NEVER a plaintext-without-key fall-through** (the 0-fail-open invariant).
    pub fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError> {
        self.core().resolve_dek(key_ref, region)
    }

    /// Rotate a tenant's KEK = **envelope re-wrap, not bulk re-encryption** (§4; `O(keys)`, not
    /// `O(data)`). Returns the new KEK epoch.
    ///
    /// **Durable backend (MR-009b Wave 5):** the new wrapped KEK + every re-wrapped DEK row is
    /// written through in ONE PG transaction (atomicity is load-bearing: a partial persist — new
    /// KEK row + old DEK rows — would be unrecoverable after a restart, since the old KEK plaintext
    /// exists nowhere to unwrap the old envelopes). On a write-through failure the loud
    /// [`KmsError::Durability`] is returned and the store atomically holds the PREVIOUS
    /// (pre-rotation) wrapping generation — every ciphertext remains decryptable after a restart
    /// (the DEK material never changed), only the epoch bump is lost (re-run the rotation).
    pub fn rotate_kek(&self, id: &KekId) -> Result<u64, KmsError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.rotate_kek(id),
            KmsBackend::Durable(d) => d.rotate_kek(id),
        }
    }

    /// **Crypto-shred at L1** (§5): destroy the tenant KEK — every DEK under it becomes
    /// unrecoverable (the tenant-offboard lever). Returns `true` if a KEK was present to destroy.
    ///
    /// **Durable backend (MR-009b Wave 5):** the durable `kms_wrapped_kek` row is DELETED FIRST
    /// (§7.5 — the shred must reach the store, or a restart resurrects the offboarded tenant's
    /// key). A delete failure is FAIL-STATIC HARD-DOWN (loud panic, the in-memory key untouched):
    /// this infallible signature has no error channel, and reporting a shred that did not reach the
    /// store would be a silent GDPR erasure failure.
    pub fn destroy_kek(&self, id: &KekId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.destroy_kek(id),
            KmsBackend::Durable(d) => d.destroy_kek(id),
        }
    }

    /// **Crypto-shred at L2** (§5): destroy a single per-subject (or per-tenant/blob) DEK — the
    /// GD-4 individual-erasure lever. Returns `true` if a DEK was present to destroy.
    ///
    /// **Durable backend (MR-009b Wave 5):** the durable `kms_wrapped_dek` row is DELETED FIRST;
    /// a delete failure is FAIL-STATIC HARD-DOWN (loud panic) — same posture as
    /// [`Self::destroy_kek`].
    pub fn destroy_dek(&self, id: &DekId) -> bool {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.destroy_dek(id),
            KmsBackend::Durable(d) => d.destroy_dek(id),
        }
    }

    /// A backup snapshot of the engine's key material — the wrapped DEKs ONLY (§7.5: a
    /// crypto-shredded key is **excluded** — it must stay dead across a restore).
    pub fn backup_snapshot(&self) -> Vec<(DekId, WrappedDek)> {
        self.core().backup_snapshot()
    }

    /// **The extended backup snapshot (MR-025): the SEALED root + the wrapped KEKs + the wrapped
    /// DEKs** — everything a clean target needs (WITH the same seal key) to recover every encrypted
    /// column. A crypto-shredded key stays excluded (§7.5).
    pub fn backup_snapshot_durable(&self, seal_key: &SealKey) -> KmsDurableSnapshot {
        self.core().backup_snapshot_durable(seal_key)
    }

    /// Envelope-wrap raw DEK material under a tenant's KEK (the L1→L2 seal) — the primitive the
    /// [`KeyOrigin`](crate::key_origin::KeyOrigin) platform/BYOK origins call. Ensures the tenant
    /// KEK exists first; the material is wrapped, never stored bare.
    ///
    /// **Durable backend (MR-009b Wave 5):** a freshly ensured KEK is written through (same
    /// fail-static posture as [`Self::ensure_kek`]). The returned [`WrappedDek`] itself is the
    /// CALLER's to persist (the [`KeyOrigin`] holders store it) — it is not a `kms_wrapped_dek` row.
    ///
    /// [`KeyOrigin`]: crate::key_origin::KeyOrigin
    pub fn wrap_dek_material(
        &self,
        tenant: &TenantId,
        region: &Region,
        material: &[u8; KEY_LEN],
    ) -> Result<WrappedDek, KmsError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            KmsBackend::Memory(core) => core.wrap_dek_material(tenant, region, material),
            KmsBackend::Durable(d) => d.wrap_dek_material(tenant, region, material),
        }
    }

    /// Unwrap a [`WrappedDek`] under a tenant's KEK into a usable [`DekHandle`] — the inverse of
    /// [`Self::wrap_dek_material`] (read-only; both backends resolve from the working set). A
    /// destroyed KEK / non-authenticating envelope fails LOUDLY ([`KmsError`]) — never a
    /// plaintext-without-key fall-through.
    pub fn unwrap_dek_material(
        &self,
        tenant: &TenantId,
        region: &Region,
        w: &WrappedDek,
    ) -> Result<DekHandle, KmsError> {
        self.core().unwrap_dek_material(tenant, region, w)
    }
}

impl fmt::Debug for KmsEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Counts only — NEVER any key material.
        let (keks, deks) = self.core().counts();
        f.debug_struct("KmsEngine")
            .field("keks", &keks)
            .field("deks", &deks)
            .finish_non_exhaustive()
    }
}

// ─────────────────────────────── the KMS adapter seam ───────────────────────────────

/// The narrow adapter seam every encrypted store resolves DEKs through (§4: "wire a self-hostable
/// engine — Vault-Transit-class — behind an adapter"). A real deployment swaps the in-cell
/// [`KmsEngine`] for a Vault Transit / HSM-backed implementation behind this trait without
/// touching a caller. The read path ([`KmsReadPath`]) is layered OVER this so the fail-static
/// posture is independent of the backing engine.
pub trait KmsAdapter: Send + Sync {
    /// Resolve the DEK named by a `pii_key_ref` into a usable handle, or fail loudly. An adapter
    /// MUST NEVER return a fabricated/empty handle on an unavailable key — that is the fail-open
    /// the whole posture forbids.
    fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError>;
}

impl KmsAdapter for KmsEngine {
    fn resolve_dek(&self, key_ref: &PiiKeyRef, region: &Region) -> Result<DekHandle, KmsError> {
        KmsEngine::resolve_dek(self, key_ref, region)
    }
}

// ─────────────────────────────── the fail-static read path (STOR-D6) ───────────────────────────

pub use crate::kms_failstatic::{KmsReadError, KmsReadPath, KmsReadResult, KmsReadiness};

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r(s: &str) -> Region {
        Region(s.to_string())
    }

    // ───────────── pii_key_ref (the frozen §4 shape) ─────────────

    #[test]
    fn pii_key_ref_encodes_tenant_epoch_class_exactly() {
        // tenant class
        let kr = PiiKeyRef::new(t("acme"), 0, KeyClass::Tenant);
        assert_eq!(kr.to_uri(), "kms://acme/0/tenant");
        // subject class carries the id
        let kr = PiiKeyRef::new(t("acme"), 3, KeyClass::Subject("u-42".into()));
        assert_eq!(kr.to_uri(), "kms://acme/3/subject:u-42");
        // blob class
        let kr = PiiKeyRef::new(t("acme"), 7, KeyClass::Blob);
        assert_eq!(kr.to_uri(), "kms://acme/7/blob");
    }

    #[test]
    fn pii_key_ref_round_trips_through_parse() {
        for uri in [
            "kms://acme/0/tenant",
            "kms://acme/12/subject:u-99",
            "kms://acme/5/blob",
        ] {
            let kr = PiiKeyRef::parse(uri).expect("parses the canonical grammar");
            assert_eq!(kr.to_uri(), uri, "round-trip is byte-identical");
        }
    }

    #[test]
    fn pii_key_ref_rejects_malformed_uris_loudly() {
        // A malformed ref is NEVER silently coerced (a wrong ref must be a loud None).
        assert!(
            PiiKeyRef::parse("https://acme/0/tenant").is_none(),
            "wrong scheme"
        );
        assert!(PiiKeyRef::parse("kms://acme/0").is_none(), "missing class");
        assert!(
            PiiKeyRef::parse("kms://acme/notanint/tenant").is_none(),
            "non-int epoch"
        );
        assert!(
            PiiKeyRef::parse("kms:///0/tenant").is_none(),
            "empty tenant"
        );
        assert!(
            PiiKeyRef::parse("kms://acme/0/bogus").is_none(),
            "unknown class"
        );
        assert!(
            PiiKeyRef::parse("kms://acme/0/subject:").is_none(),
            "empty subject id"
        );
    }

    #[test]
    fn subject_dek_uri_with_colon_in_class_parses_the_full_id() {
        // The class segment legitimately contains a `:` — splitn(3, '/') keeps it whole.
        let kr = PiiKeyRef::parse("kms://acme/4/subject:alice:bob").expect("parses");
        assert_eq!(kr.class, KeyClass::Subject("alice:bob".into()));
        assert_eq!(kr.dek_epoch, 4);
    }

    // ───────────── the three-level hierarchy: wrap → unwrap round-trip ─────────────

    #[test]
    fn wrap_unwrap_round_trips_a_dek_under_a_kek() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let kr = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("ensure dek");

        // Resolve the DEK and use it to seal+open a payload (the full wrap→unwrap→use round-trip).
        let dek = kms.resolve_dek(&kr, &region).expect("resolve");
        let (nonce, ct) = dek.seal(b"some encrypted column value");
        let pt = dek.open(&nonce, &ct).expect("authenticated open");
        assert_eq!(pt, b"some encrypted column value");

        // A SECOND resolve yields a key that decrypts the SAME ciphertext (stable DEK material).
        let dek2 = kms.resolve_dek(&kr, &region).expect("resolve again");
        assert_eq!(
            dek2.open(&nonce, &ct).expect("open"),
            b"some encrypted column value"
        );
    }

    #[test]
    fn per_subject_dek_is_distinct_from_the_tenant_dek() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let tk = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("tenant dek");
        let sk = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-1".into()))
            .expect("subject dek");
        assert_ne!(tk, sk, "different key refs");

        // A payload sealed under the tenant DEK does NOT open under the subject DEK (distinct keys
        // — the GD-4 individual-erasure lever depends on this).
        let tdek = kms.resolve_dek(&tk, &region).expect("resolve tenant");
        let sdek = kms.resolve_dek(&sk, &region).expect("resolve subject");
        let (nonce, ct) = tdek.seal(b"bulk");
        assert!(
            sdek.open(&nonce, &ct).is_none(),
            "subject DEK must not open tenant ciphertext"
        );
    }

    // ───────────── crypto-shred: destroy renders DEKs unrecoverable ─────────────

    #[test]
    fn destroy_kek_renders_every_dek_under_it_unrecoverable() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        let kek_id = KekId::new(tenant.clone(), region.clone());
        kms.ensure_kek(&kek_id);
        let tk = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("tenant dek");
        let sk = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-1".into()))
            .expect("subject dek");

        // Both resolve before the shred.
        assert!(kms.resolve_dek(&tk, &region).is_ok());
        assert!(kms.resolve_dek(&sk, &region).is_ok());

        // Crypto-shred the tenant KEK (tenant offboard).
        assert!(kms.destroy_kek(&kek_id), "a KEK was present to destroy");

        // EVERY DEK under it is now unrecoverable — a LOUD KekUnavailable, NEVER a plaintext.
        assert_eq!(
            kms.resolve_dek(&tk, &region),
            Err(KmsError::KekUnavailable(kek_id.clone()))
        );
        assert_eq!(
            kms.resolve_dek(&sk, &region),
            Err(KmsError::KekUnavailable(kek_id))
        );
    }

    #[test]
    fn destroy_subject_dek_leaves_the_tenant_and_other_subjects_intact() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let tk = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("tenant");
        let s1 = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-1".into()))
            .expect("s1");
        let s2 = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-2".into()))
            .expect("s2");

        // Shred ONLY subject u-1 (one person's Art. 17 erasure).
        let s1_id = DekId::new(tenant.clone(), KeyClass::Subject("u-1".into()));
        assert!(kms.destroy_dek(&s1_id), "subject DEK present to destroy");

        // u-1 is gone (loud), the tenant + u-2 are untouched.
        assert_eq!(
            kms.resolve_dek(&s1, &region),
            Err(KmsError::DekUnavailable(s1_id))
        );
        assert!(
            kms.resolve_dek(&tk, &region).is_ok(),
            "tenant DEK untouched"
        );
        assert!(
            kms.resolve_dek(&s2, &region).is_ok(),
            "other subject untouched"
        );
    }

    // ───────────── rotation = envelope re-wrap, not bulk re-encryption ─────────────

    #[test]
    fn rotate_re_wraps_without_re_encrypting_the_payload() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        let kek_id = KekId::new(tenant.clone(), region.clone());
        kms.ensure_kek(&kek_id);
        let kr = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("dek");

        // Seal a payload BEFORE rotation.
        let dek = kms.resolve_dek(&kr, &region).expect("resolve");
        let (nonce, ct) = dek.seal(b"a value sealed before rotation");

        // Rotate the KEK (new epoch). This re-wraps the DEK — it does NOT touch the ciphertext.
        let new_epoch = kms.rotate_kek(&kek_id).expect("rotate");
        assert_eq!(new_epoch, 1, "forward-only epoch bump");

        // The DEK now lives at a new epoch in its ref.
        let kr2 = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("dek post-rotate");
        assert_eq!(kr2.dek_epoch, 1, "the dek epoch bumped on re-wrap");

        // The pre-rotation ciphertext STILL decrypts to the same plaintext — proof the DEK
        // material (and so the payload) was untouched; only the wrapping changed (O(keys)).
        let dek2 = kms.resolve_dek(&kr2, &region).expect("resolve post-rotate");
        assert_eq!(
            dek2.open(&nonce, &ct).expect("still opens"),
            b"a value sealed before rotation",
            "rotation re-wraps the DEK; the payload is NOT re-encrypted"
        );
    }

    // ───────────── the loud-failure / never-fail-open invariants ─────────────

    #[test]
    fn resolve_with_no_kek_fails_loudly_never_plaintext() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        // A DEK ref for a tenant whose KEK was never created → loud KekUnavailable / DekUnavailable,
        // never a fabricated key.
        let kr = PiiKeyRef::new(tenant.clone(), 0, KeyClass::Tenant);
        assert!(matches!(
            kms.resolve_dek(&kr, &region),
            Err(KmsError::DekUnavailable(_))
        ));
    }

    #[test]
    fn ensure_dek_without_a_kek_fails_loudly() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        // No KEK provisioned → ensure_dek cannot wrap → loud KekUnavailable (never a bare DEK).
        let err = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect_err("no kek");
        assert_eq!(err, KmsError::KekUnavailable(KekId::new(tenant, region)));
    }

    #[test]
    fn backup_snapshot_excludes_a_crypto_shredded_tenant() {
        let kms = KmsEngine::new();
        let (live, region) = (t("live-co"), r("eu-west"));
        let (dead, _) = (t("offboarded-co"), r("eu-west"));
        kms.ensure_kek(&KekId::new(live.clone(), region.clone()));
        kms.ensure_kek(&KekId::new(dead.clone(), region.clone()));
        kms.ensure_dek(&live, &region, KeyClass::Tenant)
            .expect("live dek");
        kms.ensure_dek(&dead, &region, KeyClass::Tenant)
            .expect("dead dek");

        // Offboard the dead tenant (crypto-shred its KEK).
        assert!(kms.destroy_kek(&KekId::new(dead.clone(), region.clone())));

        // The backup snapshot carries the LIVE tenant's wrapped DEK but EXCLUDES the shredded one
        // (it must stay dead across a restore — §7.5 / STOR-D3).
        let snap = kms.backup_snapshot();
        assert!(
            snap.iter().any(|(d, _)| d.tenant == live),
            "live tenant DEK is backed up"
        );
        assert!(
            !snap.iter().any(|(d, _)| d.tenant == dead),
            "a crypto-shredded tenant DEK is EXCLUDED from backup (stays dead across restore)"
        );
    }

    #[test]
    fn pii_key_ref_display_equals_the_uri() {
        // kills the Display `fmt → Ok(default)` mutant: Display must render the canonical uri (a
        // blank Display would mis-log every ciphertext's key ref).
        let kr = PiiKeyRef::new(t("acme"), 2, KeyClass::Subject("u-7".into()));
        assert_eq!(format!("{kr}"), "kms://acme/2/subject:u-7");
        assert_eq!(format!("{kr}"), kr.to_uri());
    }

    #[test]
    fn kms_error_display_names_the_loud_failure() {
        // kills the KmsError Display `fmt → Ok(default)` mutant: the error must name the loud
        // failure + the "NOT a plaintext fall-through" posture (a blank error hides the breach).
        let e = KmsError::KekUnavailable(KekId::new(t("acme"), r("eu-west")));
        let m = e.to_string();
        assert!(m.contains("acme") && m.contains("crypto-shred"), "got: {m}");
        let e = KmsError::DekUnavailable(DekId::new(t("acme"), KeyClass::Subject("u".into())));
        assert!(
            e.to_string().contains("unrecoverable"),
            "names the unrecoverable outcome"
        );
        let e = KmsError::UnwrapFailed(DekId::new(t("acme"), KeyClass::Tenant));
        assert!(
            e.to_string().contains("authenticate"),
            "names the auth failure"
        );
    }

    #[test]
    fn raw_key_debug_redacts_the_key_bytes() {
        // kills the RawKey Debug `fmt → Ok(default)` mutant AND proves the redaction: a 256-bit
        // key NEVER enters Debug output (a key in a log is a key compromise). We reach RawKey's
        // Debug via the CellRoot wrapper (which derives Debug over its RawKey field).
        let root = CellRoot::generate();
        let dbg = format!("{root:?}");
        assert!(dbg.contains("redacted"), "RawKey Debug is redacted: {dbg}");
        assert!(
            dbg.contains("CellRoot"),
            "the CellRoot wrapper is named: {dbg}"
        );
    }

    // ───────────── MR-025: the software-sealed durable root-of-trust (DB-free crypto proofs) ─────

    #[test]
    fn seal_unseal_round_trips_the_root_and_never_rests_plaintext() {
        let root = CellRoot::generate();
        let seal = SealKey::from_bytes([3u8; KEY_LEN]);
        let sealed = root.seal(&seal);
        // The sealed bytes are CIPHERTEXT — the root NEVER rests in plaintext at rest.
        assert_ne!(
            sealed.ciphertext.as_slice(),
            root.root.0.as_slice(),
            "the sealed root is ciphertext, never the plaintext root"
        );
        // Unseal under the CORRECT key recovers the exact root (so every KEK it wrapped still unwraps).
        let recovered = CellRoot::unseal(&seal, &sealed).expect("unseal under the correct seal key");
        assert_eq!(
            recovered.root.0, root.root.0,
            "unseal recovers the exact 256-bit root"
        );
    }

    #[test]
    fn unseal_with_a_wrong_seal_key_fails_never_a_silent_root() {
        // The crypto heart of the fail-closed posture: a sealed root does NOT unseal under a wrong
        // seal key — None (AEAD auth failure), NEVER a fabricated/wrong root. The durable
        // load_or_generate turns this None into a LOUD WrongSealKey + refuses to start (proven live).
        let root = CellRoot::generate();
        let sealed = root.seal(&SealKey::from_bytes([1u8; KEY_LEN]));
        assert!(
            CellRoot::unseal(&SealKey::from_bytes([2u8; KEY_LEN]), &sealed).is_none(),
            "a wrong seal key must NOT unseal the root"
        );
    }

    #[test]
    fn a_kek_wrapped_under_the_root_survives_a_seal_unseal_cycle() {
        // The durability invariant in miniature: a KEK wrapped under the root still unwraps under the
        // root recovered from its sealed form — so persisted KEKs survive a restart (the very thing a
        // fresh-root-per-process loses today).
        let root = CellRoot::generate();
        let kek_plain = RawKey::generate();
        let wrapped = root.wrap_kek(&kek_plain);
        let seal = SealKey::from_bytes([9u8; KEY_LEN]);
        let recovered = CellRoot::unseal(&seal, &root.seal(&seal)).expect("unseal");
        let unwrapped = recovered
            .unwrap_kek(&wrapped)
            .expect("the KEK unwraps under the recovered root");
        assert_eq!(
            unwrapped.0, kek_plain.0,
            "the KEK plaintext survived the seal/unseal cycle"
        );
    }

    #[test]
    fn seal_key_from_encoded_decodes_hex_and_rejects_garbage() {
        let hexkey = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        let k = SealKey::from_encoded(hexkey).expect("valid 32-byte hex seal key");
        // It works as a real AEAD key (seal+unseal a root under it).
        let root = CellRoot::generate();
        assert!(CellRoot::unseal(&k, &root.seal(&k)).is_some());
        // Loud, fail-closed decode errors — never a silent zero/truncated key.
        assert!(matches!(
            SealKey::from_encoded("nothex!!"),
            Err(SealKeyError::Decode(_))
        ));
        assert!(matches!(
            SealKey::from_encoded("00112233"),
            Err(SealKeyError::WrongLength(4))
        ));
    }

    #[test]
    fn seal_key_debug_is_redacted() {
        // A seal key in a log is a TOTAL compromise — its Debug must never print the bytes.
        let seal = SealKey::from_bytes([5u8; KEY_LEN]);
        assert_eq!(format!("{seal:?}"), "SealKey(<redacted seal key>)");
        assert!(!format!("{seal:?}").contains('5'));
    }

    #[test]
    fn backup_snapshot_durable_carries_root_keks_deks_and_rebuilds_a_working_engine() {
        // The full reconstruct path, DB-free: snapshot (sealed root + wrapped KEKs + wrapped DEKs) →
        // unseal the root → install the keys → resolve a DEK and decrypt. This is the in-process twin
        // of the live decrypt-across-restart proof.
        let kms = KmsEngine::new();
        let (live, region) = (t("live-co"), r("eu-west"));
        let dead = t("offboarded-co");
        kms.ensure_kek(&KekId::new(live.clone(), region.clone()));
        kms.ensure_kek(&KekId::new(dead.clone(), region.clone()));
        let kr = kms
            .ensure_dek(&live, &region, KeyClass::Tenant)
            .expect("live dek");
        kms.ensure_dek(&dead, &region, KeyClass::Tenant)
            .expect("dead dek");
        // Seal a payload under the live DEK BEFORE snapshotting.
        let (nonce, ct) = kms.resolve_dek(&kr, &region).expect("resolve").seal(b"col");

        // Crypto-shred the offboarded tenant, then snapshot.
        assert!(kms.destroy_kek(&KekId::new(dead.clone(), region.clone())));
        let seal = SealKey::from_bytes([4u8; KEY_LEN]);
        let snap = kms.backup_snapshot_durable(&seal);

        assert!(snap.keks.iter().any(|(id, _)| id.tenant == live));
        assert!(
            !snap.keks.iter().any(|(id, _)| id.tenant == dead),
            "a crypto-shredded KEK is EXCLUDED from the durable snapshot"
        );
        assert!(snap.deks.iter().any(|(id, ..)| id.tenant == live));
        assert!(
            !snap.deks.iter().any(|(id, ..)| id.tenant == dead),
            "a crypto-shredded tenant's DEK is EXCLUDED (stays dead across restore)"
        );

        // Rebuild a FRESH engine from the snapshot (unseal root + install keys) and DECRYPT.
        let engine2 = KmsEngine::from_root(
            CellRoot::unseal(&seal, &snap.sealed_root).expect("unseal the snapshot root"),
        );
        for (id, k) in snap.keks {
            engine2.install_wrapped_kek(id, k.nonce, k.wrapped, k.epoch);
        }
        for (id, w, e) in snap.deks {
            engine2.install_wrapped_dek(id, w, e);
        }
        let pt = engine2
            .resolve_dek(&kr, &region)
            .expect("the live DEK resolves after a from-snapshot rebuild")
            .open(&nonce, &ct)
            .expect("and decrypts the pre-snapshot ciphertext");
        assert_eq!(pt, b"col", "decrypt across a from-snapshot rebuild");
    }

    #[test]
    fn debug_redacts_all_key_material() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let kr = kms
            .ensure_dek(&tenant, &region, KeyClass::Tenant)
            .expect("dek");
        let dek = kms.resolve_dek(&kr, &region).expect("resolve");
        // Neither the engine nor a resolved handle leaks key bytes into Debug.
        assert!(format!("{kms:?}").contains("KmsEngine"));
        assert!(
            !format!("{dek:?}").contains("["),
            "DekHandle redacts its bytes"
        );
        assert!(format!("{dek:?}").contains("redacted"));
    }
}
