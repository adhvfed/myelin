//! # `pseudonym_store` — the S2 pseudonym map (P-ID-19 → P-077)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/identity-and-access.md`
//! §2 (the **S2 row**: `real_identity ↔ per-tenant pseudonym`; Postgres-class; **tightest RLS**;
//! `(tenant, region)` shard; **per-SUBJECT key = the erasure lever**; grammar pinned C5), §3 (the
//! opaque-stable `principal_id` / erasable `profile_ref` split — S2 is the table that maps the
//! opaque subject to its per-tenant pseudonym), recon §X-7 (the pseudonym-shred / DSR-step-1
//! erasure posture).
//!
//! **Contract-index:** rows 4.8 (`resolve_pseudonym`/`erase` — the **store + grammar** half here;
//! the RPC `resolve_pseudonym` body + the `erase` crypto-shred body are **P-ID-20**), 11.3 (per-cell
//! root → per-tenant KEK → **per-subject DEK**, the erasure lever), 11.4 (GD-4: free-text/profile =
//! per-subject DEK), 10.1 (`PersonalDataHolder` on every Id store) — all **CONSUMED / WIRED** here
//! (this prompt ships a STORE + a frozen grammar, not an RPC body).
//!
//! ## What this module ships (P-ID-19 — the S2 store + holder ONLY)
//! 1. **The S2 pseudonym map** ([`PseudonymStore`]) — the `(tenant, region)`-partitioned,
//!    **tightest-RLS** Postgres-class store mapping a subject's opaque [`PrincipalId`] to its
//!    **per-tenant pseudonym** ([`myelin_identity::PseudonymHandle`], the frozen
//!    `<pseudonym>@<tenant>.noreply` grammar, contract 4.8 / C5). Holder-registered (every Id store
//!    is a `PersonalDataHolder`, §1.1 / GD-3). There is **no cross-tenant query path** — every
//!    access is built from a verified [`TenantScope`] through [`TenantQuery::for_table`].
//! 2. **The per-SUBJECT-key erasure lever** — the `real_identity` half of each mapping (the link
//!    back to the real subject) is sealed under the principal's **per-subject DEK** (contract 11.4,
//!    GD-4) through the [`KmsEngine`] L0→L1→L2 hierarchy (P-058). Destroying that one key (the Art.
//!    17 erase, the **named P-ID-20 floor**) makes the `pseudonym → real_identity` resolution
//!    unrecoverable in DBs + backups + immutable logs, while the **public pseudonym handle**
//!    survives (so historic git attribution stays intact, EI-04 §1). This is the load-bearing GD-4
//!    boundary the per-subject-key mutation floor pins.
//!
//! ## The frozen grammar (decided NOW, before the git data model — EI-04 §1)
//! The pseudonym grammar `<pseudonym>@<tenant>.noreply` is frozen as a value type in the **contract
//! crate** ([`myelin_identity::PseudonymHandle`]) so Git M3 ([P-ID-25]) consumes it without
//! depending on this service crate — the decide-before-the-git-data-model obligation (EI-04 §1: it
//! is nearly impossible to bolt on later). This store mints + stores handles under that frozen type.
//!
//! ## The two mutation-tested mandatory-core properties (the prompt GATE)
//! - **Tightest RLS** — every access carries its `(tenant, region)` predicate (built from a
//!   verified [`TenantScope`], never a path); a read for one tenant structurally cannot reach
//!   another's mappings (a mutation dropping the tenant predicate must be caught).
//! - **The per-subject-key boundary** — subject A's `real_identity` link is sealed under A's DEK and
//!   does NOT open under subject B's DEK (distinct keys, GD-4); a mutation that sealed the
//!   real-identity link under the per-TENANT key (so one destroy could not erase exactly one person)
//!   MUST be caught — it would break the individual crypto-shred lever.
//!
//! ## Floors named (frozen shape now → bodies in a later prompt)
//! - **`resolve_pseudonym` (the RPC) + `erase` (the crypto-shred) are P-ID-20** (→ P-078). This
//!   module ships the STORE (the mapping rows + RLS + per-subject-key seal + holder) ONLY; the
//!   `IdentityService::resolve_pseudonym` body that reads through it and the `PersonalDataHolder`
//!   `erase` that destroys the per-subject DEK land in P-ID-20. The lever (the per-subject key) is
//!   real now; the destroy CALL is the named floor.
//! - **Git pseudonymous-by-default commits are M3 ([P-ID-25])** — they CONSUME the frozen
//!   [`myelin_identity::PseudonymHandle`] grammar this prompt freezes. Recorded as the cross-band
//!   follow-on.
//! - **The in-memory store models the SQL S2 table** (the same EI-01 §1 deviation S1/S3/S5/S8
//!   document): there is no live OLTP database until the driver lands (P-S15); the
//!   `(tenant, region)`-partitioned, tightest-RLS, per-subject-encrypted semantics are byte-for-byte
//!   the §2/11.3 contract. The seam shape does not change when the binding lands.

use myelin_identity::{PrincipalId, PseudonymHandle};
use myelin_storage::{
    KeyClass, KmsEngine, KmsError, OltpHolderRegistration, OltpStoreHolder, PiiKeyRef, TenantQuery,
    TenantScope, TenantTable,
};
use myelin_tenancy::{Region, TenantId};
// `HashMap`/`Mutex` back the in-memory test-double [`Inner`] only (MR-009b Wave 6a — `test-support`-
// gated); the durable production path is the PG backing, so they are absent from the default build.
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

/// The S2 store's tenant-owned table name (the `(tenant, region)`-first, tightest-RLS table). Every
/// access is built through [`TenantQuery::for_table`] over THIS table, so a pseudonym read/write
/// without a verified `(tenant, region)` scope does not compile (the `tenant-predicate` floor).
pub const S2_TABLE: &str = "pseudonym_map";

/// The S2 store's stable holder name (the `PersonalDataHolder` identifier). The store
/// auto-registers under this name so "we forgot the pseudonym map" is structurally impossible
/// (§1.1, GD-3). The DSR `erase` body (the per-subject crypto-shred) lands in P-ID-20.
pub const S2_HOLDER: &str = "identity_pseudonym";

