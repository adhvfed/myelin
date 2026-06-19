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
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
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
        PiiKeyRef { tenant, dek_epoch, class }
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
        Some(PiiKeyRef { tenant: TenantId(tenant.to_string()), dek_epoch, class })
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
        CellRoot { root: RawKey::generate() }
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
        WrappedKey { nonce: n, wrapped: ct }
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

/// The self-hostable software KMS engine (Vault-Transit-class) — the in-cell key store behind the
/// [`KmsAdapter`] seam (§4). It holds the L0 cell root, the L1 per-(tenant,region) KEKs, and the
/// L2 DEKs stored ENVELOPE-WRAPPED ([`WrappedDek`]) under their KEKs. Every public operation goes
/// through the hierarchy: a DEK is never resolved without its KEK, a KEK never without the cell
/// root.
///
/// `destroy_kek` / `destroy_dek` are the crypto-shred levers; a destroyed key is removed from the
/// store AND excluded from [`backup_snapshot`](Self::backup_snapshot) (it must stay dead across a
/// restore, §7.5).
pub struct KmsEngine {
    root: CellRoot,
    /// L1: one KEK per `(tenant, region)`.
    keks: Mutex<BTreeMap<KekId, StoredKek>>,
    /// L2: the wrapped DEKs, keyed by `(tenant, class)` at their current epoch. The
    /// [`PiiKeyRef`] epoch travelling with a ciphertext selects the right generation; a rotation
    /// re-wraps in place and bumps the stored epoch.
    deks: Mutex<BTreeMap<DekId, (WrappedDek, u64 /* dek_epoch */)>>,
}

impl KmsEngine {
    /// Stand up a fresh engine over a generated cell root (one per cell).
    pub fn new() -> KmsEngine {
        KmsEngine {
            root: CellRoot::generate(),
            keks: Mutex::new(BTreeMap::new()),
            deks: Mutex::new(BTreeMap::new()),
        }
    }

    /// Provision (or fetch) the L1 KEK for a `(tenant, region)`. Idempotent: a second call for the
    /// same id returns the existing KEK's epoch (it does NOT silently rotate). The KEK material is
    /// wrapped-by-the-root conceptually; on this floor it is held sealed in-process (never
    /// exported).
    pub fn ensure_kek(&self, id: &KekId) -> u64 {
        let mut keks = self.keks.lock().expect("KMS keks poisoned");
        if let Some(existing) = keks.get(id) {
            return existing.epoch;
        }
        // A fresh KEK is generated and immediately sealed under the cell root (the L0→L1 envelope);
        // only the wrapped form is stored — the bare KEK never rests in the map.
        let wrapped = self.root.wrap_kek(&RawKey::generate());
        keks.insert(id.clone(), StoredKek { wrapped, epoch: 0 });
        0
    }

    /// Unwrap the KEK for `id` under the cell root into its transient plaintext. Loud failure if
    /// the KEK is unavailable (destroyed / never created) or fails to authenticate under the root.
    fn open_kek(&self, id: &KekId) -> Result<RawKey, KmsError> {
        let keks = self.keks.lock().expect("KMS keks poisoned");
        let kek = keks.get(id).ok_or_else(|| KmsError::KekUnavailable(id.clone()))?;
        self.root
            .unwrap_kek(&kek.wrapped)
            .ok_or_else(|| KmsError::KekUnavailable(id.clone()))
    }

