//! # The CI consumer holder (H2) + the per-subject CI-log DEK crypto-shred reach + the CI instance
//! of the ONE posture BY REFERENCE (P-GA-29 → P-332)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§3.2 H2 [P5]** (*the CI
//! subsystem DB + log segments — run actors (pseudonym), log refs, inline free-text PII in log
//! lines; erasure = pseudonymise + **per-subject DEK crypto-shred of isolable log-segment PII**
//! (was a per-tenant floor) + short-TTL log retention*), **§7.1** (the structural floor extends to
//! CI log segments — the per-subject DEK lever), and **§7.4** (the CI instance of the ONE posture,
//! BY REFERENCE — no restatement). Prove-it: `external-insights/04-hard-problems.md` §1 (the
//! free-text crypto-shreds via per-subject DEK; the third-party / interleaved residual is the
//! documented limit).
//!
//! **Contract-index:** owns (orchestration) the **CI-holder fan-out** leg of row **10.1** (the
//! `erase` IMPL is CI's; GDPR REGISTERS H2 into the map + CALLS it in the canonical order); confirms
//! (not restates) row **10.9** (the CI instance BY REFERENCE — [`crate::posture`]). Consumed: **11.4**
//! (the per-subject CI-log DEK, the C1/P5 extension shipped storage-side in P-329 / P-ST-27, reached
//! here through the [`crate::holders::CryptoShredKms`] seam), **11.8** (the T3 log-tier
//! `(job, step, byte-range)` index — the locator that surfaces the isolable segments, P-328 /
//! P-ST-26), **10.3** (the data map H2 registers into, [`crate::datamap`]).
//!
//! ## What THIS prompt (P-GA-29) ships — and what it REUSES (EI-01 §7 coherence)
//! The storage layer already shipped the **mechanism** for the per-subject CI-log DEK: P-329
//! (P-ST-27) added `FirehoseArchiver::with_subject_dek` + `CiLogTier::seal_ci_batch_for_subject`,
//! keying an ISOLABLE inline-PII CI log segment under the subject's DEK (and `seal_ci_batch` stays
//! the per-tenant FALLBACK for non-isolable interleaved PII), recording a `SegmentKeying`
//! (`Tenant | Subject`) on each step span. P-328 (P-ST-26) shipped the `(job, step, byte-range)`
//! log-tier index (11.8). This prompt adds the **GDPR-orchestration** side over that mechanism —
//! the same shape the M3 producer holders ([`crate::producer_holders`]) and the M2 derivative
//! holders ([`crate::derivative_erasure`]) take, NEVER a second orchestrator:
//! 1. **Registration into the data map** ([`ci_holder_schemas`]) — H2 (CI + log segments) declares
//!    its [`crate::datamap::HolderSchema`] so the generated data map surfaces it (the data-map diff
//!    in CI surfaces the new holder; no holder-without-map drift — gdpr §2.2). *The map, not a
//!    hand-written list, drives erasure*, so once H2 is in the map the DSR fan-out reaches it
//!    structurally.
//! 2. **The fan-out reaches it** ([`CiHolderRegistration::register_ci`]) — H2 registers at its
//!    [`ci_phase_of`] phase ([`CanonicalErasePhase::CryptoShredDek`] — the CI log free-text is a
//!    per-subject-DEK-shredded free-text holder, §4.1) alongside the upstream + derivative + producer
//!    holders, so the combined DSR fan-out drives it in the canonical erase order.
//! 3. **The per-subject DEK crypto-shred reaching isolable CI log-segment PII** ([`CiLogHolder`]) —
//!    H2's `erase` (CI-D3) destroys the subject's per-subject CI-log DEK (so their ISOLABLE inline
//!    log PII — live AND in backups — becomes unrecoverable ciphertext) and crypto-shreds the
//!    per-tenant FALLBACK key only for the tenant-offboarding scope (the non-isolable interleaved PII
//!    is not isolable to one subject — the honest per-subject/per-tenant split). The run-GRAPH
//!    structure survives (the PII is shredded, the run topology remains — §3.2).
//! 4. **The CI instance of the ONE posture** (10.9 §7.4) — [`CI_INSTANCE`] CITES the platform anchor
//!    ([`crate::posture::POSTURE_ANCHOR`]) and adds NO restated posture text (confirmed by reference,
//!    the SAME [`crate::posture::reference_is_by_reference`] predicate the Git instance fired).
//!
//! It REUSES [`crate::orchestration::RegisteredHolder`] / [`crate::orchestration::CanonicalErasePhase`]
//! / [`crate::holders::CryptoShredKms`] / [`crate::posture::reference_is_by_reference`] WHOLESALE — it
//! does NOT re-define the orchestrator, the erase order, the crypto-shred mechanism, or the
//! by-reference predicate. The [`CiLogHolder`] is a faithful in-memory model of the live CI subsystem's
//! `erase` impl (the real binding behind the [`myelin_gdpr::PersonalDataHolder`] seam is a config swap
//! at boot — the per-subject CI-log DEK mechanism is `myelin-storage`'s, reached through the KMS seam;
//! never an `import myelin_storage`, the no-cross-store-read law).
//!
//! ## The per-subject-where-isolable / per-tenant-fallback split (named, the honest answer — §3.2 / §7.1)
//! Per-subject is the TARGET where the inline log-line PII is **isolable** to one subject (the C1/P5
//! extension over the per-tenant floor): erasing that subject crypto-shreds exactly their CI log
//! content without touching the rest of the tenant's logs. Per-tenant is the documented FALLBACK for
//! **non-isolable** interleaved free-text PII (many subjects' inline mentions in one segment) — its
//! residual is the ONE platform posture's residual (the third-party / interleaved free-text limit,
//! 10.9 §7.2). This split is named in writing here (the prompt's floor): a subject erase destroys
//! their per-subject CI-log DEK (the isolable reach); the per-tenant fallback's interleaved residual
//! rides the documented lawful-basis limit + `restrict`, identical to the ONE posture residual.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **Per-subject is the target where isolable; per-tenant is the fallback** for non-isolable inline
//!   PII (the honest split — §3.2 / §7.1). The structural reach (the per-subject CI-log DEK shred)
//!   ships here; the interleaved residual is the ONE posture residual (10.9), `[OPEN — LEGAL]` like
//!   every other subsystem's residual — never pretended-solved. Recorded in writing per the
//!   DELIVERABLE.
//! - **The Issues (H3) + Chat (H5) consumer holders** (the next consumer-holder instances over this
//!   SAME pattern) → **P-GA-30 → P-333** (named below — [`CONSUMER_HOLDER_FOLLOW_ON`]).
//! - **The live CI `erase` binding** behind the [`myelin_gdpr::PersonalDataHolder`] seam (the real CI
//!   subsystem `erase` over `myelin-storage`'s per-subject CI-log DEK) is the config swap the producer
//!   holders named; on THIS floor the holder is a faithful in-memory model whose per-subject-DEK
//!   crypto-shred + per-tenant-fallback + structure-survives semantics are the CI-D3 post-conditions.
//!   This module composes already-shipped seams (the crypto-shred KMS, the orchestrator, the data
//!   map, the posture) and touches **NO new DB / object-store / cache / bus contract — no
//!   `--features integration` leg owed** (the per-subject CI-log DEK's OWN live-stack integration
//!   proof is owned storage-side, P-329 / STOR-D4-C1).
//!
//! ## Mutation floor (P-GA-29 TESTS — the per-subject-where-isolable / per-tenant-fallback SELECTION
//! path is mandatory-core). The behavioral core every mutation must be caught on:
//! [`ci_phase_of`] (H2 slots into the correct canonical phase), [`CiHolderRegistration::register_ci`]
//! (H2 registers because the map drives it), and [`CiLogHolder::erase`] (the per-subject-DEK shred of
//! the ISOLABLE segments + the per-tenant FALLBACK selection on a tenant offboarding — `0 dangling
//! leak`). CI's own `erase`-impl floor is owned by CI (this prompt owns the ORCHESTRATION fan-out
//! leg).
//!
//! `cargo mutants -p myelin-gdpr-service --file crates/myelin-gdpr-service/src/ci_instance.rs`
//! (2026-06-22): **49 mutants, 22 caught, 1 missed, 26 unviable** — every BEHAVIORAL mutant on the
//! mandatory-core paths is CAUGHT: [`CiLogHolder::erase`]'s scope selection (subject ⇒ per-subject
//! DEK / tenant ⇒ per-tenant fallback — each polarity pinned by an expected content-address),
//! [`ci_phase_of`] (the canonical phase + the unknown-holder `None`), the data-map registration +
//! coverage-gap detection, the `locate` present/0-recoverable branches, the run-graph
//! structure-survives reading (both polarities), and the holder-id address. The 1 residual is the
//! documented non-core equivalent-wrapper class: `ci_section_references_posture -> true` is the thin
//! NO-ARG public wrapper that delegates to the SHARED [`reference_is_by_reference`] predicate with
//! the REAL production constant ([`CI_INSTANCE`] — a valid by-reference cite); through the public API
//! the production constant ALWAYS yields `true`, so the wrapper's boolean output is unobservable-false
//! — the SAME equivalent-wrapper class already documented for
//! `git_instance::git_section_references_posture` and `audit::verify_chain`, whose delegated LOGIC
//! ([`reference_is_by_reference`]) is mutation-killed in [`crate::posture`]. Stated, not hidden
//! (EI-01 §3).

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::Region;

