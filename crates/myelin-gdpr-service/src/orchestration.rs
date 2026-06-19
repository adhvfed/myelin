//! # The upstream-store holder ORCHESTRATION (H6/H8/H9/H10/H14/H15) + the canonical erase
//! order + resumable receipts (P-GA-06 → P-106)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §3.1 (the
//! **no-cross-store-read law** — the orchestrator NEVER reaches into a store, it calls the
//! holder contract), §3.2 (the holder list — **H6** object/blob store, **H8** event-bus
//! history, **H9** caches/CDN, **H10** backups/snapshots, **H14** authz tuples incl. the
//! reverse index, **H15** Identity incl. the pseudonym map; each holder's erasure-mechanism
//! column), and **§4.1** (the **canonical erase order**:
//! `Id.erase (pseudonym map first) → KMS.destroy per-subject DEK → Search.purge+reindex →
//! Refs.tombstone → Bus.erase → notif/authz/agent-memory → record receipt`). The
//! policy↔mechanism boundary is `external-insights/04-hard-problems.md` §1: **delete the
//! identity, not the fact** — Identity owns the pseudonym-map shred LEVER, Storage owns the
//! crypto-shred MECHANISM. Prove-it: `external-insights/01-process-and-quality-doctrine.md`
//! §4 (chain mutations end-to-end — test the chained fan-out ORDERING, not a single holder).
//!
//! **Contract-index:** row **10.1** — the M1-shared-layer holder ORCHESTRATION + the canonical
//! erase order + the resumable receipt (OWNED here). Consumed: 4.8 (Identity pseudonym-map
//! shred + erase — H14/H15), 11.3/11.4 (the crypto-shred mechanism — H6/H10), 2.2/2.7 (the bus
//! erase + `*.erased` tombstones — H8).
//!
//! ## What this prompt ships vs what P-105 shipped
//! P-105 ([`crate::holders`]) shipped the trait BODIES + the **GDPR-OWNED** holders (H18 + H16).
//! P-106 (this module) ships the **ORCHESTRATION over the UPSTREAM shared-layer stores** — the
//! stores own their `erase` impls (in their own crates: Identity, Storage, Bus, cache), GDPR
//! REGISTERS them as holders and CALLS them in the canonical order. Because the orchestrator
//! must NEVER import a store (the no-cross-store-read law + the downward-DAG-edge ban), an
//! upstream holder is reached through a **[`PersonalDataHolder`] SEAM** the harness/orchestrator
//! wires with the real store impl at boot — exactly the [`crate::holders::CryptoShredKms`]
//! pattern. The architecture test (in `holders.rs`) already asserts this crate carries no
//! `myelin_storage` import; this module extends the law to the upstream holders structurally
//! (it holds only `&dyn PersonalDataHolder`, never a concrete store type).
//!
//! ## The canonical erase order (the load-bearing correctness property — §4.1)
//! The order is NOT cosmetic. **Identity (H15) erases the pseudonym map FIRST** so that every
//! downstream holder, when its `erase` runs, sees **only the opaque pseudonym** — never the
//! real identity (the delete-the-identity-not-the-fact split, EI-04 §1). The §4.1 sequence,
//! projected onto the H6/H8/H9/H10/H14/H15 subset this prompt owns:
//!
//! 1. **H15 Identity** (`Id.erase`, **pseudonym map FIRST**) — the erasure LEVER.
//! 2. **H6 Blob** (`KMS.destroy` per-subject/-tenant DEK) — the crypto-shred MECHANISM.
//! 3. **H14 Authz tuples** (delete the subject's tuples + the reverse index; "authz" in §4.1).
//! 4. **H8 Bus** (`Bus.erase` — crypto-shred inline-PII keys + `*.erased` tombstones).
//! 5. **H9 Cache** (TTL expiry + targeted purge — the derived copies).
//! 6. **H10 Backup** (crypto-shred BY CONSTRUCTION — the key destroyed in steps 1–4 already
//!    renders the backup ciphertext unrecoverable; this step records the post-restore
//!    re-erasure cursor so a restore resurrects nothing — §7, ADR-18). Backup is LAST because
//!    its erasure is a CONSEQUENCE of the upstream key destruction, recorded for independent
//!    checkability against the KMS key-destruction log.
//!
//! Each holder call is **idempotent + resumable**: the **durable per-holder checklist IS the
//! state** ([`EraseChecklist`]); a crashed orchestrator re-drives ONLY un-receipted holders, and
//! a re-driven holder erase is a no-op returning the SAME content-addressed receipt (the holder
//! bodies guarantee this; P-105). Every call returns a receipt recording the **destroyed key
//! epoch + the purge cursor** (so erasure is independently checkable against the KMS log).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **These M1-shared-layer holders orchestrate here.** The full erasure PROOF over them
//!   (post-erase `locate` = 0 incl. backups, cell-scale) is **P-GA-09 → and the M5 GA-D1 gate
//!   P-GA-32 → P-505**. Here the floor is the M1-holder orchestration floor: the fan-out hits
//!   100% of the *existing* (registered) holders in the canonical order, each returning a
//!   resumable receipt.
//! - **The producer holders H1/H4/H17** are **M3 P-GA-27 → P-256**; **the consumer holders
//!   H2/H3/H5** are **M4 P-GA-29/P-GA-30**. They register through this same seam as their stores
//!   ship — the orchestrator's canonical-order sequence already has slots for them
//!   ([`CanonicalErasePhase`] is the abstract phase, not a fixed six-holder list).
//! - **The live store-`erase` bindings** behind the [`PersonalDataHolder`] seam are wired by the
//!   harness/orchestrator at boot (the real Identity / Storage / Bus / cache `erase` impls). On
//!   THIS floor each upstream holder is modelled by a [`SeamHolder`] test double whose `erase`
//!   crypto-shreds through the same [`crate::holders::CryptoShredKms`] seam the GDPR-owned
//!   holders use — so the ORDERING + idempotency + resumability are proven against a faithful
//!   model, and the live binding is a config swap, never a code change.
//! - **The durable Postgres checklist table** (the G1 `dsr_request` per-holder checklist rows)
//!   is the same DB floor every M0 in-memory store carries (P-007 / P-S12). On this floor the
//!   checklist is an in-memory [`EraseChecklist`] with byte-for-byte the resumability semantics.

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{EraseReceipt, EraseScope, PersonalDataHolder, SubjectRef, TenantId};