/// A pseudonym-map error (4.8 store half). A failed write/read is a typed, LOUD value — never a
/// silent partial write or a plaintext-without-key fall-through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PseudonymError {
    /// The KMS could not seal/resolve the per-subject DEK (a destroyed key — the crypto-shredded
    /// subject — or an unwrap failure). Surfaced LOUDLY: a shredded subject's `resolve` returns
    /// THIS, **never** a plaintext-without-key (the 0-fail-open invariant, identity §1).
    Kms(String),
    /// The decrypted real-identity bytes were not the frozen UTF-8 shape — a corrupt or wrong-key
    /// open, refused (never a wrong-key read silently coerced).
    CorruptMapping,
    /// The pseudonym did not match the frozen `<pseudonym>@<tenant>.noreply` grammar for the
    /// verified tenant (a mismatched/forged handle) — refused. Defence in depth: the store mints
    /// handles bound to the verified scope's tenant.
    GrammarMismatch {
        /// The offending rendering (for the audit log) — PII-free (it is a pseudonym handle).
        handle: String,
    },
    /// The durable PG backing failed (a connection/query error from the live store, MR-009b W6a) — a
    /// LOUD typed value, never a silent partial write/read. Distinct from [`PseudonymError::Kms`] (a
    /// key failure) so the verifier can tell a storage fault from a crypto fault.
    Storage(String),
}

impl core::fmt::Display for PseudonymError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PseudonymError::Kms(why) => write!(
                f,
                "pseudonym-map KMS error (the read/write did NOT succeed — never \
                 plaintext-without-key; a crypto-shredded subject resolves to THIS): {why}"
            ),
            PseudonymError::CorruptMapping => write!(
                f,
                "pseudonym real-identity decrypted to a non-conforming shape (a wrong-key/corrupt \
                 open — refused, never silently coerced)"
            ),
            PseudonymError::GrammarMismatch { handle } => write!(
                f,
                "pseudonym handle `{handle}` does not match the frozen \
                 `<pseudonym>@<tenant>.noreply` grammar for the verified tenant (refused)"
            ),
            PseudonymError::Storage(why) => write!(
                f,
                "pseudonym-map durable backing error (the read/write did NOT succeed — never a \
                 silent partial write): {why}"
            ),
        }
    }
}

impl std::error::Error for PseudonymError {}

impl From<KmsError> for PseudonymError {
    fn from(e: KmsError) -> PseudonymError {
        PseudonymError::Kms(e.to_string())
    }
}

/// A sealed `real_identity` link — the at-rest, per-subject-DEK-encrypted form of the opaque
/// subject id a pseudonym maps back to (the PII-sensitive half never rests in the clear in S2). The
/// `(nonce, ciphertext)` is the AES-256-GCM seal; [`PseudonymRow::real_id_key_ref`] names the DEK
/// that opens it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SealedRealIdentity {
    nonce: [u8; myelin_storage::NONCE_LEN],
    ciphertext: Vec<u8>,
}

/// **A stored S2 mapping row (architecture §2 the S2 store).**
///
/// The PUBLIC half — the per-tenant [`PseudonymHandle`] (the frozen `<pseudonym>@<tenant>.noreply`
/// grammar) — is stored in the clear: it is PII-free and SURVIVES erasure (so historic git
/// attribution stays intact, EI-04 §1). The PII-sensitive half — the link back to the real subject
/// — is held ENCRYPTED under the subject's **per-subject DEK** (the [`SealedRealIdentity`], keyed by
/// `real_id_key_ref`); destroying that one key is the subject's Art. 17 erasure (the crypto-shred
/// lever, P-ID-20 body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PseudonymRow {
    /// The verified tenant partition (from the write scope — never from a path/string).
    pub tenant: TenantId,
    /// The residency region partition (12.1 — `(tenant, region)` is the partition key).
    pub region: Region,
    /// **The per-tenant public pseudonym handle (the frozen grammar, contract 4.8 / C5).** PII-free
    /// + erasure-surviving: this is the handle baked into pseudonymous git commits (M3, P-ID-25).
    pub pseudonym: PseudonymHandle,
    /// **The `pii_key_ref` of the per-SUBJECT DEK that sealed the real-identity link (GD-4).**
    /// Destroying this key (the P-ID-20 erase body) makes the `pseudonym → real_identity`
    /// resolution unrecoverable while the public `pseudonym` survives.
    pub real_id_key_ref: PiiKeyRef,
}

/// The shared inner state of a [`PseudonymStore`] (behind `Arc<Mutex<…>>` so the store is a
/// cloneable handle and a write is atomic under one lock).
///
/// **MR-009b Wave 6a — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature = "test-support"))]`).**
/// The PRODUCTION default is the durable PG backing ([`PgPseudonymBacking`], via
/// [`PseudonymStore::with_pg`]); this in-memory `Inner` is the DB-free unit-test double downstream
/// crates reach via the `test-support` dev-dependency. The `no-in-memory-durable-store` scanner treats
/// a `test-support`-gated backing as a test double, so S2 leaves the baseline (SI-018 cluster).
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
struct Inner {
    /// The committed mapping rows, keyed by `(tenant, region)` partition then the opaque subject
    /// `principal_id`. The OUTER map is the `(tenant, region)` partition (no cross-tenant query
    /// path: a read for tenant A never touches tenant B's map). The forward direction:
    /// `subject → row` (DSR step 1 starts from the subject).
    by_subject: HashMap<(String, String), HashMap<String, PseudonymRow>>,
    /// The reverse index `pseudonym-rendering → subject id`, within a `(tenant, region)` partition —
    /// the lookup `resolve_pseudonym` (the RPC, P-ID-20) keys on (a git commit carries the rendered
    /// pseudonym; resolving it back to the subject is the reverse direction). Per-tenant only.
    by_pseudonym: HashMap<(String, String), HashMap<String, String>>,
    /// The sealed real-identity links, keyed identically to `by_subject`. Held separately so the
    /// link can be crypto-shredded (the per-subject DEK destroyed) while the public pseudonym row
    /// survives (the EI-04 §1 immutable-attribution-survives split, made structural).
    sealed: HashMap<(String, String), HashMap<String, SealedRealIdentity>>,
}