use crate::datamap::HolderSchema;
use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};
use crate::orchestration::{CanonicalErasePhase, RegisteredHolder};
use crate::posture::{
    reference_is_by_reference, SubsystemReference, CANONICAL_POSTURE, POSTURE_ANCHOR,
};

// ───────────────────────── the CI consumer holder id (§3.2 H2) ─────────────────────────

/// **H2** — the CI subsystem DB + log segments (run actors as pseudonyms, log refs, inline free-text
/// PII in log lines — §3.2). The stable, PII-free holder name CI registers under (contract 1.4 — the
/// data-map / DSR fan-out address book). PII-free: a holder id is a store name, never a subject.
pub const CI_DB: &str = "ci_oltp";

/// The subsystem name the CI erasure-section reference registers under (the §7.4 by-reference cite).
pub const CI_SUBSYSTEM: &str = "ci";

/// The prompt that ships the NEXT consumer-holder instances (Issues H3 + Chat H5) over this SAME
/// consumer-holder + per-derivative-fan-out pattern — named in writing so the follow-on is never
/// pretended-shipped (VISION §3 name-your-floors).
pub const CONSUMER_HOLDER_FOLLOW_ON: &str = "P-GA-30 → P-333 (Issues H3 + Chat H5)";

/// **The canonical phase H2 occupies in the §4.1 erase order.** The CI holder's **identity** (run
/// actors) is shredded by Identity (phase 0 — pseudonym map); its **inline free-text log PII** is
/// crypto-shred via the per-subject CI-log DEK at [`CanonicalErasePhase::CryptoShredDek`] (alongside
/// the H6 blob + the other free-text DEK holders — §4.1 step "KMS.destroy"). H2 declares its phase
/// HERE (not via [`crate::orchestration::canonical_phase_of`], which knows only the six upstream
/// holders) — the §4.1 order is a property of the phase, so the CI holder slots in correctly without
/// re-deriving a hand-written sequence.
pub fn ci_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        CI_DB => Some(CanonicalErasePhase::CryptoShredDek),
        _ => None,
    }
}