// ───────────────────────── the upstream shared-layer holder ids (§3.2) ─────────────────────────

/// The stable, PII-free holder names the M1-shared-layer upstream stores register under
/// (contract 1.4 — the data-map / DSR fan-out address book). One per §3.2 holder this prompt
/// orchestrates (H6/H8/H9/H10/H14/H15). PII-free: a holder id is a store name, never a subject.
pub mod holder_ids {
    /// **H15** — Identity (Principal/Auth DB + the pseudonym map; the erasure LEVER — §3.2).
    pub const IDENTITY: &str = "identity";
    /// **H6** — the object/blob store (crypto-shred per-tenant/-subject DEK — §3.2).
    pub const BLOB: &str = "blob_store";
    /// **H14** — the authz tuples incl. the reverse index (delete the subject's tuples — §3.2).
    pub const AUTHZ_TUPLES: &str = "authz_tuples";
    /// **H8** — the event-bus history (crypto-shred inline-PII keys + `*.erased` tombstones — §3.2).
    pub const BUS: &str = "event_bus";
    /// **H9** — the caches / CDN (TTL expiry + targeted purge — §3.2).
    pub const CACHE: &str = "cache_cdn";
    /// **H10** — the backups / snapshots (crypto-shred by construction + post-restore re-erasure — §3.2).
    pub const BACKUP: &str = "backups";
}

/// The phase a holder occupies in the **canonical erase order** (§4.1). The order is a property
/// of the PHASE, not a fixed holder list — so a producer/consumer holder (M3/M4) registering
/// later slots into the correct phase by its [`CanonicalErasePhase`], never by re-deriving a
/// hand-written sequence (which is exactly the "we forgot a holder" trap this prompt forecloses).
///
/// The numeric discriminant IS the sort key (lower = earlier). **Identity is phase 0** (the
/// pseudonym map FIRST — every downstream holder then sees only the opaque pseudonym).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum CanonicalErasePhase {
    /// Phase 0 — **Identity (H15)**: erase the pseudonym map FIRST (§4.1 — `Id.erase` first).
    /// The erasure lever; after this every downstream holder sees only the opaque pseudonym.
    IdentityPseudonymMap = 0,
    /// Phase 1 — **crypto-shred the per-subject/-tenant DEK** (§4.1 — `KMS.destroy` next).
    /// H6 (blob) lives here; the producer/consumer free-text DEK holders (H1/H3/H4/H5) join
    /// this phase as they ship.
    CryptoShredDek = 1,
    /// Phase 2 — **purge + reindex / tombstone the derived stores** (§4.1 — Search/Refs/authz).
    /// H14 (authz tuples + reverse index) lives here; Search (H7) / Refs (H12) join as they ship.
    PurgeAndTombstoneDerived = 2,
    /// Phase 3 — **bus erase + `*.erased` tombstones** (§4.1 — `Bus.erase`). H8 lives here.
    BusErase = 3,
    /// Phase 4 — **cache TTL/purge + notif/agent-memory** (§4.1 — the trailing derived copies).
    /// H9 (cache) lives here; notif (H13) / agent-memory (H11) join as they ship.
    CachesAndDerivedCopies = 4,
    /// Phase 5 — **backups** (§4.1 implicit — crypto-shred BY CONSTRUCTION). The key destroyed
    /// in phases 0–1 already renders the backup ciphertext unrecoverable; this phase records the
    /// post-restore re-erasure cursor so a restore resurrects nothing (§7, ADR-18). LAST because
    /// its erasure is a CONSEQUENCE of the upstream key destruction.
    Backups = 5,
}

/// The default canonical-order phase for each of the six upstream holders this prompt owns
/// (§3.2 → §4.1). Returns `None` for an unknown holder id (a producer/consumer holder that has
/// not yet declared its phase — it must do so when it registers, in its own prompt).
pub fn canonical_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        holder_ids::IDENTITY => Some(CanonicalErasePhase::IdentityPseudonymMap),
        holder_ids::BLOB => Some(CanonicalErasePhase::CryptoShredDek),
        holder_ids::AUTHZ_TUPLES => Some(CanonicalErasePhase::PurgeAndTombstoneDerived),
        holder_ids::BUS => Some(CanonicalErasePhase::BusErase),
        holder_ids::CACHE => Some(CanonicalErasePhase::CachesAndDerivedCopies),
        holder_ids::BACKUP => Some(CanonicalErasePhase::Backups),
        _ => None,
    }
}

// ───────────────────────── the upstream-holder registration seam ─────────────────────────

/// One **registered upstream holder**: its PII-free id, its canonical-order phase, and the
/// [`PersonalDataHolder`] seam through which the orchestrator calls it (the store owns the impl;
/// the orchestrator only holds `&dyn PersonalDataHolder` — never a concrete store type, so the
/// no-cross-store-read law holds structurally). The harness wires the real store impl here at
/// boot; the drill wires a [`SeamHolder`] test double.
pub struct RegisteredHolder<'a> {
    /// The stable, PII-free holder id (one of [`holder_ids`]).
    pub id: &'static str,
    /// The phase this holder occupies in the canonical erase order (§4.1).
    pub phase: CanonicalErasePhase,
    /// The holder contract — the ONLY way the orchestrator touches the store (§3.1).
    pub holder: &'a dyn PersonalDataHolder,
}