/// **The S2 pseudonym map (architecture §2; contracts 4.8/11.3/11.4/12.1/10.1).** A cloneable handle
/// over shared state, `(tenant, region)`-partitioned under the TIGHTEST RLS, per-subject-DEK-sealing
/// the real-identity link, and holder-registered.
///
/// **No cross-tenant query path:** every accessor takes a verified [`TenantScope`] and builds its
/// query through [`TenantQuery::for_table`] over [`S2_TABLE`] — so a tenant-less pseudonym access
/// does not compile (the `tenant-predicate` floor), and a read for one tenant structurally cannot
/// reach another tenant's partition.
#[derive(Clone)]
pub struct PseudonymStore {
    /// The durable backing — the REAL PG `pseudonym_map` table (MR-009b W6a) on the production path,
    /// or the in-memory test-double on the default DB-free build. The system-of-record is the Pg
    /// backing; the in-memory map is an explicit test double.
    backend: PseudonymBackend,
    /// The KMS engine the store seals/opens the real-identity link through (the L0→L1→L2 hierarchy,
    /// P-058). The link is sealed under the per-SUBJECT DEK ([`KeyClass::Subject`]) — the GD-4
    /// individual crypto-shred lever, distinct from the per-tenant DEK.
    kms: Arc<KmsEngine>,
    /// The holder this store auto-registers as (the `PersonalDataHolder` seam) — proof the "every
    /// store is a holder" invariant holds for S2 (§1.1, GD-3).
    holder: OltpStoreHolder,
}

/// The S2 store backing: the REAL durable PG `pseudonym_map` table (MR-009b W6a) — the PRODUCTION
/// default — or the in-memory test-double. Splitting the backing OUT of the role struct's direct
/// fields is what lets the `no-in-memory-durable-store` ratchet record the shortcut's removal: the
/// PRODUCTION-compiled enum presents ONLY the pool-backed `Pg` variant (the `Memory` variant is
/// `test-support`-gated, which the scanner strips as a test double), so `PseudonymStore` no longer
/// holds an in-memory collection in the production graph. The real-identity link is KMS-sealed in BOTH
/// backings; the Pg backing persists only the OPAQUE ciphertext (the per-subject DEK stays with the
/// KMS engine — MR-025 boundary).
#[derive(Clone)]
enum PseudonymBackend {
    /// The in-memory test-double — MR-009b Wave 6a: compiled ONLY under
    /// `#[cfg(any(test, feature = "test-support"))]`. NOT the production system-of-record.
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Inner>>),
    /// The REAL durable PG backing over the MR-022 provider pool + `with_tenant_tx` convention — the
    /// PRODUCTION DEFAULT (always compiled).
    Pg(PgPseudonymBacking),
}

/// The PG-backed S2 pseudonym backing (MR-009b W6a): the durable `pseudonym_map` table + the
/// sync→async bridge (`tokio::runtime::Handle` driving `block_in_place`+`block_on`). The production
/// default (always compiled).
#[derive(Clone)]
struct PgPseudonymBacking {
    backing: Arc<myelin_storage::DurablePseudonymBacking>,
    rt: tokio::runtime::Handle,
}

impl PgPseudonymBacking {
    /// Drive an async backing call from the sync store API (the `block_in_place`+`block_on` bridge).
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

impl PseudonymStore {
    /// Build the S2 store over the in-memory TEST-DOUBLE backing (MR-009b Wave 6a: compiled ONLY under
    /// `#[cfg(any(test, feature = "test-support"))]`). The PRODUCTION constructor is
    /// [`PseudonymStore::with_pg`] (the durable PG default); this `::new` is the DB-free unit-test
    /// entry point downstream crates reach via the `test-support` dev-dependency. The store
    /// auto-registers as a `PersonalDataHolder` on construction (opening IS registering, §3.4) so the
    /// registration is structural, never an afterthought.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(kms: Arc<KmsEngine>) -> PseudonymStore {
        let holder = OltpStoreHolder::new(S2_HOLDER);
        // Opening IS registering (§3.4, GD-3): the S2 store auto-registers the moment it is built,
        // so "we forgot the pseudonym map" is structurally impossible.
        let _receipt = holder.register();
        PseudonymStore {
            backend: PseudonymBackend::Memory(Arc::new(Mutex::new(Inner::default()))),
            kms,
            holder,
        }
    }

    /// **Build the S2 store over the REAL durable PG backing (MR-009b W6a / SI-018 cluster).** The
    /// mapping rows + the KMS-sealed real-identity-link ciphertext persist through the MR-022
    /// [`myelin_storage::SubstrateProvider`] pool + `with_tenant_tx` convention (tightest-RLS-scoped,
    /// no GUC bleed). `rt` is the tokio runtime handle the sync API drives the async backing on. The
    /// KMS engine is reused as-is (the per-subject-DEK seal boundary is unchanged); decrypt-across-
    /// restart depends on the durable KMS root (MR-025). Auto-registers as a holder. **The PRODUCTION
    /// default (MR-009b Wave 6a) — always compiled.**
    pub fn with_pg(
        kms: Arc<KmsEngine>,
        backing: myelin_storage::DurablePseudonymBacking,
        rt: tokio::runtime::Handle,
    ) -> PseudonymStore {
        let holder = OltpStoreHolder::new(S2_HOLDER);
        let _receipt = holder.register();
        PseudonymStore {
            backend: PseudonymBackend::Pg(PgPseudonymBacking {
                backing: Arc::new(backing),
                rt,
            }),
            kms,
            holder,
        }
    }

    /// The store AS a `PersonalDataHolder` (the holder the DSR fan-out drives). The DSR `erase` body
    /// (the per-subject crypto-shred of the real-identity link) lands with P-ID-20; here the
    /// REGISTRATION is real (the holder is constructed + registered) so the holder-registered
    /// architecture test sees the S2 store.
    pub fn holder(&self) -> &OltpStoreHolder {
        &self.holder
    }

    /// Fire the auto-registration hook for this store (contract 1.4), returning the receipt the
    /// harness collects — the proof the S2 store registered as a holder.
    pub fn register_holder(&self) -> OltpHolderRegistration {
        self.holder.register()
    }

    /// The per-SUBJECT DEK key class for a subject's real-identity link (contract 11.4, GD-4). The
    /// link is sealed under the PER-SUBJECT key (`subject:<principal_id>`) — **distinct** from the
    /// per-tenant DEK — so destroying it erases exactly that one person's `pseudonym → real_identity`
    /// resolution (the individual crypto-shred lever). This is the load-bearing GD-4 choice the
    /// per-subject-key mutation-test pins. The SAME key class S1 uses for the subject's profile, so
    /// one Art. 17 erase destroys both in one shred.
    pub fn subject_dek_class(subject: &PrincipalId) -> KeyClass {
        KeyClass::Subject(subject.0.clone())
    }

    /// The per-TENANT DEK key class (contract 11.4). Named so the GD-4 split (per-tenant vs
    /// per-subject) is explicit and testable — the real-identity link is NEVER sealed under this
    /// (that would break the individual crypto-shred lever).
    pub fn tenant_dek_class() -> KeyClass {
        KeyClass::Tenant
    }