// ───────────────────────── registration into the data map (gdpr §2.2 / contract 10.3) ─────────────────────────

/// **H2's contribution to the generated data map (gdpr §2.2; contract 10.3).** The CI store (H2,
/// the CI DB plus log segments) declares its [`HolderSchema`] so the data-map generator surfaces it
/// — *the data-map diff in CI surfaces the new holder; no holder-without-map drift* (gdpr §2.2).
/// Once H2 is in the map, the DSR fan-out reaches it STRUCTURALLY (the map, not a hand-written list,
/// drives erasure — §4.1 step 2).
///
/// H2's `#[personal_data]`-tagged PII fields (run actors as pseudonyms, log refs, inline log-line
/// PII) are owned by the CI subsystem's schema (its classify-derive); on this floor the registration
/// carries the holder roster entry (the holder id + H-number + region) so the holder appears in the
/// map's **roster** even before its full per-field slice ships from the CI crate — the GA-D1 "0
/// holders missed" property reads the roster. The per-field slice grows without a generator change as
/// the CI schema lands (gdpr §2.2; the M5 completeness floor P-GA-32).
///
/// `region` is the cell the CI store resides in (residency-pinned — gdpr §2.2 / ADR-11).
pub fn ci_holder_schemas(region: Region) -> Vec<HolderSchema> {
    vec![HolderSchema {
        registration: HolderRegistration {
            kind: StoreKind::Oltp,
            name: CI_DB,
        },
        holder: Holder::H2Ci,
        region,
        fields: &[],
    }]
}

/// The [`HolderRegistration`] the harness records for the CI store (the auto-registered holder set
/// the data-map coverage gate reads — [`crate::datamap::Inventory::coverage_gaps`]). H2 REGISTERED
/// (the harness opened the store) but absent from the map would be a coverage gap; once
/// [`ci_holder_schemas`] contributes it, the gap closes.
pub fn ci_registrations() -> Vec<HolderRegistration> {
    vec![HolderRegistration {
        kind: StoreKind::Oltp,
        name: CI_DB,
    }]
}

// ───────────────────────── the CI instance of the ONE posture, BY REFERENCE (§7.4) ─────────────────────────

/// **The CI erasure-section instance of the ONE posture — BY REFERENCE (§7.4).** The canonical §7.4
/// short form: it CITES the platform anchor ([`POSTURE_ANCHOR`]) and adds **no restated posture text**
/// (the structural floor / residual / lawful-basis text lives ONCE in [`CANONICAL_POSTURE`]). It names
/// CI's specifics (the per-subject CI-log DEK reaches isolable inline log PII; the non-isolable
/// interleaved residual is the documented limit) ONLY by reference to the posture, never restating the
/// canonical levers. This is the consumer half of the 10.9 CDC pair for the CI subsystem — the SAME
/// [`reference_is_by_reference`] predicate the Git instance (P-GA-28) fired.
pub const CI_INSTANCE: SubsystemReference = SubsystemReference {
    subsystem: CI_SUBSYSTEM,
    cited_anchor: POSTURE_ANCHOR,
    section_text:
        "Free-text / immutable-content erasure follows the platform posture in \
         00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). CI inline \
         log-line PII that is isolable to one subject is sealed under that subject's per-subject \
         CI-log DEK, so an erase crypto-shreds exactly their CI log content (live and backups) while \
         the run-graph structure survives; non-isolable interleaved PII rides the per-tenant fallback \
         and its residual is the ONE platform-posture residual.",
};

/// **The architecture test that the CI erasure section REFERENCES the platform posture (does not
/// restate it) — the P-GA-29 GATE.** Returns `true` iff [`CI_INSTANCE`] is a valid by-reference
/// instantiation (cites the canonical anchor + adds no restated posture text). Delegates to the
/// SHARED [`reference_is_by_reference`] predicate (the SAME the Git instance fired); the CI instance
/// is a real subsystem register over it.
#[must_use]
pub fn ci_section_references_posture() -> bool {
    reference_is_by_reference(&CI_INSTANCE)
}

/// **The CI residual == the ONE platform-posture residual (§7.2 / §7.4).** The CI instance's residual
/// (the non-isolable interleaved free-text PII the per-tenant fallback cannot crypto-shred to one
/// subject) IS the canonical [`CANONICAL_POSTURE`]`.residual` — confirmed equal, never re-described.
/// Exposed so a consumer reads "the CI residual" and gets back the single source.
#[must_use]
pub const fn ci_residual() -> &'static str {
    CANONICAL_POSTURE.residual
}