/// The receipt one holder returned in a fan-out, recorded into the durable checklist. PII-free:
/// the holder id + the content-addressed [`EraseReceipt`] (which itself carries only opaque ids,
/// never PII — P-105 [`myelin_gdpr::Receipt::content_addressed`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderReceipt {
    /// The holder this receipt is for (the checklist key).
    pub holder_id: &'static str,
    /// The phase the holder occupied (the order proof — receipts are collected in phase order).
    pub phase: CanonicalErasePhase,
    /// The content-addressed erase receipt the holder returned (records the destroyed key epoch).
    pub receipt: EraseReceipt,
}

/// **The durable per-holder erase checklist — the RESUMABILITY state (§4.1 step 4).** A crashed
/// orchestrator re-drives ONLY un-receipted holders: the checklist records, per holder id, the
/// receipt it already returned; [`EraseChecklist::is_done`] gates re-driving. On the live floor
/// this is the G1 `dsr_request` per-holder checklist Postgres rows (named floor); here it is an
/// in-memory map with byte-for-byte the resumability semantics.
///
/// The checklist is the SINGLE SOURCE OF TRUTH for "what's left" — the orchestrator never asks a
/// holder "are you done?", it reads the checklist. This is why a holder erase MUST be idempotent
/// (the holder bodies guarantee it, P-105): if the orchestrator crashes AFTER a holder shredded
/// but BEFORE the receipt was persisted, the re-drive re-calls the holder, which no-ops and
/// returns the SAME receipt — so the checklist converges either way.
#[derive(Debug, Default)]
pub struct EraseChecklist {
    /// holder id → the receipt it returned (present ⇒ done; absent ⇒ must be (re-)driven).
    done: Mutex<BTreeMap<&'static str, HolderReceipt>>,
}

impl EraseChecklist {
    /// A fresh checklist (nothing done yet — every registered holder must be driven).
    pub fn new() -> EraseChecklist {
        EraseChecklist {
            done: Mutex::new(BTreeMap::new()),
        }
    }

    /// Whether this holder has already returned a receipt (so a re-drive SKIPS the call —
    /// resumability). Reads the durable checklist, never the holder.
    pub fn is_done(&self, holder_id: &str) -> bool {
        self.done.lock().unwrap_or_else(|e| e.into_inner()).contains_key(holder_id)
    }

    /// Record a holder's receipt into the durable checklist (the holder is now done).
    fn record(&self, hr: HolderReceipt) {
        self.done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(hr.holder_id, hr);
    }

    /// The receipts collected so far, **in canonical erase-order** (phase ascending, then holder
    /// id). The DSR completion certificate (P-GA-12) seals these into the audit Merkle tree
    /// (P-GA-20); the order proof reads this.
    pub fn receipts_in_order(&self) -> Vec<HolderReceipt> {
        let mut v: Vec<HolderReceipt> = self
            .done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| a.phase.cmp(&b.phase).then(a.holder_id.cmp(b.holder_id)));
        v
    }

    /// How many holders are recorded done (the `erasure_fanout_coverage` numerator).
    pub fn done_count(&self) -> usize {
        self.done.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

// ───────────────────────── the telemetry contract (§4.1 GATE) ─────────────────────────

/// The `erasure_fanout_coverage` telemetry signal NAME + UNIT (gdpr §4.1 / contract 1.8 — the
/// DSR fan-out SLO). Over the M1 holder set it reads **100% of *existing* (registered) holders**
/// (the floor gate); over the WHOLE data map "0 holders missed" is the M5 gate (P-GA-32). The
/// signal NAME + UNIT are pinned here so a later emitter uses exactly this string + unit.
pub const ERASURE_FANOUT_COVERAGE: (&str, &str) = ("gdpr.erasure_fanout_coverage", "ratio");

/// The `crypto_shred_lag` telemetry signal NAME + UNIT (gdpr §4.2 — the lag on the key
/// destruction the receipt records, against which "we erased it" is independently checkable).
/// On this floor the lag is 0 (the holder shred is synchronous in the drill); the live
/// measurement lands when the KMS key-destruction log is wired (named floor, P-GA-09/P-GA-15).
pub const CRYPTO_SHRED_LAG: (&str, &str) = ("gdpr.crypto_shred_lag", "ms");

// ───────────────────────── the orchestrator ─────────────────────────

/// **The upstream-store holder ORCHESTRATOR (contract 10.1).** Holds the registered upstream
/// holders (H6/H8/H9/H10/H14/H15 on the M1 floor; producer/consumer holders join as they ship),
/// fans an erase out to them **in the canonical erase order** (§4.1), and records each receipt
/// into the durable [`EraseChecklist`] (resumability). It NEVER reaches into a store — it holds
/// only `&dyn PersonalDataHolder` and calls the contract (the no-cross-store-read law, §3.1).
pub struct UpstreamHolderOrchestrator<'a> {
    holders: Vec<RegisteredHolder<'a>>,
}