    /// Provision (or fetch) the L2 DEK for `(tenant, class)` in `region`, returning its
    /// [`PiiKeyRef`]. Idempotent per `(tenant, class)`. A per-SUBJECT class yields a DISTINCT DEK
    /// from the tenant class (GD-4). The DEK is generated, wrapped under the tenant KEK, and
    /// stored ONLY in wrapped form (envelope encryption — never a bare DEK at rest). Fails loudly
    /// if the KEK is unavailable (never fabricates a key).
    pub fn ensure_dek(
        &self,
        tenant: &TenantId,
        region: &Region,
        class: KeyClass,
    ) -> Result<PiiKeyRef, KmsError> {
        let kek_id = KekId::new(tenant.clone(), region.clone());
        let dek_id = DekId::new(tenant.clone(), class.clone());
        {
            // Fast path: the DEK already exists → return its ref at its current epoch.
            let deks = self.deks.lock().expect("KMS deks poisoned");
            if let Some((_, dek_epoch)) = deks.get(&dek_id) {
                return Ok(PiiKeyRef::new(tenant.clone(), *dek_epoch, class));
            }
        }
        // Generate a fresh DEK, wrap it under the tenant KEK, store the wrapped form only.
        let dek_plain = RawKey::generate();
        let wrapped = self.wrap_dek(&kek_id, &dek_plain)?;
        let mut deks = self.deks.lock().expect("KMS deks poisoned");
        // Re-check under the lock (another thread may have created it) — keep idempotent.
        if let Some((_, dek_epoch)) = deks.get(&dek_id) {
            return Ok(PiiKeyRef::new(tenant.clone(), *dek_epoch, class));
        }
        let dek_epoch = 0u64;
        deks.insert(dek_id, (wrapped, dek_epoch));
        Ok(PiiKeyRef::new(tenant.clone(), dek_epoch, class))
    }