// ───────────────────────── the opaque (subject, tenant) extractor ─────────────────────────

/// The opaque, PII-free `(subject_token, tenant_token)` for a scope (a tenant offboarding records the
/// `"*tenant*"` sentinel subject). Never a name/email.
fn subject_and_tenant(scope: &EraseScope) -> (String, String) {
    match scope {
        EraseScope::Subject { subject, tenant } => {
            (subject.principal.principal_id.0.clone(), tenant.0.clone())
        }
        EraseScope::Tenant(tenant) => ("*tenant*".to_string(), tenant.0.clone()),
    }
}

// ───────────────────────── H2 — the CI subsystem DB + log segments (the per-subject CI-log DEK reach) ─────────────────────────

/// A faithful in-memory model of the CI subsystem store for a subject (H2). It holds the subject's
/// inline log-line PII as TWO key classes (the per-subject/per-tenant split §3.2 / §7.1): ISOLABLE
/// inline PII sealed under the subject's per-subject CI-log DEK (crypto-shred-erasable to exactly
/// that subject — the C1/P5 reach), and a per-tenant FALLBACK for non-isolable interleaved PII. The
/// model also tracks the run-GRAPH topology (which survives an erase — the structure remains, the PII
/// is shredded). The live CI `erase` over `myelin-storage`'s per-subject CI-log DEK is the named
/// floor; this model has the CI-D3 post-conditions (0 dangling leak; structure survives).
#[derive(Debug, Default)]
pub struct CiLogModel {
    /// `subject_token → whether the subject's run-graph node is STILL present`. The run topology is
    /// NON-PII structure that survives an erase (the §3.2 "structure survives, PII is shredded"
    /// property); an erase NEVER drops it.
    run_graph: Mutex<BTreeMap<String, bool>>,
    /// The number of `erase` CALLS (the resumability / fan-out witness).
    erase_calls: Mutex<u32>,
}

impl CiLogModel {
    /// A fresh, empty CI log model.
    pub fn new() -> CiLogModel {
        CiLogModel::default()
    }

    /// Record a subject's run-graph node FROM SOURCE (the live CI run-state write step). The inline
    /// log PII itself is governed by the per-subject CI-log DEK (the KMS seam — provisioned by
    /// Storage's KMS hierarchy at seal time, `seal_ci_batch_for_subject`); the run-graph node is the
    /// NON-PII structure this tracks (it survives an erase).
    pub fn index_run_graph_from_source(&self, subject_token: &str) {
        self.run_graph
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
    }

    /// **Whether the subject's run-GRAPH node still exists (the CI-D3 "structure survives" reading).**
    /// MUST stay `true` after an erase — the run topology is NON-PII and remains; only the inline log
    /// PII (the per-subject CI-log DEK ciphertext) is rendered unrecoverable.
    pub fn run_graph_present(&self, subject_token: &str) -> bool {
        self.run_graph
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .copied()
            .unwrap_or(false)
    }

    /// How many times `erase` was actually CALLED (the resumability witness).
    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Bump the erase-call counter (the resumability witness) — the structure-survives erase is a
    /// no-op on the run graph (it leaves it present by construction).
    fn note_erase(&self) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

/// **H2 — the CI subsystem DB + log segments AS a [`PersonalDataHolder`] (contract 10.1 / 10.9 §7.4 —
/// the CI instance of the ONE posture).** Its `erase` (CI-D3) crypto-shreds the subject's inline
/// log-line PII via the **per-subject CI-log DEK** ([`CryptoShredKms`] — the C1/P5 reach over isolable
/// segments) where the scope is a single subject, and crypto-shreds the **per-tenant FALLBACK** key on
/// a tenant offboarding (the non-isolable interleaved PII). The run-GRAPH structure survives (the PII
/// is shredded, the topology remains). Run-actor identity is pseudonymised (the Id lever ran in phase
/// 0). Reached ONLY through the contract (the no-cross-store-read law — never an `import` of the CI /
/// storage crate). This is the CI instance of the platform erasure posture, BY REFERENCE ([`CI_INSTANCE`]).
pub struct CiLogHolder<'a> {
    model: &'a CiLogModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> CiLogHolder<'a> {
    /// Build the H2 holder over a CI log model + the crypto-shred KMS seam (the live CI store over
    /// `myelin-storage`'s per-subject CI-log DEK at boot; the model in the drill).
    pub fn new(model: &'a CiLogModel, kms: &'a dyn CryptoShredKms) -> CiLogHolder<'a> {
        CiLogHolder { model, kms }
    }

    /// The PII-free holder id this holder registers under ([`CI_DB`]).
    pub fn holder_id(&self) -> &'static str {
        CI_DB
    }

    /// The **per-subject CI-log DEK** handle the subject's ISOLABLE inline log PII is sealed under
    /// (the C1/P5 reach — the crypto-shred key class that erases exactly that subject's CI log).
    fn subject_dek(subject_token: &str, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject_token.to_string()),
        }
    }

    /// The **per-tenant FALLBACK** DEK handle the NON-isolable interleaved CI-log PII is sealed under
    /// (the documented fallback for inline PII that is not isolable to one subject — destroyed only on
    /// a tenant offboarding, when the whole tenant goes).
    fn tenant_dek(tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Tenant,
        }
    }
}