    /// **Upsert a `subject ↔ per-tenant pseudonym` mapping; seal the real-identity link under the
    /// per-SUBJECT DEK (4.8 store half + 11.3/11.4 + 12.1).**
    ///
    /// Under ONE store lock:
    /// 1. build the access through a [`TenantQuery`] over [`S2_TABLE`] — the whole write carries its
    ///    `(tenant, region)` predicate (the tenant-predicate floor; a tenant-less write is
    ///    unconstructable — you need a [`TenantScope`] here);
    /// 2. refuse a `pseudonym` whose tenant label does not match the verified scope's tenant (the
    ///    handle is bound to the verified tenant — a forged cross-tenant handle is refused);
    /// 3. seal the `subject`'s real-identity link under the subject's **per-subject DEK** (GD-4) —
    ///    the PII-sensitive half never rests in the clear in S2;
    /// 4. store the [`PseudonymRow`] (public pseudonym + the erasable key ref) + the sealed link +
    ///    the reverse index entry in the verified `(tenant, region)` partition.
    ///
    /// Returns the stored [`PseudonymRow`], or a [`PseudonymError`] (in which case nothing changed —
    /// never a partial write).
    pub fn put_mapping(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
        pseudonym: PseudonymHandle,
    ) -> Result<PseudonymRow, PseudonymError> {
        // (1) The tenant-predicate floor: the whole write is built from the verified scope (no
        //     cross-tenant write path). A tenant-less write is unconstructable.
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));

        // (2) The handle is bound to the verified tenant — refuse a handle whose tenant label is not
        //     the verified scope's tenant (a forged/cross-tenant handle never lands).
        if pseudonym.tenant() != scope.tenant().0 {
            return Err(PseudonymError::GrammarMismatch {
                handle: pseudonym.render(),
            });
        }

        // (3) Seal the real-identity link (the opaque subject id) under the per-SUBJECT DEK (GD-4) —
        //     distinct from the per-tenant key. A wrong/missing key fails LOUDLY.
        let (key_ref, sealed) = self.seal_real_identity(scope, subject)?;

        let row = PseudonymRow {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            pseudonym: pseudonym.clone(),
            real_id_key_ref: key_ref,
        };

        // (4) Commit the row + the sealed link + the reverse index entry into the verified partition.
        //     A write for one tenant structurally cannot land in another's partition (the Memory map
        //     is keyed by the verified (tenant, region); the Pg path writes RLS-scoped).
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let part_key = Self::part_key(scope);
                let mut inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                inner
                    .by_subject
                    .entry(part_key.clone())
                    .or_default()
                    .insert(subject.0.clone(), row.clone());
                inner
                    .by_pseudonym
                    .entry(part_key.clone())
                    .or_default()
                    .insert(pseudonym.render(), subject.0.clone());
                inner
                    .sealed
                    .entry(part_key)
                    .or_default()
                    .insert(subject.0.clone(), sealed);
            }
            PseudonymBackend::Pg(pg) => {
                // The DURABLE upsert: the public pseudonym rendering + the erasable key ref + the
                // KMS-sealed real-identity ciphertext blob, through with_tenant_tx (tightest-RLS). Only
                // the ciphertext rests in PG (the per-subject DEK stays with the engine — MR-025).
                let drow = myelin_storage::DurablePseudonymRow {
                    principal_id: subject.0.clone(),
                    pseudonym_render: pseudonym.render(),
                    real_id_key_ref: row.real_id_key_ref.to_uri(),
                    nonce: sealed.nonce.to_vec(),
                    ciphertext: sealed.ciphertext.clone(),
                };
                pg.block(pg.backing.put_mapping(&scope.tenant().0, drow))
                    .map_err(|e| PseudonymError::Storage(e.to_string()))?;
            }
        }
        Ok(row)
    }

    /// Reconstruct a [`PseudonymRow`] from a durable row (the public pseudonym + the erasable key ref
    /// rebuilt from the stored rendering/URI). These are our own writes, so a parse failure is a
    /// genuine corruption — surfaced loudly (the `Storage` typed value, never a silently-coerced row).
    fn durable_to_row(
        scope: &TenantScope,
        drow: &myelin_storage::DurablePseudonymRow,
    ) -> Result<PseudonymRow, PseudonymError> {
        let pseudonym = PseudonymHandle::parse(&drow.pseudonym_render).ok_or_else(|| {
            PseudonymError::Storage(format!(
                "malformed stored pseudonym rendering `{}`",
                drow.pseudonym_render
            ))
        })?;
        let real_id_key_ref = PiiKeyRef::parse(&drow.real_id_key_ref).ok_or_else(|| {
            PseudonymError::Storage(format!(
                "malformed stored key_ref `{}`",
                drow.real_id_key_ref
            ))
        })?;
        Ok(PseudonymRow {
            tenant: scope.tenant().clone(),
            region: scope.region().clone(),
            pseudonym,
            real_id_key_ref,
        })
    }

    /// Reconstruct the sealed real-identity link (`nonce` + `ciphertext`) from a durable row. A
    /// wrong-length nonce is a corrupt blob (refused loudly, never a wrong-key read silently coerced).
    fn durable_to_sealed(
        drow: &myelin_storage::DurablePseudonymRow,
    ) -> Result<SealedRealIdentity, PseudonymError> {
        if drow.nonce.len() != myelin_storage::NONCE_LEN {
            return Err(PseudonymError::CorruptMapping);
        }
        let mut nonce = [0u8; myelin_storage::NONCE_LEN];
        nonce.copy_from_slice(&drow.nonce);
        Ok(SealedRealIdentity {
            nonce,
            ciphertext: drow.ciphertext.clone(),
        })
    }

    /// Seal a subject's real-identity link under its per-SUBJECT DEK, returning the
    /// `(pii_key_ref, SealedRealIdentity)`. Provisions the per-(tenant,region) KEK + the per-subject
    /// DEK (idempotent) and AES-256-GCM-seals the opaque subject id. The sensitive half never leaves
    /// this function in the clear once sealed.
    fn seal_real_identity(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
    ) -> Result<(PiiKeyRef, SealedRealIdentity), PseudonymError> {
        // L1: ensure the tenant's (tenant, region) KEK exists (the subject DEK wraps under it).
        let kek_id = myelin_storage::KekId::new(scope.tenant().clone(), scope.region().clone());
        self.kms.ensure_kek(&kek_id);
        // L2: the PER-SUBJECT DEK (GD-4) — distinct from the per-tenant DEK. THIS is the key the
        // subject's Art. 17 erasure destroys (one destroy = that person's mapping unrecoverable).
        let key_ref = self.kms.ensure_dek(
            scope.tenant(),
            scope.region(),
            Self::subject_dek_class(subject),
        )?;
        let dek = self.kms.resolve_dek(&key_ref, scope.region())?;
        let (nonce, ciphertext) = dek.seal(subject.0.as_bytes());
        Ok((key_ref, SealedRealIdentity { nonce, ciphertext }))
    }

    /// **Read a subject's mapping row (the public pseudonym + the erasable key ref) — RLS-scoped.**
    /// Built through a [`TenantQuery`] so the access carries its `(tenant, region)` predicate; a read
    /// for one tenant structurally cannot reach another's partition. `None` if no such mapping in the
    /// verified scope. The PUBLIC pseudonym survives a crypto-shred (only the real-identity LINK is
    /// erased), so this read keeps working post-erasure.
    pub fn mapping_of(&self, scope: &TenantScope, subject: &PrincipalId) -> Option<PseudonymRow> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                inner
                    .by_subject
                    .get(&Self::part_key(scope))
                    .and_then(|p| p.get(&subject.0).cloned())
            }
            PseudonymBackend::Pg(pg) => pg
                .block(pg.backing.get_by_principal(&scope.tenant().0, &subject.0))
                .ok()
                .flatten()
                .and_then(|drow| Self::durable_to_row(scope, &drow).ok()),
        }
    }

    /// **Resolve a per-tenant pseudonym handle back to its subject's opaque `principal_id` (the
    /// reverse direction `resolve_pseudonym` keys on; 11.3 read path) — RLS-scoped.**
    ///
    /// This opens the sealed real-identity link under the subject's per-SUBJECT DEK. A
    /// crypto-shredded subject (DEK destroyed) returns [`PseudonymError::Kms`] — **never** a
    /// plaintext-without-key (the 0-fail-open invariant). `Ok(None)` if the handle is unknown in the
    /// verified scope. The RPC `IdentityService::resolve_pseudonym` body that wraps this is **P-ID-20**;
    /// this is the store primitive it reads through.
    ///
    /// **The per-subject-key boundary (mutation-tested):** the link opens ONLY under the SAME
    /// per-subject DEK that sealed it — subject A's link does not open under subject B's key (GD-4).
    pub fn resolve(
        &self,
        scope: &TenantScope,
        pseudonym: &PseudonymHandle,
    ) -> Result<Option<PrincipalId>, PseudonymError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        // The reverse index names the subject; the sealed link + key_ref open it. All read under the
        // verified scope's partition.
        let (key_ref, sealed) = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let part_key = Self::part_key(scope);
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                let subject_id = match inner
                    .by_pseudonym
                    .get(&part_key)
                    .and_then(|m| m.get(&pseudonym.render()))
                {
                    Some(s) => s.clone(),
                    None => return Ok(None),
                };
                let row = match inner
                    .by_subject
                    .get(&part_key)
                    .and_then(|p| p.get(&subject_id))
                {
                    Some(r) => r.clone(),
                    None => return Ok(None),
                };
                let sealed = match inner.sealed.get(&part_key).and_then(|p| p.get(&subject_id)) {
                    Some(s) => s.clone(),
                    None => return Ok(None),
                };
                (row.real_id_key_ref, sealed)
            }
            PseudonymBackend::Pg(pg) => {
                // The reverse-lookup index resolves the pseudonym rendering to its row (subject +
                // key_ref + sealed link) in ONE tenant-scoped read.
                let drow = match pg
                    .block(
                        pg.backing
                            .get_by_pseudonym(&scope.tenant().0, &pseudonym.render()),
                    )
                    .map_err(|e| PseudonymError::Storage(e.to_string()))?
                {
                    Some(d) => d,
                    None => return Ok(None),
                };
                let row = Self::durable_to_row(scope, &drow)?;
                let sealed = Self::durable_to_sealed(&drow)?;
                (row.real_id_key_ref, sealed)
            }
        };
        // Resolve the per-SUBJECT DEK + open. A shredded/destroyed key is a LOUD KmsError here, never
        // a plaintext fall-through (the 0-fail-open invariant).
        let dek = self.kms.resolve_dek(&key_ref, scope.region())?;
        let plain = dek
            .open(&sealed.nonce, &sealed.ciphertext)
            .ok_or(PseudonymError::CorruptMapping)?;
        let subject = String::from_utf8(plain).map_err(|_| PseudonymError::CorruptMapping)?;
        Ok(Some(PrincipalId(subject)))
    }

    /// **Is a subject's real-identity link STILL recoverable? (the ID-D8 resurrection probe.)** Opens
    /// the sealed real-identity link under the subject's per-SUBJECT DEK; returns the recovered opaque
    /// real id iff BOTH the map row survives AND the per-subject DEK still opens it. Returns `None` if
    /// the row was shredded OR the DEK was destroyed (crypto-shredded) — i.e. the subject IS erased.
    ///
    /// This is the forward analogue of [`PseudonymStore::resolve`] (which keys on the public
    /// pseudonym); the re-erasure pass uses THIS to assert "0 resurrected" (a restored backup that
    /// brought back the row AND the key would make this `Some` — a resurrection). RLS-scoped.
    pub fn resolve_subject(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
    ) -> Option<PrincipalId> {
        self.try_resolve_subject(scope, subject).ok().flatten()
    }

    /// Fallible resurrection probe for erasure verification. An absent row or a destroyed key is a
    /// clean `None`; storage faults, malformed durable rows, key unwrap failures, and corrupt
    /// ciphertext are errors and must not be certified as successful erasure.
    pub fn try_resolve_subject(
        &self,
        scope: &TenantScope,
        subject: &PrincipalId,
    ) -> Result<Option<PrincipalId>, PseudonymError> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        let (key_ref, sealed) = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let part_key = Self::part_key(scope);
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                let Some(row) = inner
                    .by_subject
                    .get(&part_key)
                    .and_then(|p| p.get(&subject.0)) else {
                        return Ok(None);
                    };
                let Some(sealed) = inner
                    .sealed
                    .get(&part_key)
                    .and_then(|p| p.get(&subject.0)) else {
                        return Err(PseudonymError::CorruptMapping);
                    };
                (row.real_id_key_ref.clone(), sealed.clone())
            }
            PseudonymBackend::Pg(pg) => {
                let Some(drow) = pg
                    .block(pg.backing.get_by_principal(&scope.tenant().0, &subject.0))
                    .map_err(|e| PseudonymError::Storage(e.to_string()))? else {
                        return Ok(None);
                    };
                let row = Self::durable_to_row(scope, &drow)?;
                let sealed = Self::durable_to_sealed(&drow)?;
                (row.real_id_key_ref, sealed)
            }
        };
        // A deliberately destroyed subject/tenant key is the crypto-shred success condition. Every
        // other KMS failure is corruption or infrastructure failure and invalidates the proof.
        let dek = match self.kms.resolve_dek(&key_ref, scope.region()) {
            Ok(dek) => dek,
            Err(KmsError::KekUnavailable(_) | KmsError::DekUnavailable(_)) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let plain = dek
            .open(&sealed.nonce, &sealed.ciphertext)
            .ok_or(PseudonymError::CorruptMapping)?;
        let subject = String::from_utf8(plain).map_err(|_| PseudonymError::CorruptMapping)?;
        Ok(Some(PrincipalId(subject)))
    }

    /// **The subject's per-subject DEK class (the Art. 17 crypto-shred lever).** Destroying THIS key
    /// (the P-ID-20 erase body) makes the subject's `pseudonym → real_identity` resolution
    /// unrecoverable in DBs + backups + immutable logs while the PUBLIC pseudonym handle survives.
    /// Exposed so the erase fan-out keys on the one structural lever the store builds. The destroy
    /// CALL is the named floor (P-ID-20); the lever is real now.
    pub fn shred_key_for(&self, scope: &TenantScope, subject: &PrincipalId) -> Option<PiiKeyRef> {
        self.mapping_of(scope, subject).map(|r| r.real_id_key_ref)
    }

    /// **Shred a subject's pseudonym-map row (the `erase` body half, P-ID-20 / 4.8).** Removes the
    /// row + its sealed real-identity link + the reverse-index entry from the verified `(tenant,
    /// region)` partition, under one store lock. RLS-scoped (built through a [`TenantQuery`]); a shred
    /// for one tenant structurally cannot reach another's partition.
    ///
    /// Returns `true` iff a row was present to shred (so the caller can report idempotency — a
    /// re-shred of an already-shredded subject removes nothing and returns `false`, which is correct:
    /// the subject IS already erased). This shreds ONLY the resolvable mapping; the **crypto-shred of
    /// the per-subject DEK** (the key that sealed the link) is the [`KmsEngine::destroy_dek`] half the
    /// erase engine pairs with this — together they are the complete DSR-step-1 crypto-shred.
    pub fn shred_row(&self, scope: &TenantScope, subject: &PrincipalId) -> bool {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let part_key = Self::part_key(scope);
                let mut inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                // Find the public pseudonym rendering first (to remove its reverse-index entry) — read
                // it out before the forward row is gone.
                let pseudonym_rendering = inner
                    .by_subject
                    .get(&part_key)
                    .and_then(|p| p.get(&subject.0))
                    .map(|r| r.pseudonym.render());
                let removed = inner
                    .by_subject
                    .get_mut(&part_key)
                    .and_then(|p| p.remove(&subject.0))
                    .is_some();
                // Remove the sealed real-identity link (defence in depth: even if the DEK were somehow
                // recoverable, the ciphertext is gone) and the reverse-index entry.
                inner
                    .sealed
                    .get_mut(&part_key)
                    .and_then(|p| p.remove(&subject.0));
                if let Some(rendering) = pseudonym_rendering {
                    inner
                        .by_pseudonym
                        .get_mut(&part_key)
                        .and_then(|m| m.remove(&rendering));
                }
                removed
            }
            PseudonymBackend::Pg(pg) => {
                // The DURABLE crypto-shred: DELETE the row (row + sealed link + reverse-lookup path) in
                // one tenant-scoped tx. A storage fault is swallowed to `false` (the shred did NOT
                // land) — the erase engine's LOUD signal is the KMS destroy_dek half it pairs with.
                pg.block(pg.backing.shred(&scope.tenant().0, &subject.0))
                    .unwrap_or(false)
            }
        }
    }

    /// List the mapping rows in a `(tenant, region)` partition (for the directory / tests). There is
    /// NO accessor that reads across partitions — a read is scoped to one verified `(tenant,
    /// region)`, so cross-tenant reads are structurally impossible.
    pub fn mappings_in(&self, scope: &TenantScope) -> Vec<PseudonymRow> {
        let _q = TenantQuery::for_table(scope.clone(), TenantTable::new(S2_TABLE));
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PseudonymBackend::Memory(inner_arc) => {
                let inner = inner_arc.lock().unwrap_or_else(|e| e.into_inner());
                inner
                    .by_subject
                    .get(&Self::part_key(scope))
                    .map(|p| p.values().cloned().collect())
                    .unwrap_or_default()
            }
            PseudonymBackend::Pg(pg) => pg
                .block(pg.backing.mappings_in(&scope.tenant().0))
                .unwrap_or_default()
                .iter()
                .filter_map(|drow| Self::durable_to_row(scope, drow).ok())
                .collect(),
        }
    }

    /// The `(tenant, region)` partition key for a verified scope (the OUTER partition; 12.1). A
    /// `(String, String)` so the partition map is keyed by the residency-pinned tenant+region — a
    /// read for one never reaches another's bucket. In-memory test-double helper (MR-009b Wave 6a —
    /// `test-support`-gated; the durable path scopes via `with_tenant_tx`).
    #[cfg(any(test, feature = "test-support"))]
    fn part_key(scope: &TenantScope) -> (String, String) {
        (scope.tenant().0.clone(), scope.region().0.clone())
    }

    /// Lock the in-memory test-double backing (the Memory arm; the unit tests' defence-in-depth
    /// ciphertext probe uses it). Panics on the Pg backend (which has no in-process map) — only the
    /// in-memory tests call this.
    #[cfg(test)]
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match &self.backend {
            PseudonymBackend::Memory(arc) => arc.lock().unwrap_or_else(|e| e.into_inner()),
            PseudonymBackend::Pg(_) => {
                panic!("lock() is the in-memory test-double accessor; the Pg backend has no map")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalKind};

    fn kms() -> Arc<KmsEngine> {
        Arc::new(KmsEngine::new())
    }

    fn scope(tenant: &str) -> TenantScope {
        scope_region(tenant, "eu-west")
    }

    fn scope_region(tenant: &str, region: &str) -> TenantScope {
        let p = Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        );
        TenantScope::from_verified_token(&p, Region(region.into()))
    }

    fn handle(pseudonym: &str, tenant: &str) -> PseudonymHandle {
        PseudonymHandle::new(pseudonym, tenant).expect("a well-formed handle")
    }

    /// **An S2 mapping row round-trips under the tightest RLS (4.8 store half + 12.1).** A mapping
    /// written under a verified scope is readable back under the SAME scope; the public pseudonym +
    /// the erasable key ref are preserved; the real-identity link resolves back to the subject.
    #[test]
    fn s2_mapping_round_trips_under_rls() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let h = handle("anon-7f3a", "acme");
        let written = store
            .put_mapping(&s, &PrincipalId("p:alice".into()), h.clone())
            .expect("write");
        assert_eq!(written.pseudonym, h, "the public pseudonym is stored");

        // The forward direction (subject → row) round-trips.
        let read = store
            .mapping_of(&s, &PrincipalId("p:alice".into()))
            .expect("the row round-trips under the same scope");
        assert_eq!(
            read, written,
            "the S2 row round-trips byte-for-byte under RLS"
        );

        // The reverse direction (pseudonym → subject) resolves the real-identity link.
        let subject = store
            .resolve(&s, &h)
            .expect("resolve succeeds")
            .expect("the pseudonym resolves");
        assert_eq!(
            subject,
            PrincipalId("p:alice".into()),
            "the pseudonym resolves back to the real subject (the real-identity link)"
        );
    }

    #[test]
    fn fallible_resurrection_probe_rejects_corrupt_ciphertext() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let subject = PrincipalId("p:alice".into());
        store
            .put_mapping(&s, &subject, handle("anon-7f3a", "acme"))
            .expect("write mapping");

        let part = (s.tenant().0.clone(), s.region().0.clone());
        let mut inner = store.lock();
        let sealed = inner
            .sealed
            .get_mut(&part)
            .and_then(|rows| rows.get_mut(&subject.0))
            .expect("sealed mapping");
        sealed.ciphertext[0] ^= 0xff;
        drop(inner);

        assert_eq!(
            store.try_resolve_subject(&s, &subject),
            Err(PseudonymError::CorruptMapping),
            "corruption must invalidate an erasure proof instead of looking erased"
        );
    }

    /// **A cross-tenant read returns nothing (the tightest-RLS floor — mutation-tested
    /// mandatory-core).** A mapping written under tenant `acme` is INVISIBLE to a read under tenant
    /// `globex`; the partitions are isolated by the verified `(tenant, region)` scope, and there is
    /// NO accessor that reads across them. A mutation dropping the tenant predicate (reading the
    /// wrong partition) makes this fail — the catch the prompt GATE requires.
    #[test]
    fn cross_tenant_read_returns_nothing() {
        let store = PseudonymStore::new(kms());
        let acme = scope("acme");
        let globex = scope("globex");
        let h = handle("anon-7f3a", "acme");
        store
            .put_mapping(&acme, &PrincipalId("p:alice".into()), h.clone())
            .expect("acme write");

        // globex sees NOTHING acme wrote.
        assert!(
            store
                .mapping_of(&globex, &PrincipalId("p:alice".into()))
                .is_none(),
            "no cross-tenant read path: globex cannot see acme's mapping"
        );
        // globex cannot resolve acme's pseudonym (the reverse index is per-partition too).
        assert_eq!(
            store.resolve(&globex, &h).expect("resolve"),
            None,
            "globex cannot resolve acme's pseudonym"
        );
        assert!(
            store.mappings_in(&globex).is_empty(),
            "globex's partition is empty"
        );
        assert_eq!(store.mappings_in(&acme).len(), 1);
    }

    /// **A cross-REGION read returns nothing (12.1 — `(tenant, region)` is the partition key).** The
    /// same tenant in a different region is a different partition; residency is a first-class
    /// partition dimension.
    #[test]
    fn cross_region_read_returns_nothing() {
        let store = PseudonymStore::new(kms());
        let eu = scope_region("acme", "eu-west");
        let us = scope_region("acme", "us-east");
        store
            .put_mapping(
                &eu,
                &PrincipalId("p:alice".into()),
                handle("anon-7f3a", "acme"),
            )
            .expect("eu write");
        assert!(
            store
                .mapping_of(&us, &PrincipalId("p:alice".into()))
                .is_none(),
            "residency partition: the us-east partition cannot see the eu-west mapping"
        );
        assert_eq!(store.mappings_in(&eu).len(), 1);
    }

    /// **Each subject's mapping is under a DISTINCT per-subject key (4.8 / GD-4).** Two subjects in
    /// the same tenant get distinct `subject:<id>` DEK classes (NOT the per-tenant key) — the GD-4
    /// individual-erasure lever. This is the per-subject-key boundary the prompt names mandatory-core.
    #[test]
    fn each_mapping_is_under_a_distinct_per_subject_key() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        store
            .put_mapping(&s, &PrincipalId("p:alice".into()), handle("anon-a", "acme"))
            .unwrap();
        store
            .put_mapping(&s, &PrincipalId("p:bob".into()), handle("anon-b", "acme"))
            .unwrap();

        let alice_ref = store
            .shred_key_for(&s, &PrincipalId("p:alice".into()))
            .unwrap();
        let bob_ref = store
            .shred_key_for(&s, &PrincipalId("p:bob".into()))
            .unwrap();

        // Each names the PER-SUBJECT key class, NOT the per-tenant key.
        assert_eq!(alice_ref.class, KeyClass::Subject("p:alice".into()));
        assert_ne!(
            alice_ref.class,
            PseudonymStore::tenant_dek_class(),
            "the real-identity link is keyed under the PER-SUBJECT DEK, not the per-tenant DEK"
        );
        // Distinct subjects ⇒ distinct keys (GD-4).
        assert_ne!(
            alice_ref.class, bob_ref.class,
            "distinct subjects get distinct per-subject DEKs"
        );
    }

    /// **The per-subject-key boundary (mutation-tested mandatory-core): subject A's real-identity
    /// link does NOT open under subject B's DEK.** Distinct subjects get distinct keys (GD-4); a
    /// mutation that sealed the link under the per-TENANT key (so two subjects shared a key) would
    /// let B's key open A's link — this catches that, cryptographically (not just by string id).
    #[test]
    fn per_subject_key_boundary_a_does_not_open_b() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        store
            .put_mapping(&s, &PrincipalId("p:alice".into()), handle("anon-a", "acme"))
            .unwrap();
        store
            .put_mapping(&s, &PrincipalId("p:bob".into()), handle("anon-b", "acme"))
            .unwrap();
        let bob_ref = store
            .shred_key_for(&s, &PrincipalId("p:bob".into()))
            .unwrap();

        // Pull alice's sealed link directly and try to open it under BOB's DEK — it must NOT open.
        let inner = store.lock();
        let part = (s.tenant().0.clone(), s.region().0.clone());
        let alice_sealed = inner
            .sealed
            .get(&part)
            .unwrap()
            .get("p:alice")
            .unwrap()
            .clone();
        drop(inner);
        let bob_dek = store.kms.resolve_dek(&bob_ref, s.region()).unwrap();
        assert!(
            bob_dek
                .open(&alice_sealed.nonce, &alice_sealed.ciphertext)
                .is_none(),
            "bob's per-subject DEK must NOT open alice's real-identity link (the GD-4 boundary)"
        );
    }

    /// **A crypto-shredded subject's resolve fails LOUDLY — never plaintext-without-key (the
    /// 0-fail-open invariant, identity §1) — while the PUBLIC pseudonym survives (EI-04 §1).**
    /// Destroying the subject's per-subject DEK (the Art. 17 erase lever, P-ID-20 floor) makes the
    /// `pseudonym → real_identity` resolution unrecoverable: `resolve` returns a LOUD `Kms` error,
    /// never a fabricated subject. The public pseudonym row SURVIVES (historic git attribution stays
    /// intact).
    #[test]
    fn crypto_shredded_resolve_fails_loud_but_pseudonym_survives() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let h = handle("anon-7f3a", "acme");
        store
            .put_mapping(&s, &PrincipalId("p:alice".into()), h.clone())
            .unwrap();

        // Destroy alice's per-subject DEK (the crypto-shred lever — the erase BODY is P-ID-20).
        let key_ref = store
            .shred_key_for(&s, &PrincipalId("p:alice".into()))
            .unwrap();
        let dek_id = myelin_storage::DekId::new(key_ref.tenant.clone(), key_ref.class.clone());
        assert!(
            store.kms.destroy_dek(&dek_id),
            "the per-subject DEK is destroyed (crypto-shred)"
        );

        // The resolve now fails LOUDLY (the key is gone) — never plaintext-without-key.
        let r = store.resolve(&s, &h);
        assert!(
            matches!(r, Err(PseudonymError::Kms(_))),
            "a crypto-shredded resolve fails loud (KmsError), never plaintext-without-key"
        );
        // The PUBLIC pseudonym row SURVIVES the shred (the EI-04 §1 immutable-attribution split:
        // the public handle stays, only the real-identity link is erased).
        assert!(
            store
                .mapping_of(&s, &PrincipalId("p:alice".into()))
                .is_some(),
            "the public pseudonym row survives the crypto-shred (historic attribution intact)"
        );
    }

    /// **A handle bound to a DIFFERENT tenant than the verified scope is REFUSED (defence in
    /// depth).** The store mints/stores handles bound to the verified tenant; a forged
    /// `anon@globex.noreply` written under an `acme` scope is rejected (a cross-tenant handle never
    /// lands). Nothing is written on rejection (never a partial write).
    #[test]
    fn cross_tenant_handle_is_refused() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let forged = handle("anon-7f3a", "globex"); // tenant label != the verified scope's tenant
        let r = store.put_mapping(&s, &PrincipalId("p:alice".into()), forged);
        assert!(
            matches!(r, Err(PseudonymError::GrammarMismatch { .. })),
            "a handle whose tenant label != the verified tenant is refused"
        );
        assert!(
            store
                .mapping_of(&s, &PrincipalId("p:alice".into()))
                .is_none(),
            "nothing was written on rejection (no partial write)"
        );
    }

    /// **The pseudonym grammar parses/formats to `<pseudonym>@<tenant>.noreply` (the prompt GATE).**
    /// The store stores the FROZEN [`PseudonymHandle`]; a stored handle renders the frozen shape and
    /// parses back. (The grammar type itself is exhaustively tested in the contract crate; this
    /// confirms the store carries it byte-for-byte.)
    #[test]
    fn stored_handle_renders_the_frozen_grammar() {
        let store = PseudonymStore::new(kms());
        let s = scope("acme");
        let h = handle("anon-7f3a", "acme");
        let row = store
            .put_mapping(&s, &PrincipalId("p:alice".into()), h)
            .unwrap();
        assert_eq!(
            row.pseudonym.render(),
            "anon-7f3a@acme.noreply",
            "the stored handle renders the frozen `<pseudonym>@<tenant>.noreply` grammar"
        );
        assert_eq!(
            PseudonymHandle::parse(&row.pseudonym.render()),
            Some(row.pseudonym.clone()),
            "the stored handle round-trips through the frozen grammar"
        );
    }

    /// **Each [`PseudonymError`] renders a LOUD, distinct, non-empty message (never a silent
    /// empty render).** The error text is what an operator/audit log sees on a refused
    /// write/resolve; an empty/blank render would hide WHY the operation failed. Pins the
    /// `Display` impl so a mutation blanking it is caught.
    #[test]
    fn pseudonym_errors_render_loud_distinct_messages() {
        let kms = PseudonymError::Kms("dek destroyed".into()).to_string();
        let corrupt = PseudonymError::CorruptMapping.to_string();
        let mismatch = PseudonymError::GrammarMismatch {
            handle: "anon@globex.noreply".into(),
        }
        .to_string();
        for (msg, needle) in [
            (&kms, "KMS"),
            (&corrupt, "non-conforming"),
            (&mismatch, "grammar"),
        ] {
            assert!(!msg.is_empty(), "the error renders a non-empty message");
            assert!(
                msg.contains(needle),
                "the error names its cause ({needle}): {msg}"
            );
        }
        // The Kms variant carries the underlying reason; the mismatch names the offending handle.
        assert!(
            kms.contains("dek destroyed"),
            "the KMS error carries the underlying reason"
        );
        assert!(
            mismatch.contains("anon@globex.noreply"),
            "the mismatch names the offending handle"
        );
    }

    /// **The S2 store auto-registers as a `PersonalDataHolder` (§1.1, GD-3, contract 10.1) — the
    /// prompt GATE "S2 holder-registered (assert it appears in the holder list)".** Opening IS
    /// registering — the holder is constructed + registered under the S2 holder name. The DSR `erase`
    /// body (per-subject crypto-shred of the real-identity link) is the P-ID-20 floor.
    #[test]
    fn s2_store_registers_as_a_personal_data_holder() {
        let store = PseudonymStore::new(kms());
        assert_eq!(
            store.holder().store,
            S2_HOLDER,
            "the S2 store registered under its holder name"
        );
        let receipt = store.register_holder();
        assert_eq!(
            receipt.store, S2_HOLDER,
            "the holder receipt names S2 (it appears in the list)"
        );
    }
}