impl<'a> UpstreamHolderOrchestrator<'a> {
    /// Build an orchestrator over a set of registered upstream holders. The holders are stored
    /// **sorted into canonical erase-order** (phase ascending, then holder id) at construction —
    /// so the fan-out cannot accidentally call them out of order (the order is structural, not a
    /// caller responsibility).
    pub fn new(mut holders: Vec<RegisteredHolder<'a>>) -> UpstreamHolderOrchestrator<'a> {
        holders.sort_by(|a, b| a.phase.cmp(&b.phase).then(a.id.cmp(b.id)));
        UpstreamHolderOrchestrator { holders }
    }

    /// Register the **default M1-shared-layer upstream holder set** (H15/H6/H14/H8/H9/H10) over
    /// the given holder seams, each at its [`canonical_phase_of`] phase. The caller passes the
    /// store impl (the real `erase` at boot; a [`SeamHolder`] in the drill) for each id; an id
    /// without a known canonical phase is rejected (it must declare its phase in its own prompt).
    pub fn register_m1_upstream(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> UpstreamHolderOrchestrator<'a> {
        let registered = holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = canonical_phase_of(id)
                    .unwrap_or_else(|| panic!("holder `{id}` has no canonical erase phase (§4.1)"));
                RegisteredHolder { id, phase, holder }
            })
            .collect();
        UpstreamHolderOrchestrator::new(registered)
    }

    /// The registered holder ids, **in canonical erase-order** (the fan-out sequence). The
    /// `erasure_fanout_coverage` denominator (the *existing* holder set).
    pub fn holder_ids_in_order(&self) -> Vec<&'static str> {
        self.holders.iter().map(|h| h.id).collect()
    }

    /// How many holders are registered (the coverage denominator).
    pub fn registered_count(&self) -> usize {
        self.holders.len()
    }

    /// **Fan an erase out to every registered upstream holder IN THE CANONICAL ERASE ORDER
    /// (§4.1), idempotently + resumably.** For each holder, in phase order:
    /// - if the durable [`EraseChecklist`] already has its receipt (a re-drive after a crash),
    ///   **SKIP** the call (resumability — only un-receipted holders are re-driven);
    /// - else call the holder's `erase` through the contract, and record the receipt.
    ///
    /// Identity (H15) runs FIRST (its phase is 0), so every downstream holder sees only the
    /// opaque pseudonym (§4.1). Returns the ordered receipts (the order proof + the DSR
    /// completion certificate input, P-GA-12). The call is **idempotent**: re-driving a fully
    /// erased subject re-affirms the SAME receipts (the holder bodies no-op + return the same
    /// content-addressed receipt; P-105).
    ///
    /// **Errors fail the whole fan-out** (a partial erase is recorded in the checklist, so a
    /// retry resumes from the failed holder — the un-receipted ones are re-driven, the receipted
    /// ones are skipped). The first holder error is returned; the orchestrator does NOT continue
    /// past a failed holder (a downstream holder might rely on the upstream pseudonym shred).
    pub fn fan_out_erase(
        &self,
        scope: &EraseScope,
        checklist: &EraseChecklist,
    ) -> myelin_gdpr::Result<Vec<HolderReceipt>> {
        for rh in &self.holders {
            // Resumability: a holder already receipted is SKIPPED on a re-drive.
            if checklist.is_done(rh.id) {
                continue;
            }
            // The ONLY way the orchestrator touches the store: the holder contract (§3.1).
            let receipt = rh.holder.erase(scope.clone())?;
            checklist.record(HolderReceipt {
                holder_id: rh.id,
                phase: rh.phase,
                receipt,
            });
        }
        Ok(checklist.receipts_in_order())
    }

    /// The `erasure_fanout_coverage` reading (§4.1 GATE): the FRACTION of *existing* (registered)
    /// holders that returned a receipt into the checklist. On a complete fan-out over the M1
    /// holder set this reads **1.0 (100%)** — the floor gate. Over the WHOLE data map "0 holders
    /// missed" is the M5 gate (P-GA-32). Returns 1.0 for an empty holder set (vacuously complete).
    pub fn fanout_coverage(&self, checklist: &EraseChecklist) -> f64 {
        if self.holders.is_empty() {
            return 1.0;
        }
        checklist.done_count() as f64 / self.holders.len() as f64
    }
}

// ───────────────────────── a faithful upstream-holder test double (the named floor) ─────────────────────────

/// A faithful **upstream-holder test double** modelling a shared-layer store's `erase` impl on
/// the M1 floor (the live store binding is the named floor). Its `erase` crypto-shreds through
/// the SAME [`crate::holders::CryptoShredKms`] seam the GDPR-owned holders use — so the
/// orchestration's ORDERING + idempotency + resumability are proven against a faithful model of
/// the §3.2 erasure mechanisms, and the live binding is a config swap, never a code change.
///
/// A [`SeamHolder`] erases a single named [`crate::holders::ShredKeyClass`] for the subject (its
/// store's key class). Non-shred holders (a tombstone-only or purge-only store) are modelled by
/// [`ShredKeyClass`] = a sentinel the KMS double has no key for — the erase still returns a
/// receipt (a no-op shred), exactly as a derived-store tombstone returns a receipt without a key
/// epoch. Idempotent: a re-driven erase no-ops (the key is already gone) and returns the SAME
/// content-addressed receipt.
pub struct SeamHolder<'a> {
    id: &'static str,
    /// The key class this holder's store shreds (its own class — never another store's).
    key_class: crate::holders::ShredKeyClass,
    kms: &'a dyn crate::holders::CryptoShredKms,
    /// A counter of how many times `erase` was CALLED (vs short-circuited by the checklist).
    /// Lets the resumability drill assert "a re-drive did NOT re-call an already-done holder".
    erase_calls: Mutex<u32>,
}