impl PersonalDataHolder for CiLogHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        // The subject's isolable CI log PII is recoverable iff their per-subject CI-log DEK is live.
        let outcome = if self.kms.is_present(&Self::subject_dek(&sid, &tenant)) {
            "located:ci-log-segments-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed("locate", CI_DB, &sid, &tenant.0, outcome, None, 0),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export", CI_DB, &sid, &tenant.0, "exported", None, 0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed("rectify", CI_DB, &sid, "*", "rectified", None, 0),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed("restrict", CI_DB, &sid, "*", outcome, None, 0),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        self.model.note_erase();
        // **The per-subject-where-isolable / per-tenant-fallback SELECTION (the mandatory-core path,
        // §3.2 / §7.1).** A single-subject erase destroys exactly THAT subject's per-subject CI-log
        // DEK (the isolable inline PII reach — the C1/P5 extension), leaving every other subject's CI
        // log AND the per-tenant fallback untouched. A tenant offboarding destroys the per-tenant
        // FALLBACK key too (the whole tenant goes — the non-isolable interleaved PII included). The
        // run-graph structure survives in BOTH cases (the PII is shredded, the topology remains).
        let (destroyed, outcome) = match &scope {
            EraseScope::Subject { .. } => (
                self.kms.destroy(&Self::subject_dek(&sid, &tenant)),
                "crypto_shred:per_subject_ci_log_dek:isolable_segments;structure_survives",
            ),
            EraseScope::Tenant(_) => {
                // The tenant fallback goes WITH the tenant (the whole tenant's CI logs, isolable +
                // interleaved). Destroying the per-tenant DEK is the load-bearing offboarding shred.
                let d = self.kms.destroy(&Self::tenant_dek(&tenant));
                (
                    d,
                    "crypto_shred:per_tenant_ci_log_dek_fallback:tenant_offboard;structure_survives",
                )
            }
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                CI_DB,
                &sid,
                &tenant_token,
                outcome,
                destroyed,
                0,
            ),
        })
    }
}

// ───────────────────────── the CI-holder registration (the fan-out reaches it) ─────────────────────────

/// **The CI-holder registration (P-GA-29 — the orchestration leg of 10.1 over the CI consumer
/// subsystem).** Wires H2 into the combined DSR fan-out at its canonical phase. It REUSES the
/// [`RegisteredHolder`] seam + the [`CanonicalErasePhase`] order — H2 registers at its [`ci_phase_of`]
/// phase alongside the upstream + derivative + producer holders (the §4.1 order is a property of the
/// phase). It NEVER reaches into the CI store — it holds only `&dyn PersonalDataHolder` (the
/// no-cross-store-read law).
pub struct CiHolderRegistration;