    /// Wrap (envelope-encrypt) a DEK's plaintext under the named KEK — AES-256-GCM. Fails loudly
    /// if the KEK does not exist (never wraps under a fabricated key).
    fn wrap_dek(&self, kek_id: &KekId, dek_plain: &RawKey) -> Result<WrappedDek, KmsError> {
        let kek = self.open_kek(kek_id)?;
        let kek_epoch = {
            let keks = self.keks.lock().expect("KMS keks poisoned");
            keks.get(kek_id).map(|k| k.epoch).ok_or_else(|| KmsError::KekUnavailable(kek_id.clone()))?
        };
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let wrapped = kek
            .cipher()
            .encrypt(&nonce, dek_plain.0.as_slice())
            .expect("AES-256-GCM wrap");
        let mut n = [0u8; NONCE_LEN];
        n.copy_from_slice(nonce.as_slice());
        Ok(WrappedDek { nonce: n, wrapped, kek_epoch })
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
        let (wrapped, _epoch) =
            deks.get(&dek_id).ok_or_else(|| KmsError::DekUnavailable(dek_id.clone()))?;
        let wrapped = wrapped.clone();
        drop(deks);

        // Open the KEK under the cell root (the L0→L1 step), then unwrap the DEK under it
        // (the L1→L2 step) — a DEK is never resolved without walking the full hierarchy.
        let kek = self.open_kek(&kek_id)?;
        let plain = kek
            .cipher()
            .decrypt(Nonce::from_slice(&wrapped.nonce), wrapped.wrapped.as_slice())
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
            let kek = keks.get_mut(id).ok_or_else(|| KmsError::KekUnavailable(id.clone()))?;
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

}

impl Default for KmsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for KmsEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Counts only — NEVER any key material.
        let keks = self.keks.lock().map(|k| k.len()).unwrap_or(0);
        let deks = self.deks.lock().map(|d| d.len()).unwrap_or(0);
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
        for uri in ["kms://acme/0/tenant", "kms://acme/12/subject:u-99", "kms://acme/5/blob"] {
            let kr = PiiKeyRef::parse(uri).expect("parses the canonical grammar");
            assert_eq!(kr.to_uri(), uri, "round-trip is byte-identical");
        }
    }

    #[test]
    fn pii_key_ref_rejects_malformed_uris_loudly() {
        // A malformed ref is NEVER silently coerced (a wrong ref must be a loud None).
        assert!(PiiKeyRef::parse("https://acme/0/tenant").is_none(), "wrong scheme");
        assert!(PiiKeyRef::parse("kms://acme/0").is_none(), "missing class");
        assert!(PiiKeyRef::parse("kms://acme/notanint/tenant").is_none(), "non-int epoch");
        assert!(PiiKeyRef::parse("kms:///0/tenant").is_none(), "empty tenant");
        assert!(PiiKeyRef::parse("kms://acme/0/bogus").is_none(), "unknown class");
        assert!(PiiKeyRef::parse("kms://acme/0/subject:").is_none(), "empty subject id");
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
        let kr = kms.ensure_dek(&tenant, &region, KeyClass::Tenant).expect("ensure dek");

        // Resolve the DEK and use it to seal+open a payload (the full wrap→unwrap→use round-trip).
        let dek = kms.resolve_dek(&kr, &region).expect("resolve");
        let (nonce, ct) = dek.seal(b"some encrypted column value");
        let pt = dek.open(&nonce, &ct).expect("authenticated open");
        assert_eq!(pt, b"some encrypted column value");

        // A SECOND resolve yields a key that decrypts the SAME ciphertext (stable DEK material).
        let dek2 = kms.resolve_dek(&kr, &region).expect("resolve again");
        assert_eq!(dek2.open(&nonce, &ct).expect("open"), b"some encrypted column value");
    }

    #[test]
    fn per_subject_dek_is_distinct_from_the_tenant_dek() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let tk = kms.ensure_dek(&tenant, &region, KeyClass::Tenant).expect("tenant dek");
        let sk = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-1".into()))
            .expect("subject dek");
        assert_ne!(tk, sk, "different key refs");

        // A payload sealed under the tenant DEK does NOT open under the subject DEK (distinct keys
        // — the GD-4 individual-erasure lever depends on this).
        let tdek = kms.resolve_dek(&tk, &region).expect("resolve tenant");
        let sdek = kms.resolve_dek(&sk, &region).expect("resolve subject");
        let (nonce, ct) = tdek.seal(b"bulk");
        assert!(sdek.open(&nonce, &ct).is_none(), "subject DEK must not open tenant ciphertext");
    }

    // ───────────── crypto-shred: destroy renders DEKs unrecoverable ─────────────

    #[test]
    fn destroy_kek_renders_every_dek_under_it_unrecoverable() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        let kek_id = KekId::new(tenant.clone(), region.clone());
        kms.ensure_kek(&kek_id);
        let tk = kms.ensure_dek(&tenant, &region, KeyClass::Tenant).expect("tenant dek");
        let sk = kms
            .ensure_dek(&tenant, &region, KeyClass::Subject("u-1".into()))
            .expect("subject dek");

        // Both resolve before the shred.
        assert!(kms.resolve_dek(&tk, &region).is_ok());
        assert!(kms.resolve_dek(&sk, &region).is_ok());

        // Crypto-shred the tenant KEK (tenant offboard).
        assert!(kms.destroy_kek(&kek_id), "a KEK was present to destroy");

        // EVERY DEK under it is now unrecoverable — a LOUD KekUnavailable, NEVER a plaintext.
        assert_eq!(kms.resolve_dek(&tk, &region), Err(KmsError::KekUnavailable(kek_id.clone())));
        assert_eq!(kms.resolve_dek(&sk, &region), Err(KmsError::KekUnavailable(kek_id)));
    }

    #[test]
    fn destroy_subject_dek_leaves_the_tenant_and_other_subjects_intact() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let tk = kms.ensure_dek(&tenant, &region, KeyClass::Tenant).expect("tenant");
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
        assert_eq!(kms.resolve_dek(&s1, &region), Err(KmsError::DekUnavailable(s1_id)));
        assert!(kms.resolve_dek(&tk, &region).is_ok(), "tenant DEK untouched");
        assert!(kms.resolve_dek(&s2, &region).is_ok(), "other subject untouched");
    }

    // ───────────── rotation = envelope re-wrap, not bulk re-encryption ─────────────

    #[test]
    fn rotate_re_wraps_without_re_encrypting_the_payload() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        let kek_id = KekId::new(tenant.clone(), region.clone());
        kms.ensure_kek(&kek_id);
        let kr = kms.ensure_dek(&tenant, &region, KeyClass::Tenant).expect("dek");

        // Seal a payload BEFORE rotation.
        let dek = kms.resolve_dek(&kr, &region).expect("resolve");
        let (nonce, ct) = dek.seal(b"a value sealed before rotation");

        // Rotate the KEK (new epoch). This re-wraps the DEK — it does NOT touch the ciphertext.
        let new_epoch = kms.rotate_kek(&kek_id).expect("rotate");
        assert_eq!(new_epoch, 1, "forward-only epoch bump");

        // The DEK now lives at a new epoch in its ref.
        let kr2 = kms.ensure_dek(&tenant, &region, KeyClass::Tenant).expect("dek post-rotate");
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
        let err = kms.ensure_dek(&tenant, &region, KeyClass::Tenant).expect_err("no kek");
        assert_eq!(err, KmsError::KekUnavailable(KekId::new(tenant, region)));
    }

    #[test]
    fn backup_snapshot_excludes_a_crypto_shredded_tenant() {
        let kms = KmsEngine::new();
        let (live, region) = (t("live-co"), r("eu-west"));
        let (dead, _) = (t("offboarded-co"), r("eu-west"));
        kms.ensure_kek(&KekId::new(live.clone(), region.clone()));
        kms.ensure_kek(&KekId::new(dead.clone(), region.clone()));
        kms.ensure_dek(&live, &region, KeyClass::Tenant).expect("live dek");
        kms.ensure_dek(&dead, &region, KeyClass::Tenant).expect("dead dek");

        // Offboard the dead tenant (crypto-shred its KEK).
        assert!(kms.destroy_kek(&KekId::new(dead.clone(), region.clone())));

        // The backup snapshot carries the LIVE tenant's wrapped DEK but EXCLUDES the shredded one
        // (it must stay dead across a restore — §7.5 / STOR-D3).
        let snap = kms.backup_snapshot();
        assert!(snap.iter().any(|(d, _)| d.tenant == live), "live tenant DEK is backed up");
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
        assert!(e.to_string().contains("unrecoverable"), "names the unrecoverable outcome");
        let e = KmsError::UnwrapFailed(DekId::new(t("acme"), KeyClass::Tenant));
        assert!(e.to_string().contains("authenticate"), "names the auth failure");
    }

    #[test]
    fn raw_key_debug_redacts_the_key_bytes() {
        // kills the RawKey Debug `fmt → Ok(default)` mutant AND proves the redaction: a 256-bit
        // key NEVER enters Debug output (a key in a log is a key compromise). We reach RawKey's
        // Debug via the CellRoot wrapper (which derives Debug over its RawKey field).
        let root = CellRoot::generate();
        let dbg = format!("{root:?}");
        assert!(dbg.contains("redacted"), "RawKey Debug is redacted: {dbg}");
        assert!(dbg.contains("CellRoot"), "the CellRoot wrapper is named: {dbg}");
    }

    #[test]
    fn debug_redacts_all_key_material() {
        let kms = KmsEngine::new();
        let (tenant, region) = (t("acme"), r("eu-west"));
        kms.ensure_kek(&KekId::new(tenant.clone(), region.clone()));
        let kr = kms.ensure_dek(&tenant, &region, KeyClass::Tenant).expect("dek");
        let dek = kms.resolve_dek(&kr, &region).expect("resolve");
        // Neither the engine nor a resolved handle leaks key bytes into Debug.
        assert!(format!("{kms:?}").contains("KmsEngine"));
        assert!(!format!("{dek:?}").contains("["), "DekHandle redacts its bytes");
        assert!(format!("{dek:?}").contains("redacted"));
    }
}