impl<'a> SeamHolder<'a> {
    /// Build a faithful upstream-holder double for `id`, shredding `key_class` through `kms`.
    pub fn new(
        id: &'static str,
        key_class: crate::holders::ShredKeyClass,
        kms: &'a dyn crate::holders::CryptoShredKms,
    ) -> SeamHolder<'a> {
        SeamHolder {
            id,
            key_class,
            kms,
            erase_calls: Mutex::new(0),
        }
    }

    /// The PII-free holder id.
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// How many times `erase` was actually CALLED on this holder (the resumability witness — a
    /// re-drive of an already-receipted holder must NOT increment this).
    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl PersonalDataHolder for SeamHolder<'_> {
    fn locate(
        &self,
        subject: &SubjectRef,
        tenant: TenantId,
    ) -> myelin_gdpr::Result<myelin_gdpr::LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let handle = crate::holders::ShredKeyHandle {
            tenant: tenant.clone(),
            class: self.key_class.clone(),
        };
        let outcome = if self.kms.is_present(&handle) {
            "located:present"
        } else {
            "located:0-recoverable"
        };
        Ok(myelin_gdpr::LocateReport {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "locate", self.id, &sid, &tenant.0, outcome, None, 0,
            ),
        })
    }

    fn export(
        &self,
        subject: &SubjectRef,
        tenant: TenantId,
    ) -> myelin_gdpr::Result<myelin_gdpr::PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(myelin_gdpr::PortableBundle {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "export", self.id, &sid, &tenant.0, "exported", None, 0,
            ),
        })
    }

    fn rectify(
        &self,
        subject: &SubjectRef,
        _patch: myelin_gdpr::Patch,
    ) -> myelin_gdpr::Result<myelin_gdpr::RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(myelin_gdpr::RectifyReceipt {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "rectify", self.id, &sid, "*", "rectified", None, 0,
            ),
        })
    }

    fn restrict(
        &self,
        subject: &SubjectRef,
        on: bool,
    ) -> myelin_gdpr::Result<myelin_gdpr::RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on { "restricted:set" } else { "restricted:clear" };
        Ok(myelin_gdpr::RestrictReceipt {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "restrict", self.id, &sid, "*", outcome, None, 0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> myelin_gdpr::Result<EraseReceipt> {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        // The store crypto-shreds ITS OWN key class (never another store's) — the no-cross-store
        // -read law: a holder shreds the key it owns. Idempotent: a re-run no-ops (key gone) and
        // returns the SAME content-addressed receipt (the destroyed epoch is None on the re-run,
        // but the outcome + address are stable; for the byte-identical property the KMS double
        // re-affirms the epoch — see the holders.rs ReaffirmKms pattern).
        let (subject_token, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.clone())
            }
            EraseScope::Tenant(tenant) => ("*tenant*".to_string(), tenant.clone()),
        };
        let handle = crate::holders::ShredKeyHandle {
            tenant: tenant.clone(),
            class: self.key_class.clone(),
        };
        let destroyed_epoch = self.kms.destroy(&handle);
        Ok(EraseReceipt {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "erase",
                self.id,
                &subject_token,
                &tenant.0,
                "crypto_shred:own_key_class",
                destroyed_epoch,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holders::{CryptoShredKms, InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
    use myelin_gdpr::DsrError;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            t("acme"),
        ))
    }

    /// A KMS seeded with one key per upstream holder (each holder shreds its OWN class). We model
    /// the six holders' key classes as distinct `Subject(holder_id)` classes under the tenant so
    /// the drill can assert per-holder "0 recoverable after erase".
    fn kms_with_all_holder_keys(tenant: &TenantId, base_epoch: u64) -> InMemoryShredKms {
        let kms = InMemoryShredKms::new();
        for (i, id) in [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .iter()
        .enumerate()
        {
            kms.provision(
                ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::Subject((*id).to_string()),
                },
                base_epoch + i as u64,
            );
        }
        kms
    }

    fn seam_holders<'a>(kms: &'a InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'a>)> {
        [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .into_iter()
        .map(|id| {
            (
                id,
                SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms),
            )
        })
        .collect()
    }

    // ───────── the canonical erase order (the load-bearing correctness property, §4.1) ─────────

    /// **Identity (H15) is erased FIRST** (phase 0), then the §4.1 sequence: crypto-shred DEK
    /// (H6) → purge/tombstone derived (H14) → bus (H8) → caches (H9) → backups (H10). The
    /// receipts are collected in EXACTLY this order. If a holder were called out of order (e.g. a
    /// downstream holder before Identity), this fails — Identity-first is what guarantees every
    /// downstream holder sees only the opaque pseudonym.
    #[test]
    fn fan_out_calls_holders_in_the_canonical_erase_order_identity_first() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 100);
        let holders = seam_holders(&kms);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
        );

        // The registered order IS the canonical erase order.
        assert_eq!(
            orch.holder_ids_in_order(),
            vec![
                holder_ids::IDENTITY,      // phase 0 — pseudonym map FIRST
                holder_ids::BLOB,          // phase 1 — crypto-shred DEK
                holder_ids::AUTHZ_TUPLES,  // phase 2 — purge/tombstone derived
                holder_ids::BUS,           // phase 3 — bus erase
                holder_ids::CACHE,         // phase 4 — caches/derived copies
                holder_ids::BACKUP,        // phase 5 — backups (consequence of upstream shred)
            ],
            "Identity is erased FIRST; backups LAST (§4.1)"
        );

        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-order"),
            tenant: tenant.clone(),
        };
        let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();

        // The receipts are collected in canonical phase order (Identity first).
        let order: Vec<&str> = receipts.iter().map(|r| r.holder_id).collect();
        assert_eq!(order[0], holder_ids::IDENTITY, "Identity (pseudonym map) is erased FIRST");
        assert_eq!(order.last(), Some(&holder_ids::BACKUP), "backups are erased LAST");
        assert_eq!(
            order,
            vec![
                holder_ids::IDENTITY,
                holder_ids::BLOB,
                holder_ids::AUTHZ_TUPLES,
                holder_ids::BUS,
                holder_ids::CACHE,
                holder_ids::BACKUP,
            ]
        );
    }

    /// The construction sort is structural: passing the holders in a SHUFFLED order still yields
    /// the canonical erase order (Identity first). The caller cannot break the order by mis-ordering
    /// the registration list — the order is a property of the phase, not the call site.
    #[test]
    fn registration_order_does_not_affect_the_canonical_erase_order() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 200);
        let holders = seam_holders(&kms);
        // Register in REVERSE / shuffled order.
        let mut seams: Vec<(&'static str, &dyn PersonalDataHolder)> = holders
            .iter()
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect();
        seams.reverse();
        seams.swap(1, 3);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(seams);
        assert_eq!(
            orch.holder_ids_in_order()[0],
            holder_ids::IDENTITY,
            "however the list is ordered, Identity (phase 0) is erased FIRST"
        );
        assert_eq!(
            orch.holder_ids_in_order().last(),
            Some(&holder_ids::BACKUP),
            "backups (phase 5) are always LAST"
        );
    }

    // ───────── the M1-holder orchestration floor: 100% coverage in order ─────────

    /// **The M1-holder orchestration floor (the prompt GATE):** a subject seeded into the M1
    /// upstream holders → the fan-out hits EVERY existing holder in the canonical order; each
    /// returns a resumable receipt; `erasure_fanout_coverage` reads 100% (1.0); after the
    /// fan-out, every holder's key is 0-recoverable.
    #[test]
    fn m1_holder_orchestration_floor_is_green_full_coverage_in_order() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 300);
        let holders = seam_holders(&kms);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
        );
        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-floor"),
            tenant: tenant.clone(),
        };

        // BEFORE: coverage is 0 (nothing driven).
        assert_eq!(orch.fanout_coverage(&checklist), 0.0);

        let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();
        assert_eq!(receipts.len(), 6, "all six M1 upstream holders were reached");

        // AFTER: 100% coverage of the EXISTING holder set (the floor gate).
        assert_eq!(
            orch.fanout_coverage(&checklist),
            1.0,
            "erasure_fanout_coverage over the M1 holder set reads 100%"
        );

        // Every holder returned a content-addressed receipt recording a destroyed key epoch.
        for r in &receipts {
            assert_eq!(r.receipt.receipt.operation, "erase");
            assert!(r.receipt.receipt.content_hash.starts_with("blake3:"));
            assert!(
                r.receipt.receipt.key_epoch_destroyed.is_some(),
                "holder {} recorded the destroyed key epoch (the GD-4 audit trail)",
                r.holder_id
            );
        }

        // 0 recoverable across every holder's key after the fan-out (the erasure post-condition).
        for id in orch.holder_ids_in_order() {
            let handle = ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject(id.to_string()),
            };
            assert_eq!(
                kms.recoverable_in_backup(&handle),
                0,
                "holder {id}: 0 recoverable after the canonical fan-out"
            );
        }
    }

    // ───────── idempotent + resumable (the §4.1 step-4 property) ─────────

    /// **Resumability: a crashed orchestrator re-drives ONLY un-receipted holders.** We simulate a
    /// crash mid-fan-out by pre-recording the first three holders' receipts into the checklist
    /// (as if a crash happened after them), then re-drive: the first three are SKIPPED (their
    /// `erase` is NOT re-called), only the remaining three are driven, and the result is still a
    /// complete, in-order receipt set.
    #[test]
    fn fan_out_is_resumable_redrives_only_un_receipted_holders() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 400);
        let holders = seam_holders(&kms);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
        );
        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-resume"),
            tenant: tenant.clone(),
        };

        // Simulate a CRASH after the first three phases: drive only Identity/Blob/Authz by
        // running a partial fan-out over a sub-orchestrator, recording into the SAME checklist.
        let first_three: Vec<(&'static str, &dyn PersonalDataHolder)> = holders
            .iter()
            .filter(|(id, _)| {
                *id == holder_ids::IDENTITY
                    || *id == holder_ids::BLOB
                    || *id == holder_ids::AUTHZ_TUPLES
            })
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect();
        let partial = UpstreamHolderOrchestrator::register_m1_upstream(first_three);
        partial.fan_out_erase(&scope, &checklist).unwrap();
        assert_eq!(checklist.done_count(), 3, "the crash left three holders receipted");

        // Record the per-holder erase-call counts after the partial run.
        let calls_after_partial: BTreeMap<&str, u32> =
            holders.iter().map(|(id, h)| (*id, h.erase_call_count())).collect();

        // RE-DRIVE the FULL fan-out on the same checklist (resume after the crash).
        let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();

        // The first three holders were NOT re-called (resumability — they were skipped).
        for id in [holder_ids::IDENTITY, holder_ids::BLOB, holder_ids::AUTHZ_TUPLES] {
            let h = &holders.iter().find(|(hid, _)| *hid == id).unwrap().1;
            assert_eq!(
                h.erase_call_count(),
                calls_after_partial[id],
                "holder {id} was already receipted ⇒ NOT re-called on resume"
            );
        }
        // The remaining three WERE driven on resume.
        for id in [holder_ids::BUS, holder_ids::CACHE, holder_ids::BACKUP] {
            let h = &holders.iter().find(|(hid, _)| *hid == id).unwrap().1;
            assert_eq!(h.erase_call_count(), 1, "holder {id} was driven on resume");
        }
        // The resumed fan-out is complete + in order.
        assert_eq!(receipts.len(), 6);
        assert_eq!(orch.fanout_coverage(&checklist), 1.0);
        assert_eq!(receipts[0].holder_id, holder_ids::IDENTITY);
    }

    /// **Idempotent: a fully re-driven erase re-affirms the SAME receipts** (the holder bodies
    /// no-op + return the same content-addressed receipt; the checklist makes the orchestrator
    /// re-affirm rather than re-call). Re-running a COMPLETE fan-out on the same checklist is a
    /// no-op: the receipts are identical and no holder's `erase` is called again.
    #[test]
    fn re_running_a_complete_fan_out_is_an_idempotent_no_op() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 500);
        let holders = seam_holders(&kms);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
        );
        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-idem"),
            tenant: tenant.clone(),
        };
        let first = orch.fan_out_erase(&scope, &checklist).unwrap();
        let calls_after_first: Vec<u32> = holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        // Re-run the COMPLETE fan-out: every holder is already receipted ⇒ all skipped.
        let second = orch.fan_out_erase(&scope, &checklist).unwrap();
        let calls_after_second: Vec<u32> = holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        assert_eq!(first, second, "an idempotent re-drive returns the SAME receipts");
        assert_eq!(
            calls_after_first, calls_after_second,
            "no holder's erase was re-called on the idempotent re-drive"
        );
    }

    /// **A holder error fails the fan-out and is RESUMABLE: the receipted holders are recorded, so
    /// a retry re-drives only the failed-onward holders.** We inject a failing holder at phase 3
    /// (bus): the fan-out errors, phases 0–2 are receipted (resume-skippable), and a retry (after
    /// the holder is repaired) completes the erase.
    #[test]
    fn a_holder_error_fails_the_fan_out_but_leaves_a_resumable_checklist() {
        struct FailingHolder {
            calls: Mutex<u32>,
            fail: Mutex<bool>,
        }
        impl PersonalDataHolder for FailingHolder {
            fn locate(&self, _s: &SubjectRef, _t: TenantId) -> myelin_gdpr::Result<myelin_gdpr::LocateReport> {
                unreachable!()
            }
            fn export(&self, _s: &SubjectRef, _t: TenantId) -> myelin_gdpr::Result<myelin_gdpr::PortableBundle> {
                unreachable!()
            }
            fn rectify(&self, _s: &SubjectRef, _p: myelin_gdpr::Patch) -> myelin_gdpr::Result<myelin_gdpr::RectifyReceipt> {
                unreachable!()
            }
            fn restrict(&self, _s: &SubjectRef, _on: bool) -> myelin_gdpr::Result<myelin_gdpr::RestrictReceipt> {
                unreachable!()
            }
            fn erase(&self, _scope: EraseScope) -> myelin_gdpr::Result<EraseReceipt> {
                *self.calls.lock().unwrap() += 1;
                if *self.fail.lock().unwrap() {
                    return Err(DsrError("bus holder unavailable".into()));
                }
                Ok(EraseReceipt {
                    receipt: myelin_gdpr::Receipt::content_addressed(
                        "erase", holder_ids::BUS, "u-fail", "acme", "crypto_shred", Some(9), 0,
                    ),
                })
            }
        }

        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 600);
        // Build the holder set with the bus holder replaced by the failing one.
        let id_h = SeamHolder::new(holder_ids::IDENTITY, ShredKeyClass::Subject(holder_ids::IDENTITY.into()), &kms);
        let blob_h = SeamHolder::new(holder_ids::BLOB, ShredKeyClass::Subject(holder_ids::BLOB.into()), &kms);
        let authz_h = SeamHolder::new(holder_ids::AUTHZ_TUPLES, ShredKeyClass::Subject(holder_ids::AUTHZ_TUPLES.into()), &kms);
        let bus_h = FailingHolder { calls: Mutex::new(0), fail: Mutex::new(true) };
        let cache_h = SeamHolder::new(holder_ids::CACHE, ShredKeyClass::Subject(holder_ids::CACHE.into()), &kms);
        let backup_h = SeamHolder::new(holder_ids::BACKUP, ShredKeyClass::Subject(holder_ids::BACKUP.into()), &kms);

        let orch = UpstreamHolderOrchestrator::register_m1_upstream(vec![
            (holder_ids::IDENTITY, &id_h as &dyn PersonalDataHolder),
            (holder_ids::BLOB, &blob_h),
            (holder_ids::AUTHZ_TUPLES, &authz_h),
            (holder_ids::BUS, &bus_h),
            (holder_ids::CACHE, &cache_h),
            (holder_ids::BACKUP, &backup_h),
        ]);
        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-fail"),
            tenant: tenant.clone(),
        };

        // The fan-out ERRORS at the bus holder.
        let err = orch.fan_out_erase(&scope, &checklist);
        assert!(err.is_err(), "a holder error fails the whole fan-out");
        // Phases 0–2 (identity/blob/authz) were receipted before the failure (resumable).
        assert_eq!(checklist.done_count(), 3, "the pre-failure holders are receipted");
        assert!(checklist.is_done(holder_ids::IDENTITY));
        assert!(checklist.is_done(holder_ids::AUTHZ_TUPLES));
        assert!(!checklist.is_done(holder_ids::BUS), "the failed holder is NOT receipted");
        // Downstream holders (cache/backup) were NEVER called (we stop at the failure).
        assert_eq!(cache_h.erase_call_count(), 0, "we do not continue past a failed holder");
        assert_eq!(backup_h.erase_call_count(), 0);

        // REPAIR the bus holder and RETRY: only the failed-onward holders are driven.
        *bus_h.fail.lock().unwrap() = false;
        let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();
        assert_eq!(receipts.len(), 6, "the retry completes the erase");
        assert_eq!(orch.fanout_coverage(&checklist), 1.0);
        // Identity/blob/authz were NOT re-called (already receipted).
        assert_eq!(id_h.erase_call_count(), 1);
        assert_eq!(authz_h.erase_call_count(), 1);
        // The bus holder was called twice (the failed attempt + the successful retry).
        assert_eq!(*bus_h.calls.lock().unwrap(), 2);
        // cache/backup were driven once (on the retry).
        assert_eq!(cache_h.erase_call_count(), 1);
        assert_eq!(backup_h.erase_call_count(), 1);
    }

    // ───────── the no-cross-store-read law (structural) ─────────

    /// **The no-cross-store-read law (§3.1):** the orchestrator holds ONLY `&dyn
    /// PersonalDataHolder` — it cannot name a concrete store type. The structural proof is the
    /// type: [`UpstreamHolderOrchestrator`] stores [`RegisteredHolder`] whose `holder` field is a
    /// trait object. This test pins that the orchestrator can be built from a heterogeneous set of
    /// DIFFERENT holder impls behind the SAME trait object (it never depends on a concrete store).
    /// The crate-manifest / source-import half of the law is asserted in `holders.rs`'s
    /// `gdpr_service_has_no_cross_store_read_import` (extended to cover this module's source).
    #[test]
    fn orchestrator_touches_holders_only_through_the_trait_object() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 700);
        // Two DIFFERENT holder impls (SeamHolder + the GDPR-owned H18) behind the SAME dyn — the
        // orchestrator is polymorphic over the contract, never a concrete store.
        let seam = SeamHolder::new(holder_ids::BLOB, ShredKeyClass::Subject(holder_ids::BLOB.into()), &kms);
        let owned = crate::holders::GdprOwnStoreHolder::new(&kms);
        let registered = vec![
            RegisteredHolder {
                id: holder_ids::IDENTITY,
                phase: CanonicalErasePhase::IdentityPseudonymMap,
                holder: &owned as &dyn PersonalDataHolder,
            },
            RegisteredHolder {
                id: holder_ids::BLOB,
                phase: CanonicalErasePhase::CryptoShredDek,
                holder: &seam,
            },
        ];
        let orch = UpstreamHolderOrchestrator::new(registered);
        assert_eq!(orch.registered_count(), 2);
        assert_eq!(orch.holder_ids_in_order()[0], holder_ids::IDENTITY);
    }

    // ───────── the canonical phase map + telemetry naming (drift guards) ─────────

    #[test]
    fn canonical_phase_map_is_pinned_for_the_six_m1_holders() {
        assert_eq!(canonical_phase_of(holder_ids::IDENTITY), Some(CanonicalErasePhase::IdentityPseudonymMap));
        assert_eq!(canonical_phase_of(holder_ids::BLOB), Some(CanonicalErasePhase::CryptoShredDek));
        assert_eq!(canonical_phase_of(holder_ids::AUTHZ_TUPLES), Some(CanonicalErasePhase::PurgeAndTombstoneDerived));
        assert_eq!(canonical_phase_of(holder_ids::BUS), Some(CanonicalErasePhase::BusErase));
        assert_eq!(canonical_phase_of(holder_ids::CACHE), Some(CanonicalErasePhase::CachesAndDerivedCopies));
        assert_eq!(canonical_phase_of(holder_ids::BACKUP), Some(CanonicalErasePhase::Backups));
        // An unknown holder has no canonical phase (it must declare one in its own prompt).
        assert_eq!(canonical_phase_of("not_a_holder"), None);
        // Identity is strictly before every other phase (the pseudonym-map-first invariant).
        assert!(CanonicalErasePhase::IdentityPseudonymMap < CanonicalErasePhase::CryptoShredDek);
        assert!(CanonicalErasePhase::Backups > CanonicalErasePhase::CachesAndDerivedCopies);
    }

    #[test]
    fn telemetry_signal_names_and_units_are_pinned() {
        assert_eq!(ERASURE_FANOUT_COVERAGE.0, "gdpr.erasure_fanout_coverage");
        assert_eq!(ERASURE_FANOUT_COVERAGE.1, "ratio");
        assert_eq!(CRYPTO_SHRED_LAG.0, "gdpr.crypto_shred_lag");
        assert_eq!(CRYPTO_SHRED_LAG.1, "ms");
    }

    /// The faithful test double's PII-free `id()` accessor returns the holder id it was built
    /// with (pins the accessor against drift — it is the holder's address in the data map).
    #[test]
    fn seam_holder_id_accessor_returns_the_holder_id() {
        let kms = InMemoryShredKms::new();
        let h = SeamHolder::new(holder_ids::BLOB, ShredKeyClass::Subject("u".into()), &kms);
        assert_eq!(h.id(), holder_ids::BLOB);
        assert_eq!(h.id(), "blob_store");
    }

    /// Tenant offboarding (`EraseScope::Tenant`) also fans out in the canonical order — the same
    /// orchestration, a different scope (destroy the per-tenant key class per holder).
    #[test]
    fn tenant_offboarding_fans_out_in_canonical_order() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        // Provision a Tenant-class key per holder.
        for id in [holder_ids::IDENTITY, holder_ids::BLOB, holder_ids::BACKUP] {
            kms.provision(
                ShredKeyHandle { tenant: tenant.clone(), class: ShredKeyClass::Subject(id.to_string()) },
                1,
            );
        }
        let holders: Vec<(&'static str, SeamHolder)> = [holder_ids::IDENTITY, holder_ids::BLOB, holder_ids::BACKUP]
            .into_iter()
            .map(|id| (id, SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), &kms)))
            .collect();
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders.iter().map(|(id, h)| (*id, h as &dyn PersonalDataHolder)).collect(),
        );
        let checklist = EraseChecklist::new();
        let receipts = orch
            .fan_out_erase(&EraseScope::Tenant(tenant.clone()), &checklist)
            .unwrap();
        assert_eq!(receipts[0].holder_id, holder_ids::IDENTITY, "Identity first for offboarding too");
        assert_eq!(orch.fanout_coverage(&checklist), 1.0);
    }
}