impl CiHolderRegistration {
    /// **Register the CI consumer holder (H2) at its canonical phase.** The caller passes the holder
    /// seam (the live CI `erase` at boot; the faithful model in the drill); it is registered at its
    /// [`ci_phase_of`] phase so a combined fan-out over upstream + derivative + producer + consumer
    /// holders runs in the canonical erase order. A holder id without a known CI phase is rejected (it
    /// must declare one — the "we forgot a holder" trap is foreclosed structurally).
    pub fn register_ci<'a>(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> Vec<RegisteredHolder<'a>> {
        holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = ci_phase_of(id)
                    .unwrap_or_else(|| panic!("CI holder `{id}` has no canonical erase phase"));
                RegisteredHolder { id, phase, holder }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamap::data_map;
    use crate::holders::InMemoryShredKms;
    use crate::orchestration::UpstreamHolderOrchestrator;
    use crate::posture::restatement_markers;
    use crate::EraseChecklist;
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

    fn subject_scope(s: &str) -> EraseScope {
        EraseScope::Subject {
            subject: subject(s),
            tenant: t("acme"),
        }
    }

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn provision_subject_dek(
        kms: &InMemoryShredKms,
        tenant: &TenantId,
        subject_token: &str,
        epoch: u64,
    ) {
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject(subject_token.to_string()),
            },
            epoch,
        );
    }

    fn provision_tenant_dek(kms: &InMemoryShredKms, tenant: &TenantId, epoch: u64) {
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Tenant,
            },
            epoch,
        );
    }

    // ───────── registration into the data map (gdpr §2.2 — no holder-without-map drift) ─────────

    /// **H2 appears in the generated data map after registration (gdpr §2.2).** The CI holder
    /// contributes its [`HolderSchema`] → the data-map roster surfaces it. *The data-map diff in CI
    /// surfaces the new holder.* A holder REGISTERED but absent from the map is a coverage gap; once it
    /// contributes, the gap closes (0 holders missed).
    #[test]
    fn ci_holder_appears_in_the_data_map_after_registration() {
        let inv = data_map(&ci_holder_schemas(region()));
        assert!(inv.holders.contains("oltp:ci_oltp"), "H2 CI is in the map");
        assert_eq!(inv.holder_count(), 1, "exactly the one CI holder");
        // No holder-without-map drift: the REGISTERED CI holder is in the map (0 gaps).
        assert!(
            inv.coverage_gaps(&ci_registrations()).is_empty(),
            "the registered CI holder is in the map — 0 holders missed"
        );
    }

    /// **The RED coverage verdict.** H2 is REGISTERED (the harness opened the CI store) but did NOT
    /// contribute a [`HolderSchema`] — a coverage gap the data-map diff surfaces (the DSR fan-out
    /// cannot silently skip the CI store the map forgot).
    #[test]
    fn a_registered_ci_holder_absent_from_the_map_is_a_coverage_gap() {
        // The map is generated over an UNRELATED holder; the harness registered the CI store.
        let inv = data_map(&[]);
        let gaps = inv.coverage_gaps(&ci_registrations());
        assert_eq!(
            gaps,
            vec!["oltp:ci_oltp".to_string()],
            "the registered-but-unmapped CI holder is the coverage gap"
        );
    }

    // ───────── the canonical phase (H2 slots into the correct erase phase) ─────────

    /// **H2 declares its canonical erase phase (§4.1).** The CI inline log PII crypto-shreds at
    /// [`CanonicalErasePhase::CryptoShredDek`] (the per-subject CI-log DEK is a free-text DEK holder).
    /// The §4.1 order is a property of the phase — H2 slots in without re-deriving a hand-written
    /// sequence.
    #[test]
    fn ci_holder_declares_its_canonical_erase_phase() {
        assert_eq!(
            ci_phase_of(CI_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        // An unknown holder has no CI phase (it must declare one in its own prompt).
        assert_eq!(ci_phase_of("not_the_ci_store"), None);
    }

    /// **The CI holder registers under the frozen `ci_oltp` id (the data-map / fan-out address).** The
    /// holder id is the SAME id the schema + the registration use (ONE name — EI-01 §7); a drifted id
    /// would leave the holder unreachable by the map-driven fan-out. Pins the accessor (kills the
    /// `holder_id -> ""` / `"xyzzy"` mutants).
    #[test]
    fn ci_holder_id_is_the_frozen_ci_oltp_address() {
        let kms = InMemoryShredKms::new();
        let model = CiLogModel::new();
        let holder = CiLogHolder::new(&model, &kms);
        assert_eq!(
            holder.holder_id(),
            CI_DB,
            "the holder id is the frozen ci_oltp address"
        );
        assert_eq!(holder.holder_id(), "ci_oltp");
        // The id matches the schema's registration id (the map addresses it by the same name).
        assert_eq!(
            ci_holder_schemas(region())[0].holder_id(),
            "oltp:ci_oltp",
            "the schema registers the holder under the same store name"
        );
    }

    // ───────── the fan-out reaches H2 (the data map drives it) ─────────

    /// **The combined DSR fan-out checklist INCLUDES H2 (the mandatory-core path).** The CI holder
    /// registers through [`CiHolderRegistration::register_ci`] and joins the orchestrator; the fan-out
    /// reaches it in the canonical erase order. This is the "the fan-out reaches it because the data
    /// map drives it" property.
    #[test]
    fn the_fan_out_reaches_the_ci_holder_in_canonical_order() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-ci", 10);

        let model = CiLogModel::new();
        model.index_run_graph_from_source("u-ci");
        let ci_h = CiLogHolder::new(&model, &kms);

        let ci = CiHolderRegistration::register_ci(vec![(CI_DB, &ci_h as &dyn PersonalDataHolder)]);
        let orch = UpstreamHolderOrchestrator::new(ci);

        let ids = orch.holder_ids_in_order();
        assert!(ids.contains(&CI_DB), "H2 CI is in the fan-out");

        let checklist = EraseChecklist::new();
        let receipts = orch
            .fan_out_erase(&subject_scope("u-ci"), &checklist)
            .unwrap();
        assert_eq!(receipts.len(), 1, "the CI holder was reached");
        assert_eq!(
            orch.fanout_coverage(&checklist),
            1.0,
            "100% coverage of the CI holder"
        );
    }

    // ───────── CI-D3: the per-subject CI-log DEK crypto-shred reaches isolable log PII; structure survives ─────────

    /// **CI-D3: a subject erase crypto-shreds exactly that subject's per-subject CI-log DEK (the
    /// isolable inline PII reach) — 0 dangling leak — while a SECOND subject's CI log AND the
    /// per-tenant fallback survive, and the run-graph structure survives.** This is the C1/P5
    /// extension's headline: the per-subject reach, not a per-tenant blunt erase.
    #[test]
    fn ci_d3_per_subject_dek_shred_reaches_isolable_log_pii_structure_survives() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-erase", 20);
        provision_subject_dek(&kms, &tenant, "u-other", 21);
        provision_tenant_dek(&kms, &tenant, 22);
        let model = CiLogModel::new();
        model.index_run_graph_from_source("u-erase");
        model.index_run_graph_from_source("u-other");

        let erase_dek = CiLogHolder::subject_dek("u-erase", &tenant);
        let other_dek = CiLogHolder::subject_dek("u-other", &tenant);
        let tenant_dek = CiLogHolder::tenant_dek(&tenant);
        assert!(
            kms.is_present(&erase_dek),
            "the subject's CI-log DEK is live before erase"
        );

        let holder = CiLogHolder::new(&model, &kms);
        let receipt = holder.erase(subject_scope("u-erase")).unwrap();

        // 0 dangling leak for the erased subject: their per-subject CI-log DEK is destroyed (live AND
        // backups), so their isolable inline PII is unrecoverable ciphertext.
        assert!(
            !kms.is_present(&erase_dek),
            "the subject's per-subject CI-log DEK is destroyed"
        );
        assert_eq!(
            kms.recoverable_in_backup(&erase_dek),
            0,
            "0 recoverable in backups (crypto-shred reaches backups — CI-D3)"
        );
        // The per-subject reach: a SECOND subject's CI log AND the per-tenant fallback are untouched.
        assert!(
            kms.is_present(&other_dek),
            "a different subject's CI log survives (the per-subject reach, not a blunt per-tenant erase)"
        );
        assert!(
            kms.is_present(&tenant_dek),
            "the per-tenant fallback key survives a single-subject erase"
        );
        // Structure survives: the run-graph topology remains (the PII is shredded, not the structure).
        assert!(
            model.run_graph_present("u-erase"),
            "the run-graph structure survives the erase (§3.2 — structure survives, PII shredded)"
        );
        // A subject that was NEVER indexed has no run-graph node (kills the `run_graph_present -> true`
        // mutant — the structure-survives reading is observable on both polarities).
        assert!(
            !model.run_graph_present("u-never-indexed"),
            "an un-indexed subject has no run-graph node (present is observably false)"
        );
        // The receipt records the per-subject-DEK destroy + is content-addressed.
        assert!(
            receipt.receipt.key_epoch_destroyed.is_some(),
            "the erase receipt records the destroyed per-subject-DEK epoch (CI-D3 telemetry)"
        );
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
        // The receipt names the per-subject CI-log DEK reach — proven via the expected content-address
        // (the outcome string is folded into the content hash, not a stored field).
        let expected = Receipt::content_addressed(
            "erase",
            CI_DB,
            "u-erase",
            &tenant.0,
            "crypto_shred:per_subject_ci_log_dek:isolable_segments;structure_survives",
            receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            receipt.receipt.content_hash, expected.content_hash,
            "the receipt names the per-subject CI-log DEK reach (the C1/P5 extension)"
        );
    }

    /// **The per-tenant FALLBACK fires on a tenant offboarding (the non-isolable interleaved PII).**
    /// A `EraseScope::Tenant` offboarding destroys the per-tenant CI-log DEK fallback (the whole
    /// tenant's logs go — isolable + interleaved). This is the OTHER polarity of the mandatory-core
    /// selection path: subject ⇒ per-subject DEK; tenant ⇒ per-tenant fallback. A mutant that
    /// collapsed the two would be caught here (the tenant fallback key would otherwise survive).
    #[test]
    fn the_per_tenant_fallback_fires_on_a_tenant_offboarding() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-iso", 30);
        provision_tenant_dek(&kms, &tenant, 31);
        let model = CiLogModel::new();

        let subject_dek = CiLogHolder::subject_dek("u-iso", &tenant);
        let tenant_dek = CiLogHolder::tenant_dek(&tenant);
        let holder = CiLogHolder::new(&model, &kms);

        let receipt = holder.erase(EraseScope::Tenant(tenant.clone())).unwrap();

        // The per-tenant fallback key is destroyed (the offboarding shred — the interleaved PII goes).
        assert!(
            !kms.is_present(&tenant_dek),
            "a tenant offboarding destroys the per-tenant CI-log DEK fallback"
        );
        assert_eq!(
            kms.recoverable_in_backup(&tenant_dek),
            0,
            "0 recoverable in backups"
        );
        // The tenant-scope erase names the per-tenant fallback (proven via the expected content-hash;
        // the offboarding subject token is the "*tenant*" sentinel).
        let expected_tenant = Receipt::content_addressed(
            "erase",
            CI_DB,
            "*tenant*",
            &tenant.0,
            "crypto_shred:per_tenant_ci_log_dek_fallback:tenant_offboard;structure_survives",
            receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            receipt.receipt.content_hash, expected_tenant.content_hash,
            "the tenant-scope erase names the per-tenant fallback (the selection polarity)"
        );
        // A subject erase names the per-subject reach, NOT the fallback — proving the selection is
        // observable on both polarities (kills the "always per-subject" / "always per-tenant" mutant).
        let subject_receipt = holder.erase(subject_scope("u-iso")).unwrap();
        let expected_subject = Receipt::content_addressed(
            "erase",
            CI_DB,
            "u-iso",
            &tenant.0,
            "crypto_shred:per_subject_ci_log_dek:isolable_segments;structure_survives",
            subject_receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            subject_receipt.receipt.content_hash, expected_subject.content_hash,
            "a subject erase names the per-subject reach (not the fallback)"
        );
        // The two outcomes differ (the selection is load-bearing, not a constant string).
        assert_ne!(
            receipt.receipt.content_hash,
            subject_receipt.receipt.content_hash
        );
        assert!(
            !kms.is_present(&subject_dek),
            "the subject's per-subject CI-log DEK is destroyed"
        );
    }

    /// **An idempotent re-erase returns a stable content-addressed receipt + does not resurrect the
    /// structure.** The CI holder is idempotent (a re-run no-ops on the already-destroyed DEK + leaves
    /// the run-graph present) — the §4.1-step-4 resumability the combined fan-out relies on.
    #[test]
    fn ci_holder_erase_is_idempotent() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-idem", 40);
        let model = CiLogModel::new();
        model.index_run_graph_from_source("u-idem");
        let holder = CiLogHolder::new(&model, &kms);

        let first = holder.erase(subject_scope("u-idem")).unwrap();
        let second = holder.erase(subject_scope("u-idem")).unwrap();
        assert_eq!(first.receipt.operation, second.receipt.operation);
        // The re-erase: the DEK was already gone (key_epoch_destroyed None now), the structure remains.
        assert!(
            second.receipt.key_epoch_destroyed.is_none(),
            "the re-erase destroyed no key"
        );
        assert!(
            model.run_graph_present("u-idem"),
            "the structure survives the re-erase too"
        );
        assert_eq!(model.erase_call_count(), 2, "both erase calls were counted");
    }

    /// **The CI `locate` distinguishes present-PII from 0-recoverable on the per-subject CI-log DEK
    /// presence (mandatory-core).** A live DEK ⇒ isolable CI log PII located; a destroyed DEK ⇒
    /// 0-recoverable. The exact content-addressed receipts pin the outcome string (kills the
    /// always-located / always-zero mutants).
    #[test]
    fn ci_locate_reports_present_on_a_live_dek_and_zero_after_shred() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-loc", 50);
        let model = CiLogModel::new();
        let holder = CiLogHolder::new(&model, &kms);

        let present = holder.locate(&subject("u-loc"), tenant.clone()).unwrap();
        let expected_present = Receipt::content_addressed(
            "locate",
            CI_DB,
            "u-loc",
            &tenant.0,
            "located:ci-log-segments-present",
            None,
            0,
        );
        assert_eq!(present.receipt.content_hash, expected_present.content_hash);

        holder.erase(subject_scope("u-loc")).unwrap();
        let after = holder.locate(&subject("u-loc"), tenant.clone()).unwrap();
        let expected_zero = Receipt::content_addressed(
            "locate",
            CI_DB,
            "u-loc",
            &tenant.0,
            "located:0-recoverable",
            None,
            0,
        );
        assert_eq!(
            after.receipt.content_hash, expected_zero.content_hash,
            "after the per-subject CI-log DEK shred, locate reports 0-recoverable"
        );
        assert_ne!(present.receipt.content_hash, after.receipt.content_hash);
    }

    // ───────── the CI instance: references the ONE posture, never restates (§7.4) ─────────

    /// **The CI instance references the platform posture (does not restate it) — the P-GA-29 GATE.**
    /// The CI erasure section cites the canonical anchor and adds no restated posture text (the X-7
    /// anti-pattern is foreclosed). The SAME [`reference_is_by_reference`] predicate the Git instance
    /// fired — a real subsystem register over it.
    #[test]
    fn the_ci_instance_references_the_posture_and_does_not_restate() {
        assert_eq!(CI_INSTANCE.subsystem, "ci");
        assert_eq!(
            CI_INSTANCE.cited_anchor, POSTURE_ANCHOR,
            "the CI instance cites the ONE anchor"
        );
        assert!(
            ci_section_references_posture(),
            "the CI erasure section is a valid BY-REFERENCE instantiation (cites + does not restate)"
        );
        // It carries NONE of the canonical restatement markers (the posture sentences that may live
        // ONLY in the ONE artifact) — the X-7 anti-pattern is structurally absent.
        let lowered = CI_INSTANCE.section_text.to_ascii_lowercase();
        for marker in restatement_markers() {
            assert!(
                !lowered.contains(&marker.to_ascii_lowercase()),
                "the CI section must not restate the canonical marker {marker:?}"
            );
        }
    }

    /// **A CI-shaped section that RESTATES the posture is rejected** — the gate forbids the X-7
    /// anti-pattern even for CI. Pins that the predicate is load-bearing for the CI register.
    #[test]
    fn a_restating_ci_section_would_be_rejected() {
        let restating = SubsystemReference {
            subsystem: "ci",
            cited_anchor: POSTURE_ANCHOR,
            // Restates the structural floor — the forbidden duplication (a canonical marker phrase).
            section_text: "CI erasure: per-subject DEK crypto-shred renders isolable log-line PII \
                 unrecoverable; the documented lawful-basis limit covers interleaved mentions ...",
        };
        assert!(
            !reference_is_by_reference(&restating),
            "a CI section that restates the posture (a canonical marker) is rejected — X-7"
        );
    }

    /// **The CI residual == the ONE platform-posture residual (§7.2 / §7.4).** The CI instance's
    /// residual (the non-isolable interleaved free-text PII the per-tenant fallback cannot shred to one
    /// subject) IS the canonical residual — confirmed equal, never re-described.
    #[test]
    fn ci_residual_is_the_one_platform_posture_residual() {
        assert_eq!(
            ci_residual(),
            CANONICAL_POSTURE.residual,
            "the CI residual IS the single-source canonical residual (not a CI-specific restatement)"
        );
        assert!(
            ci_residual().contains("AUTHOR's DEK") && ci_residual().contains("not the subject's"),
            "the residual is third-party / interleaved PII under the AUTHOR's DEK — not shreddable by the subject's key"
        );
    }

    /// The next consumer-holder follow-on (Issues H3 + Chat H5) is named in writing (the SAME
    /// consumer-holder pattern) — the floor is never pretended-shipped.
    #[test]
    fn the_consumer_holder_follow_on_is_named() {
        assert!(
            CONSUMER_HOLDER_FOLLOW_ON.contains("P-GA-30"),
            "the Issues/Chat consumer holders are the named follow-on"
        );
    }
}
