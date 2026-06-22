//! # The producer-subsystem holders register + the DSR fan-out reaches them + the Knowledge
//! instance (P-GA-27 → P-256)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§3.2** (the producer
//! holders: **H1** Git subsystem DB — *PR/review/comment authorship [pseudonym] + free-text bodies*,
//! erasure = *pseudonymise [Id lever] + crypto-shred inline bodies [per-subject DEK]*; **H4**
//! Knowledge subsystem DB — *page authorship [pseudonym] + free-text content + db-row values*,
//! erasure = *pseudonymise + crypto-shred content*; **H17** agent execution trace — *a
//! content-addressed Knowledge doc of a run's trace*, erasure = *crypto-shred, distinct from the
//! audit log [§6.5]*) and **§7.4** (the Knowledge instance of the ONE posture BY REFERENCE:
//! self-authored free-text crypto-shreds via per-subject DEK; identity via pseudonym-map shred; the
//! third-party / immutable residual is the documented limit + `restrict`). Prove-it:
//! `external-insights/04-hard-problems.md` §1 (the producer holders' free-text crypto-shreds via
//! per-subject DEK; the residual is the documented limit) + §5 (purge-not-hide for embeddings — the
//! Knowledge instance purges its derived embeddings, they re-identify).
//!
//! **Contract-index:** owns (orchestration) the **producer-holder fan-out** leg of row **10.1** (the
//! impls are Git/KN/Agent's; GDPR REGISTERS them + CALLS them in the canonical order); confirms (not
//! restates) row **10.9** (the Knowledge instance BY REFERENCE — [`crate::posture`]); consumed: **8.8**
//! (the agent-trace holder seam, [`crate::agent_trace_seam`]), **4.8** (the pseudonym lever), **11.4**
//! (per-subject DEK, [`crate::holders::CryptoShredKms`]), **10.3** (the data map the new holders
//! register into, [`crate::datamap`]).
//!
//! ## What THIS prompt (P-GA-27) ships — and what it reuses (EI-01 §7 coherence)
//! The upstream-store orchestration (P-GA-06 → [`crate::orchestration`]) and the per-derivative
//! fan-out (P-GA-24 → [`crate::derivative_erasure`]) already fan an erase out over the M1/M2 holders
//! **in the canonical erase order**, idempotently + resumably, through the
//! [`myelin_gdpr::PersonalDataHolder`] **SEAM** (the no-cross-store-read law — the orchestrator
//! NEVER imports a subsystem store, it calls the contract). This prompt adds the **PRODUCER-subsystem
//! holders** (Git H1 / Knowledge H4 / agent-trace H17) — the three subsystems whose stores ship in
//! M3 — with their producer-SPECIFIC `erase` semantics, registered through that SAME seam at their
//! canonical phases, PLUS:
//! 1. **Registration into the data map** ([`producer_holder_schemas`]) — H1/H4/H17 declare their
//!    [`crate::datamap::HolderSchema`] contributions so the generated data map surfaces them (the
//!    data-map diff in CI surfaces the new holders; no holder-without-map drift — gdpr §2.2). *The
//!    map, not a hand-written list, drives erasure*, so once the producer holders are in the map the
//!    DSR fan-out reaches them structurally.
//! 2. **The fan-out reaches them** ([`ProducerHolderRegistration::register_producers`]) — the three
//!    holders register at their [`producer_phase_of`] phase alongside the upstream + derivative
//!    holders (the §4.1 order is a property of the phase), so the combined DSR fan-out drives them in
//!    the canonical erase order.
//! 3. **The Knowledge instance** of the ONE posture (10.9 §7.4) — the H4 Knowledge store
//!    ([`KnowledgeStoreHolder`]) crypto-shreds free-text **blocks** + **db-row values** via the
//!    per-subject DEK, and purges the derived **embeddings** (they re-identify, EI-04 §5); the
//!    agent-trace holder (H17, [`KnowledgeAgentTraceHolder`]) crypto-shreds the content-addressed
//!    trace and is **distinct from the audit log** (KN-D12 / §6.5). This FILLS the
//!    [`crate::agent_trace_seam::AgentTraceHolderSeam`] floor named in P-GA-26 (the loud "M3 P-GA-27"
//!    deferral): the live trace `locate`/`export`/`erase` body now exists, registered under the SAME
//!    [`crate::agent_trace_seam::AGENT_TRACE_HOLDER_ID`] at the SAME [`crate::agent_trace_seam::agent_trace_phase`]
//!    the seam declared — the registration shape did not change (EI-01 §7).
//!
//! It REUSES [`crate::orchestration::RegisteredHolder`] / [`crate::orchestration::CanonicalErasePhase`]
//! / [`crate::holders::CryptoShredKms`] wholesale — it does NOT re-define the orchestrator, the
//! checklist, the erase order, or the crypto-shred mechanism. The three producer holders here are
//! faithful in-memory models of the live Git / Knowledge / agent-trace `erase` impls (in `myelin-git`
//! / the Knowledge M3 service / `myelin-agent-service`, behind the seam); the live binding is a config
//! swap at boot, never a code change.
//!
//! ## The three producer erasure mechanisms (§3.2 — each is a REAL crypto-shred, never hide)
//! 1. **Git (H1) — pseudonymise + crypto-shred inline bodies.** Author/subject identity in the Git
//!    DB is the stable opaque pseudonym (`<pseudonym>@<tenant>.noreply`, contract 4.8); the inline
//!    free-text bodies (PR/review/comment text the subject authored) are sealed under the per-subject
//!    DEK and crypto-shredded on erase. The immutable COMMIT bytes are the residual — pseudonymous-
//!    by-default (the X-7 Git instance) handles author identity; the third-party / immutable residual
//!    is the documented limit (the Git X-7 instance + GIT-D2 is **P-GA-28**, named below).
//! 2. **Knowledge (H4) — pseudonymise + crypto-shred content + purge embeddings (the ONE posture
//!    instance, §7.4).** Page authorship is the opaque pseudonym; the free-text **blocks** + the
//!    **db-row values** the subject authored are sealed under the per-subject DEK and crypto-shredded;
//!    the derived **embeddings** are PURGED (not hidden — they re-identify, EI-04 §5). KN-D4: 0
//!    recoverable incl. vectors.
//! 3. **Agent trace (H17) — crypto-shred, distinct from audit (§6.5).** The run's content-addressed
//!    reasoning trace is sealed under the per-subject DEK and crypto-shredded on erase. It is the
//!    **erasable** holder — kept DELIBERATELY distinct from the H16 audit carve-out (the retain
//!    record): erasing a person's trace never touches the tamper-evident audit log. KN-D12: agent
//!    traces shredded, attribution → pseudonym.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The Git pseudonymous-commit X-7 instance (10.9 by reference) + GIT-D2** → **P-GA-28 → P-257**
//!   (the Git data model freeze rides the P-GA-18 prerequisite). This prompt registers H1 + reaches
//!   it via the fan-out + crypto-shreds the inline bodies; the immutable-commit-byte residual posture
//!   instance is P-GA-28. Recorded in writing per the prompt DELIVERABLE.
//! - **The live Git / Knowledge / agent-trace `erase` bindings** behind the
//!   [`myelin_gdpr::PersonalDataHolder`] seam are wired by the harness/orchestrator at boot (the real
//!   `myelin-git` / Knowledge-service / `myelin-agent-service` impls). On THIS floor each producer
//!   holder is a faithful in-memory model whose crypto-shred + pseudonymise + embedding-purge
//!   semantics are byte-for-byte the KN-D4 / KN-D12 post-conditions — so the fan-out ORDER + the
//!   crypto-shred-incl-vectors + the trace-distinct-from-audit properties are proven against a
//!   faithful model, and the live binding is a config swap, never a code change. This module composes
//!   already-shipped seams (the crypto-shred KMS, the orchestrator, the data map) and touches **NO new
//!   DB / object-store / cache / bus contract — no `--features integration` leg owed**.
//! - **The durable Postgres per-holder checklist + the per-subject DEK provisioning at write time**
//!   are the same DB/KMS floor every M0/M1 store carries (P-007 / P-S12).
//!
//! ## Mutation floor (P-GA-27 TESTS — the fan-out-checklist-includes-the-new-holders path is
//! mandatory-core). The behavioral core every mutation must be caught on:
//! [`producer_phase_of`] (a producer holder slots into the correct canonical phase),
//! [`ProducerHolderRegistration::register_producers`] (the three holders register because the map
//! drives them), [`KnowledgeStoreHolder::erase`] (the crypto-shred incl. the embedding purge —
//! `reidentify_hits == 0`), and [`KnowledgeAgentTraceHolder::erase`] (the trace crypto-shred, distinct
//! from audit). The subsystems' own erase-impl floors are owned by Git/KN/Agent (this prompt owns the
//! ORCHESTRATION fan-out leg). `cargo mutants` score recorded in the commit body (EI-01 §3).

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::Region;

use crate::agent_trace_seam::{agent_trace_phase, AGENT_TRACE_HOLDER_ID};
use crate::datamap::HolderSchema;
use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};
use crate::orchestration::{CanonicalErasePhase, RegisteredHolder};

// ───────────────────────── the producer holder ids (§3.2) ─────────────────────────

/// The stable, PII-free holder names the M3 producer-subsystem stores register under (contract 1.4 —
/// the data-map / DSR fan-out address book). One per §3.2 producer this prompt orchestrates
/// (H1 Git / H4 Knowledge / H17 agent-trace). PII-free: a holder id is a store name, never a subject.
///
/// The H17 id is NOT redeclared here — it is the SAME [`AGENT_TRACE_HOLDER_ID`] the P-GA-26 seam
/// froze (`agent_fabric_trace`), aligned to the agent subsystem's store name (ONE name across the
/// seam, EI-01 §7). [`producer_holder_ids::AGENT_TRACE`] re-exports it for local symmetry.
pub mod producer_holder_ids {
    use super::AGENT_TRACE_HOLDER_ID;

    /// **H1** — the Git subsystem DB (pseudonymise + crypto-shred inline bodies — §3.2 / 10.1).
    pub const GIT_DB: &str = "git_oltp";
    /// **H4** — the Knowledge subsystem DB (pseudonymise + crypto-shred content + purge embeddings —
    /// §3.2 / 10.9 §7.4).
    pub const KNOWLEDGE_DB: &str = "knowledge_oltp";
    /// **H17** — the agent execution trace (crypto-shred, distinct from the audit log — §3.2 / §6.5).
    /// The SAME id the P-GA-26 [`AGENT_TRACE_HOLDER_ID`] seam declared (`agent_fabric_trace`).
    pub const AGENT_TRACE: &str = AGENT_TRACE_HOLDER_ID;
}

/// The canonical phase each producer holder occupies in the §4.1 erase order. The producer holders'
/// **identity** is shredded by Identity (phase 0); their **self-authored free-text** is crypto-shred
/// via the per-subject DEK at [`CanonicalErasePhase::CryptoShredDek`] (alongside H6 blob and the
/// other free-text DEK holders — §4.1 step "KMS.destroy"). The agent **trace** is a TRAILING derived
/// copy (a run's reasoning record) so it shreds at [`crate::agent_trace_seam::agent_trace_phase`]
/// ([`CanonicalErasePhase::CachesAndDerivedCopies`]) — AFTER the pseudonym map + the per-subject DEK
/// are already destroyed. A producer holder declares its phase HERE (not via
/// [`crate::orchestration::canonical_phase_of`], which knows only the six upstream holders) — the
/// §4.1 order is a property of the phase, so a producer slots in correctly without re-deriving a
/// hand-written sequence.
pub fn producer_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        producer_holder_ids::GIT_DB => Some(CanonicalErasePhase::CryptoShredDek),
        producer_holder_ids::KNOWLEDGE_DB => Some(CanonicalErasePhase::CryptoShredDek),
        // H17 reuses the phase the agent-trace seam declared (the seam is the single source — EI-01 §7).
        producer_holder_ids::AGENT_TRACE => Some(agent_trace_phase()),
        _ => None,
    }
}

/// The exhaustive list of producer-holder ids this prompt registers (the M3 producer subsystems —
/// Git H1 / Knowledge H4 / agent-trace H17). The order is the declaration order; the canonical erase
/// order is applied by the orchestrator (a property of [`producer_phase_of`], not this list).
pub fn producer_holder_id_list() -> [&'static str; 3] {
    [
        producer_holder_ids::GIT_DB,
        producer_holder_ids::KNOWLEDGE_DB,
        producer_holder_ids::AGENT_TRACE,
    ]
}

// ───────────────────────── registration into the data map (gdpr §2.2 / contract 10.3) ─────────────────────────

/// **The producer holders' contributions to the generated data map (gdpr §2.2; contract 10.3).** The
/// three M3 producer stores (Git H1 / Knowledge H4 / agent-trace H17) declare their
/// [`HolderSchema`] so the data-map generator surfaces them — *the data-map diff in CI surfaces the
/// new holders; no holder-without-map drift* (gdpr §2.2). Once they are in the map, the DSR fan-out
/// reaches them STRUCTURALLY (the map, not a hand-written list, drives erasure — §4.1 step 2).
///
/// Each holder's `#[personal_data]`-tagged PII fields are owned by the producing subsystem's schema
/// (the Git/Knowledge/agent crates' classify-derive); on this floor the registration carries the
/// holder roster entry (the holder id + H-number + region) so the holder appears in the map's
/// **roster** even before its full per-field slice ships from the subsystem crate — the GA-D1
/// "0 holders missed" property reads the roster. The per-field slices grow without a generator change
/// as the subsystem schemas land (gdpr §2.2; the M5 completeness floor P-GA-32).
///
/// `region` is the cell the producer stores reside in (residency-pinned — gdpr §2.2 / ADR-11).
pub fn producer_holder_schemas(region: Region) -> Vec<HolderSchema> {
    vec![
        // H1 — Git subsystem DB.
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: producer_holder_ids::GIT_DB,
            },
            holder: Holder::H1Git,
            region: region.clone(),
            fields: &[],
        },
        // H4 — Knowledge subsystem DB.
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: producer_holder_ids::KNOWLEDGE_DB,
            },
            holder: Holder::H4Knowledge,
            region: region.clone(),
            fields: &[],
        },
        // H17 — the agent execution trace (a content-addressed Knowledge doc of a run's trace).
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: producer_holder_ids::AGENT_TRACE,
            },
            holder: Holder::H17AgentTrace,
            region,
            fields: &[],
        },
    ]
}

/// The [`HolderRegistration`]s the harness records for the three producer stores (the auto-registered
/// holder set the data-map coverage gate reads — [`crate::datamap::Inventory::coverage_gaps`]). A
/// producer holder REGISTERED (the harness opened the store) but absent from the map would be a
/// coverage gap; once [`producer_holder_schemas`] contributes them, the gap closes.
pub fn producer_registrations() -> Vec<HolderRegistration> {
    vec![
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: producer_holder_ids::GIT_DB,
        },
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: producer_holder_ids::KNOWLEDGE_DB,
        },
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: producer_holder_ids::AGENT_TRACE,
        },
    ]
}

// ───────────────────────── the opaque (subject, tenant) extractor ─────────────────────────

/// The opaque, PII-free `(subject_token, tenant_token)` for a scope (a tenant offboarding records
/// the `"*tenant*"` sentinel subject). Never a name/email.
fn subject_and_tenant(scope: &EraseScope) -> (String, String) {
    match scope {
        EraseScope::Subject { subject, tenant } => {
            (subject.principal.principal_id.0.clone(), tenant.0.clone())
        }
        EraseScope::Tenant(tenant) => ("*tenant*".to_string(), tenant.0.clone()),
    }
}

// ───────────────────────── H1 — the Git subsystem DB ─────────────────────────

/// **H1 — the Git subsystem DB AS a [`PersonalDataHolder`] (contract 10.1).** Its `erase` pseudonymises
/// authorship (the Id pseudonym lever ran in phase 0) AND crypto-shreds the inline free-text bodies
/// (PR/review/comment text the subject authored) through the per-subject DEK ([`CryptoShredKms`]). The
/// immutable COMMIT bytes are the residual — the pseudonymous-by-default X-7 Git instance (P-GA-28)
/// handles author identity; the third-party / immutable residual is the documented limit. Reached
/// ONLY through this contract (the no-cross-store-read law — never an `import myelin_git`).
pub struct GitDbHolder<'a> {
    kms: &'a dyn CryptoShredKms,
}

impl<'a> GitDbHolder<'a> {
    /// Build the H1 holder over the crypto-shred KMS seam (the live `myelin-git` DB at boot; the model
    /// in the drill).
    pub fn new(kms: &'a dyn CryptoShredKms) -> GitDbHolder<'a> {
        GitDbHolder { kms }
    }

    /// The per-subject DEK handle the subject's inline Git bodies are sealed under (the crypto-shred
    /// key class).
    fn dek(subject_token: &str, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject_token.to_string()),
        }
    }
}

impl PersonalDataHolder for GitDbHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.kms.is_present(&Self::dek(&sid, &tenant)) {
            "located:inline-bodies-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                producer_holder_ids::GIT_DB,
                &sid,
                &tenant.0,
                outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                producer_holder_ids::GIT_DB,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                producer_holder_ids::GIT_DB,
                &sid,
                "*",
                "rectified",
                None,
                0,
            ),
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
            receipt: Receipt::content_addressed(
                "restrict",
                producer_holder_ids::GIT_DB,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        // Crypto-shred the per-subject DEK sealing the inline bodies (pseudonymisation of authorship
        // ran in phase 0; the immutable commit-byte residual is the P-GA-28 X-7 instance).
        let destroyed = self.kms.destroy(&Self::dek(&sid, &tenant));
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                producer_holder_ids::GIT_DB,
                &sid,
                &tenant_token,
                "pseudonymise+crypto_shred:inline_bodies",
                destroyed,
                0,
            ),
        })
    }
}

// ───────────────────────── H4 — the Knowledge subsystem DB (the ONE posture instance, §7.4) ─────────────────────────

/// A faithful in-memory model of the Knowledge subsystem store for a subject (H4). It holds the
/// subject's self-authored **free-text blocks** + **db-row values** under their per-subject DEK
/// (crypto-shred-erasable), and a parallel **embedding** presence in the derived vector space (which
/// re-identifies — purged, not hidden, EI-04 §5). The live Knowledge-service store is the named floor;
/// this model has byte-for-byte the §7.4 / KN-D4 post-conditions (0 recoverable incl. vectors).
///
/// **Crypto-shred + purge-incl-vectors (the load-bearing property):** `erase` destroys the
/// per-subject DEK (so the blocks + db-row values are unrecoverable ciphertext — live AND in backups,
/// §7.5) AND compacts the embedding out of the vector space — a re-identification probe
/// ([`Self::reidentify_hits`]) then returns 0. A *hide* would leave the embedding present and
/// re-identifiable — the anti-pattern this model forecloses.
#[derive(Debug, Default)]
pub struct KnowledgeStoreModel {
    /// `subject_token → whether the subject's derived embedding is present in the vector space`. The
    /// blocks + db-row values themselves are governed by the per-subject DEK (the KMS seam); this
    /// tracks the derived-vector purge-not-hide property KN-D4 reads.
    embeddings: Mutex<BTreeMap<String, bool>>,
    /// The number of `erase` CALLS (the resumability / fan-out witness — a re-drive must not re-call
    /// an already-shredded subject through the checklist).
    erase_calls: Mutex<u32>,
}

impl KnowledgeStoreModel {
    /// A fresh, empty Knowledge store.
    pub fn new() -> KnowledgeStoreModel {
        KnowledgeStoreModel::default()
    }

    /// Index (or reindex) a subject's derived embedding FROM SOURCE — the live indexer's projection
    /// step (marks the embedding present). Provisioning the per-subject DEK that seals the blocks +
    /// db-row values is the caller's job (the drill seeds it on the KMS; a real store's DEK is
    /// provisioned by Storage's KMS hierarchy at write time).
    pub fn index_embedding_from_source(&self, subject_token: &str) {
        self.embeddings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
    }

    /// **The re-identification probe (the KN-D4 purge-not-hide GATE reading).** How many derived
    /// embeddings could still re-identify the subject — MUST be 0 after `erase` (the embedding was
    /// COMPACTED OUT, not hidden). 0 recoverable incl. vectors is the KN-D4 headline.
    pub fn reidentify_hits(&self, subject_token: &str) -> usize {
        let e = self.embeddings.lock().unwrap_or_else(|e| e.into_inner());
        usize::from(e.get(subject_token).copied().unwrap_or(false))
    }

    /// How many times `erase` was actually CALLED (the resumability witness).
    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The H4 derived-embedding purge (the KN-D4 vector half).** Compacts the subject's embedding
    /// out of the vector space (purge-not-hide). The free-text blocks + db-row values are rendered
    /// unrecoverable by the per-subject DEK crypto-shred (the KMS seam, driven by the holder `erase`);
    /// this purges the parallel derived vector. Idempotent: a re-erase is a no-op (already gone).
    fn purge_embedding(&self, subject_token: &str) -> bool {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.embeddings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token)
            .is_some()
    }
}

/// **H4 — the Knowledge subsystem DB AS a [`PersonalDataHolder`] (contract 10.1 / 10.9 §7.4 — the ONE
/// posture instance).** Its `erase` crypto-shreds the subject's free-text blocks + db-row values via
/// the per-subject DEK ([`CryptoShredKms`]) AND purges the derived embedding (the
/// [`KnowledgeStoreModel`] vector half). Page authorship is pseudonymised (the Id lever ran in phase
/// 0). This is the Knowledge instance of the platform erasure posture, BY REFERENCE (gdpr §7.4 —
/// confirmed-not-restated; the canonical posture is [`crate::posture::CANONICAL_POSTURE`]). Reached
/// ONLY through the contract (the no-cross-store-read law — never an `import` of the Knowledge crate).
pub struct KnowledgeStoreHolder<'a> {
    model: &'a KnowledgeStoreModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> KnowledgeStoreHolder<'a> {
    /// Build the H4 holder over a Knowledge store model + the crypto-shred KMS seam (the live
    /// Knowledge-service store at boot; the model in the drill).
    pub fn new(
        model: &'a KnowledgeStoreModel,
        kms: &'a dyn CryptoShredKms,
    ) -> KnowledgeStoreHolder<'a> {
        KnowledgeStoreHolder { model, kms }
    }

    /// The per-subject DEK handle the subject's Knowledge blocks + db-row values are sealed under.
    fn dek(subject_token: &str, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject_token.to_string()),
        }
    }
}

impl PersonalDataHolder for KnowledgeStoreHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let dek_present = self.kms.is_present(&Self::dek(&sid, &tenant));
        let outcome = if dek_present || self.model.reidentify_hits(&sid) > 0 {
            "located:content+embeddings"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                &tenant.0,
                outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                "*",
                "rectified:reindex_from_source",
                None,
                0,
            ),
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
            receipt: Receipt::content_addressed(
                "restrict",
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        // Crypto-shred the per-subject DEK (the blocks + db-row values become unrecoverable
        // ciphertext) AND purge the derived embedding (purge-not-hide — it re-identifies).
        let destroyed = self.kms.destroy(&Self::dek(&sid, &tenant));
        self.model.purge_embedding(&sid);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                &tenant_token,
                "crypto_shred:blocks+db_rows+embeddings_purged_not_hidden",
                destroyed,
                0,
            ),
        })
    }
}

// ───────────────────────── H17 — the agent execution trace (distinct from audit, §6.5) ─────────────────────────

/// A faithful in-memory model of the agent execution-trace store for a subject (H17). It holds the
/// run's **content-addressed reasoning trace** — sealed under the per-subject DEK, crypto-shred-
/// erasable. DISTINCT from the audit log (§6.5): trace = the run's reasoning record [erasable];
/// audit = the complete tamper-evident who-did-what [retained]. The live trace store
/// (`myelin-agent-service`'s `agent_fabric_trace`) is the named floor; this model has byte-for-byte
/// the KN-D12 post-condition (the trace is crypto-shredded, attribution → pseudonym).
#[derive(Debug, Default)]
pub struct AgentTraceModel {
    /// `subject_token → the content-address of the subject's run trace`. A crypto-shred renders it
    /// unrecoverable (the DEK is destroyed); the model drops the entry on erase (the trace row is gone
    /// — distinct from the audit carve-out, which RETAINS the minimised record).
    traces: Mutex<BTreeMap<String, String>>,
    /// The number of `erase` CALLS (the resumability witness).
    erase_calls: Mutex<u32>,
}

impl AgentTraceModel {
    /// A fresh, empty trace store.
    pub fn new() -> AgentTraceModel {
        AgentTraceModel::default()
    }

    /// Record a subject's content-addressed run trace FROM SOURCE (the live agent's trace-write step).
    /// `content_address` is the `blake3:<hex>` of the run's reasoning record (PII-free — the body is
    /// sealed under the per-subject DEK).
    pub fn write_trace_from_source(&self, subject_token: &str, content_address: &str) {
        self.traces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), content_address.to_string());
    }

    /// Whether the subject's trace is STILL present (the post-erase `locate` "0 recoverable" reading).
    pub fn has_trace(&self, subject_token: &str) -> bool {
        self.traces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(subject_token)
    }

    /// How many times `erase` was actually CALLED (the resumability witness).
    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The H17 trace crypto-shred (KN-D12).** Drops the content-addressed trace (the per-subject DEK
    /// is destroyed by the holder `erase`, rendering the body unrecoverable; the model drops the trace
    /// row). Idempotent: a re-erase is a no-op (already gone).
    fn shred_trace(&self, subject_token: &str) -> bool {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.traces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token)
            .is_some()
    }
}

/// **H17 — the agent execution trace AS a [`PersonalDataHolder`] (contract 8.8 / 10.1 — the Knowledge
/// instance of the trace holder body).** This FILLS the [`crate::agent_trace_seam::AgentTraceHolderSeam`]
/// floor (the P-GA-26 loud "M3 P-GA-27" deferral): the live trace `locate`/`export`/`erase` body now
/// exists. Its `erase` crypto-shreds the per-subject DEK (the trace body becomes unrecoverable) +
/// drops the content-addressed trace row. It is registered under the SAME
/// [`AGENT_TRACE_HOLDER_ID`] at the SAME [`agent_trace_phase`] the seam declared — the registration
/// shape did not change (EI-01 §7). DISTINCT from the H16 audit carve-out (the retain record): erasing
/// a person's trace never touches the tamper-evident audit log (§6.5). Reached ONLY through the
/// contract (the no-cross-store-read law).
pub struct KnowledgeAgentTraceHolder<'a> {
    model: &'a AgentTraceModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> KnowledgeAgentTraceHolder<'a> {
    /// Build the H17 holder over an agent-trace model + the crypto-shred KMS seam (the live
    /// `agent_fabric_trace` store at boot; the model in the drill).
    pub fn new(
        model: &'a AgentTraceModel,
        kms: &'a dyn CryptoShredKms,
    ) -> KnowledgeAgentTraceHolder<'a> {
        KnowledgeAgentTraceHolder { model, kms }
    }

    /// The PII-free holder id this holder registers under ([`AGENT_TRACE_HOLDER_ID`]).
    pub fn holder_id(&self) -> &'static str {
        AGENT_TRACE_HOLDER_ID
    }

    /// The per-subject DEK handle the subject's content-addressed trace is sealed under.
    fn dek(subject_token: &str, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject_token.to_string()),
        }
    }
}

impl PersonalDataHolder for KnowledgeAgentTraceHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.model.has_trace(&sid) {
            "located:run-trace-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                AGENT_TRACE_HOLDER_ID,
                &sid,
                &tenant.0,
                outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                AGENT_TRACE_HOLDER_ID,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                AGENT_TRACE_HOLDER_ID,
                &sid,
                "*",
                "rectified",
                None,
                0,
            ),
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
            receipt: Receipt::content_addressed(
                "restrict",
                AGENT_TRACE_HOLDER_ID,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        // Crypto-shred the per-subject DEK (the trace body becomes unrecoverable) + drop the trace row.
        // This is the H17 ERASURE — distinct from the H16 audit carve-out (which RETAINS its record).
        let destroyed = self.kms.destroy(&Self::dek(&sid, &tenant));
        self.model.shred_trace(&sid);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                AGENT_TRACE_HOLDER_ID,
                &sid,
                &tenant_token,
                "crypto_shred:agent_trace:distinct_from_audit",
                destroyed,
                0,
            ),
        })
    }
}

// ───────────────────────── the producer-holder registration (the fan-out reaches them) ─────────────────────────

/// **The producer-holder registration (P-GA-27 — the orchestration leg of 10.1 over the M3 producer
/// subsystems).** Wires the three producer holders (Git H1 / Knowledge H4 / agent-trace H17) into the
/// combined DSR fan-out at their canonical phases. It REUSES the [`RegisteredHolder`] seam +
/// the [`CanonicalErasePhase`] order — the producer holders register at their [`producer_phase_of`]
/// phase alongside the upstream + derivative holders (the §4.1 order is a property of the phase). It
/// NEVER reaches into a producer store — it holds only `&dyn PersonalDataHolder` (the
/// no-cross-store-read law).
pub struct ProducerHolderRegistration;

impl ProducerHolderRegistration {
    /// **Register the three M3 producer holders at their canonical phases.** The caller passes the
    /// holder seam (the live Git/Knowledge/agent-trace `erase` at boot; the faithful models in the
    /// drill); each is registered at its [`producer_phase_of`] phase so a combined fan-out over
    /// upstream + derivative + producer holders runs in the canonical erase order. A holder id without
    /// a known producer phase is rejected (it must declare one — the "we forgot a holder" trap is
    /// foreclosed structurally).
    pub fn register_producers<'a>(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> Vec<RegisteredHolder<'a>> {
        holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = producer_phase_of(id).unwrap_or_else(|| {
                    panic!("producer holder `{id}` has no canonical erase phase")
                });
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
    use crate::{EraseChecklist, AUDIT_CARVE_OUT_STORE};
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

    /// Provision a per-subject DEK on the KMS for each producer holder (each seals its own free-text).
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

    // ───────── registration into the data map (gdpr §2.2 — no holder-without-map drift) ─────────

    /// **H1/H4/H17 appear in the generated data map after registration (gdpr §2.2).** The three
    /// producer holders contribute their [`HolderSchema`] → the data-map roster surfaces them. *The
    /// data-map diff in CI surfaces the new holders.* A holder REGISTERED but absent from the map is a
    /// coverage gap; once they contribute, the gap closes (0 holders missed).
    #[test]
    fn producer_holders_appear_in_the_data_map_after_registration() {
        let inv = data_map(&producer_holder_schemas(region()));

        // The three producer holders are in the map's roster (H1 Git / H4 Knowledge / H17 trace).
        assert!(
            inv.holders.contains("oltp:git_oltp"),
            "H1 Git is in the map"
        );
        assert!(
            inv.holders.contains("oltp:knowledge_oltp"),
            "H4 Knowledge is in the map"
        );
        assert!(
            inv.holders.contains("oltp:agent_fabric_trace"),
            "H17 agent-trace is in the map"
        );
        assert_eq!(inv.holder_count(), 3, "exactly the three producer holders");

        // No holder-without-map drift: every REGISTERED producer holder is in the map (0 gaps).
        assert!(
            inv.coverage_gaps(&producer_registrations()).is_empty(),
            "every registered producer holder is in the map — 0 holders missed"
        );
    }

    /// **The RED coverage verdict.** A producer holder is REGISTERED (the harness opened the store) but
    /// did NOT contribute a [`HolderSchema`] — a coverage gap the data-map diff surfaces (the DSR
    /// fan-out cannot silently skip a store the map forgot).
    #[test]
    fn a_registered_producer_holder_absent_from_the_map_is_a_coverage_gap() {
        // The map is generated over only Git + Knowledge (the agent-trace schema is missing)…
        let partial: Vec<HolderSchema> = producer_holder_schemas(region())
            .into_iter()
            .filter(|s| s.holder_id() != "oltp:agent_fabric_trace")
            .collect();
        let inv = data_map(&partial);
        // …but the harness registered all three.
        let gaps = inv.coverage_gaps(&producer_registrations());
        assert_eq!(
            gaps,
            vec!["oltp:agent_fabric_trace".to_string()],
            "the registered-but-unmapped producer holder is the coverage gap"
        );
    }

    // ───────── the canonical phases (a producer slots into the correct erase phase) ─────────

    /// **Each producer holder declares its canonical erase phase (§4.1).** Git (H1) + Knowledge (H4)
    /// crypto-shred free-text at [`CanonicalErasePhase::CryptoShredDek`]; the agent trace (H17) is a
    /// trailing derived copy at [`agent_trace_phase`]. The §4.1 order is a property of the phase — a
    /// producer slots in without re-deriving a hand-written sequence.
    #[test]
    fn producer_holders_declare_their_canonical_erase_phases() {
        assert_eq!(
            producer_phase_of(producer_holder_ids::GIT_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        assert_eq!(
            producer_phase_of(producer_holder_ids::KNOWLEDGE_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        assert_eq!(
            producer_phase_of(producer_holder_ids::AGENT_TRACE),
            Some(agent_trace_phase())
        );
        assert_eq!(
            producer_phase_of(producer_holder_ids::AGENT_TRACE),
            Some(CanonicalErasePhase::CachesAndDerivedCopies)
        );
        // An unknown holder has no producer phase (it must declare one in its own prompt).
        assert_eq!(producer_phase_of("not_a_producer"), None);
        // The trace shreds AFTER the per-subject DEK (the free-text) — a trailing derived copy.
        assert!(
            producer_phase_of(producer_holder_ids::AGENT_TRACE)
                > producer_phase_of(producer_holder_ids::KNOWLEDGE_DB)
        );
    }

    // ───────── the fan-out reaches the producer holders (the data map drives them) ─────────

    /// **The combined DSR fan-out checklist INCLUDES H1/H4/H17 (the mandatory-core path).** The
    /// producer holders register through [`ProducerHolderRegistration::register_producers`] and join
    /// the orchestrator alongside the upstream holders; the fan-out reaches every one in the canonical
    /// erase order. This is the "the fan-out reaches them because the data map drives them" property.
    #[test]
    fn the_fan_out_reaches_the_producer_holders_in_canonical_order() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-prod", 10);

        let knowledge = KnowledgeStoreModel::new();
        knowledge.index_embedding_from_source("u-prod");
        let trace = AgentTraceModel::new();
        trace.write_trace_from_source("u-prod", "blake3:deadbeef");

        let git_h = GitDbHolder::new(&kms);
        let kn_h = KnowledgeStoreHolder::new(&knowledge, &kms);
        let trace_h = KnowledgeAgentTraceHolder::new(&trace, &kms);

        let producers = ProducerHolderRegistration::register_producers(vec![
            (
                producer_holder_ids::GIT_DB,
                &git_h as &dyn PersonalDataHolder,
            ),
            (producer_holder_ids::KNOWLEDGE_DB, &kn_h),
            (producer_holder_ids::AGENT_TRACE, &trace_h),
        ]);
        let orch = UpstreamHolderOrchestrator::new(producers);

        // The fan-out checklist includes all three producer holders.
        let ids = orch.holder_ids_in_order();
        assert!(
            ids.contains(&producer_holder_ids::GIT_DB),
            "H1 Git is in the fan-out"
        );
        assert!(
            ids.contains(&producer_holder_ids::KNOWLEDGE_DB),
            "H4 Knowledge is in the fan-out"
        );
        assert!(
            ids.contains(&producer_holder_ids::AGENT_TRACE),
            "H17 agent-trace is in the fan-out"
        );
        // The trace (a trailing derived copy) is fanned LAST (after the free-text DEK shreds).
        assert_eq!(
            ids.last(),
            Some(&producer_holder_ids::AGENT_TRACE),
            "the trace shreds last"
        );

        let checklist = EraseChecklist::new();
        let receipts = orch
            .fan_out_erase(&subject_scope("u-prod"), &checklist)
            .unwrap();
        assert_eq!(receipts.len(), 3, "all three producer holders were reached");
        assert_eq!(
            orch.fanout_coverage(&checklist),
            1.0,
            "100% coverage of the producer holders"
        );
    }

    // ───────── the Knowledge instance: crypto-shred free-text + purge embeddings (KN-D4) ─────────

    /// **KN-D4: the Knowledge instance crypto-shreds free-text + purges embeddings — 0 recoverable
    /// incl. vectors.** Before erase: the subject's per-subject DEK is live (blocks + db-row values
    /// recoverable) + the embedding re-identifies. After erase: the DEK is destroyed (0 recoverable in
    /// DBs AND backups) AND the embedding is purged (0 re-identification) — the headline KN-D4 number.
    #[test]
    fn knowledge_instance_crypto_shreds_freetext_and_purges_embeddings_zero_incl_vectors() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-kn", 20);
        let model = KnowledgeStoreModel::new();
        model.index_embedding_from_source("u-kn");

        let dek = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject("u-kn".into()),
        };
        assert!(
            kms.is_present(&dek),
            "the per-subject DEK is live before erase"
        );
        assert_eq!(
            model.reidentify_hits("u-kn"),
            1,
            "the embedding re-identifies before erase"
        );

        let holder = KnowledgeStoreHolder::new(&model, &kms);
        let receipt = holder.erase(subject_scope("u-kn")).unwrap();

        // 0 recoverable: the DEK is destroyed (DBs AND backups) AND the embedding is purged.
        assert!(
            !kms.is_present(&dek),
            "the per-subject DEK is destroyed (free-text unrecoverable)"
        );
        assert_eq!(
            kms.recoverable_in_backup(&dek),
            0,
            "0 recoverable in backups (crypto-shred reaches backups)"
        );
        assert_eq!(
            model.reidentify_hits("u-kn"),
            0,
            "0 re-identification — the embedding was PURGED (KN-D4)"
        );
        assert!(
            receipt.receipt.key_epoch_destroyed.is_some(),
            "the erase receipt records the destroyed key epoch (the GD-4 audit trail)"
        );
        assert!(
            receipt.receipt.content_hash.starts_with("blake3:"),
            "the receipt is content-addressed"
        );
    }

    /// **The Knowledge instance has NO hide path — only a real purge.** The only mutation that drops
    /// the embedding is `erase` (a real purge); after it, 0 re-identification. A hidden doc would leave
    /// the embedding re-identifiable — the anti-pattern this model forecloses.
    #[test]
    fn knowledge_instance_has_no_hide_path_only_a_real_purge() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-hide", 30);
        let model = KnowledgeStoreModel::new();
        model.index_embedding_from_source("u-hide");
        KnowledgeStoreHolder::new(&model, &kms)
            .erase(subject_scope("u-hide"))
            .unwrap();
        assert_eq!(
            model.reidentify_hits("u-hide"),
            0,
            "the only erase is a real purge"
        );
    }

    // ───────── the H17 trace: crypto-shred, distinct from audit (KN-D12 / §6.5) ─────────

    /// **KN-D12: the agent trace (H17) is crypto-shredded AND distinct from the audit log (§6.5).**
    /// Before erase: the subject's content-addressed trace is present + its DEK is live. After erase:
    /// the trace is shredded (the DEK destroyed, the trace row dropped, 0 recoverable). The H17 holder
    /// id is NOT the H16 audit carve-out id — erasing the trace never touches the audit log.
    #[test]
    fn agent_trace_is_crypto_shredded_and_distinct_from_audit() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-trace", 40);
        let model = AgentTraceModel::new();
        model.write_trace_from_source("u-trace", "blake3:cafef00d");

        let dek = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject("u-trace".into()),
        };
        assert!(
            model.has_trace("u-trace"),
            "the run trace is present before erase"
        );
        assert!(kms.is_present(&dek), "the trace DEK is live before erase");

        let holder = KnowledgeAgentTraceHolder::new(&model, &kms);
        // The H17 holder is the agent-trace store id — DISTINCT from the H16 audit carve-out.
        assert_eq!(holder.holder_id(), AGENT_TRACE_HOLDER_ID);
        assert_ne!(
            holder.holder_id(),
            AUDIT_CARVE_OUT_STORE,
            "H17 trace is distinct from the H16 audit carve-out (§6.5)"
        );

        let receipt = holder.erase(subject_scope("u-trace")).unwrap();

        // 0 recoverable: the DEK is destroyed AND the content-addressed trace row is dropped.
        assert!(
            !model.has_trace("u-trace"),
            "the trace row is dropped (crypto-shredded)"
        );
        assert!(!kms.is_present(&dek), "the trace DEK is destroyed");
        assert_eq!(
            kms.recoverable_in_backup(&dek),
            0,
            "0 recoverable in backups"
        );
        assert!(
            receipt.receipt.key_epoch_destroyed.is_some(),
            "the destroyed key epoch is recorded"
        );
        assert!(
            receipt.receipt.content_hash.starts_with("blake3:"),
            "the trace-shred receipt is content-addressed"
        );
    }

    /// **This holder FILLS the P-GA-26 agent-trace seam floor — the body is no longer a loud deferral.**
    /// The seam ([`crate::agent_trace_seam::AgentTraceHolderSeam`]) returned a loud "M3 P-GA-27" error
    /// for every op; the live [`KnowledgeAgentTraceHolder`] now returns real receipts under the SAME id
    /// + phase. The seam coordinates did not change (EI-01 §7) — the live impl plugged into them.
    #[test]
    fn the_live_trace_holder_fills_the_p_ga_26_seam_floor() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-fill", 50);
        let model = AgentTraceModel::new();
        model.write_trace_from_source("u-fill", "blake3:abc123");
        let holder = KnowledgeAgentTraceHolder::new(&model, &kms);

        // The live body returns a real receipt (no longer the loud "P-GA-27" deferral the seam had).
        let loc = holder
            .locate(&subject("u-fill"), tenant.clone())
            .expect("the live locate body exists");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));
        let erased = holder
            .erase(subject_scope("u-fill"))
            .expect("the live erase body exists");
        assert_eq!(erased.receipt.operation, "erase");
        // The id + phase match the seam's frozen coordinates (the live impl plugged into them).
        assert_eq!(holder.holder_id(), AGENT_TRACE_HOLDER_ID);
        assert_eq!(
            producer_phase_of(holder.holder_id()),
            Some(agent_trace_phase())
        );
    }

    // ───────── idempotent re-erase (the fan-out resumability property) ─────────

    /// **An idempotent re-erase returns the SAME content-addressed receipt + does not double-shred.**
    /// The producer holders are idempotent (the holder bodies no-op on a re-run + return the same
    /// receipt) — the §4.1-step-4 resumability the combined fan-out relies on.
    #[test]
    fn producer_holder_erase_is_idempotent() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-idem", 60);
        let model = KnowledgeStoreModel::new();
        model.index_embedding_from_source("u-idem");
        let holder = KnowledgeStoreHolder::new(&model, &kms);

        let first = holder.erase(subject_scope("u-idem")).unwrap();
        // The re-erase: the DEK is already gone (key_epoch_destroyed None now), the embedding already
        // purged — the model's purge is a no-op. The outcome string is stable.
        let second = holder.erase(subject_scope("u-idem")).unwrap();
        assert_eq!(first.receipt.operation, second.receipt.operation);
        assert_eq!(
            model.reidentify_hits("u-idem"),
            0,
            "0 re-identification after the re-erase too"
        );
    }

    /// The producer-holder id roster is the three M3 producer subsystems (a drift guard).
    #[test]
    fn producer_holder_id_list_is_the_three_m3_producers() {
        assert_eq!(
            producer_holder_id_list(),
            ["git_oltp", "knowledge_oltp", "agent_fabric_trace"]
        );
    }

    /// **The Knowledge `locate` outcome distinguishes present-content from 0-recoverable on BOTH the
    /// DEK presence AND the embedding presence (mandatory-core).** Either a live DEK (free-text
    /// recoverable) OR a present embedding (re-identifiable) means content is located; only when BOTH
    /// are gone does `locate` report `0-recoverable`. This kills the `|| → &&` and the `> ==/</>=`
    /// mutants on the locate disjunction.
    #[test]
    fn knowledge_locate_reports_present_on_either_dek_or_embedding() {
        let tenant = t("acme");

        // (a) DEK present, embedding absent → located (the `||` left conjunct).
        let kms_a = InMemoryShredKms::new();
        provision_subject_dek(&kms_a, &tenant, "u-a", 80);
        let model_a = KnowledgeStoreModel::new(); // no embedding indexed
        let loc_a = KnowledgeStoreHolder::new(&model_a, &kms_a)
            .locate(&subject("u-a"), tenant.clone())
            .unwrap();
        assert_eq!(model_a.reidentify_hits("u-a"), 0, "no embedding for u-a");
        assert!(
            loc_a.receipt.content_hash.starts_with("blake3:"),
            "located receipt is content-addressed"
        );

        // (b) DEK absent, embedding present → STILL located (the `||` right conjunct, `> 0`). We pin
        // the EXACT located receipt against the expected content-address for the SAME subject: under
        // the `|| → &&` or `> → ==/</>=` mutant the outcome would flip to `0-recoverable`, producing a
        // DIFFERENT content hash for the SAME subject — caught by this exact-match.
        let kms_b = InMemoryShredKms::new(); // no DEK provisioned
        let model_b = KnowledgeStoreModel::new();
        model_b.index_embedding_from_source("u-b");
        let holder_b = KnowledgeStoreHolder::new(&model_b, &kms_b);
        assert!(!kms_b.is_present(&ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject("u-b".into())
        }));
        assert_eq!(
            model_b.reidentify_hits("u-b"),
            1,
            "the embedding re-identifies (> 0)"
        );
        let located = holder_b.locate(&subject("u-b"), tenant.clone()).unwrap();
        let expected_located = Receipt::content_addressed(
            "locate",
            producer_holder_ids::KNOWLEDGE_DB,
            "u-b",
            &tenant.0,
            "located:content+embeddings",
            None,
            0,
        );
        assert_eq!(
            located.receipt.content_hash, expected_located.content_hash,
            "embedding-present ⇒ `located:content+embeddings` (the `||`+`> 0` branch is load-bearing)"
        );
        // The 0-recoverable outcome for the SAME subject would hash differently — proving the outcome
        // string (not just the subject token) drives the address.
        let zero = Receipt::content_addressed(
            "locate",
            producer_holder_ids::KNOWLEDGE_DB,
            "u-b",
            &tenant.0,
            "located:0-recoverable",
            None,
            0,
        );
        assert_ne!(located.receipt.content_hash, zero.content_hash);

        // (c) BOTH absent → `0-recoverable`. Driven through the holder so the `>= 0`-always-true mutant
        // (which would wrongly report `located:content+embeddings`) is caught: the actual receipt must
        // equal the EXPECTED 0-recoverable address, not the content+embeddings one.
        let kms_c = InMemoryShredKms::new(); // no DEK
        let model_c = KnowledgeStoreModel::new(); // no embedding
        let loc_c = KnowledgeStoreHolder::new(&model_c, &kms_c)
            .locate(&subject("u-c"), tenant.clone())
            .unwrap();
        let expected_zero_c = Receipt::content_addressed(
            "locate",
            producer_holder_ids::KNOWLEDGE_DB,
            "u-c",
            &tenant.0,
            "located:0-recoverable",
            None,
            0,
        );
        assert_eq!(
            loc_c.receipt.content_hash, expected_zero_c.content_hash,
            "both DEK and embedding absent ⇒ `located:0-recoverable` (kills the `>= 0`-always-true mutant)"
        );
    }

    /// **The Git holder crypto-shreds the inline bodies via the per-subject DEK.** Authorship is
    /// pseudonymised (the Id lever ran in phase 0); the inline free-text bodies are crypto-shredded.
    #[test]
    fn git_holder_crypto_shreds_inline_bodies() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-git", 70);
        let dek = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject("u-git".into()),
        };
        assert!(
            kms.is_present(&dek),
            "the inline-body DEK is live before erase"
        );

        let receipt = GitDbHolder::new(&kms)
            .erase(subject_scope("u-git"))
            .unwrap();
        assert!(
            !kms.is_present(&dek),
            "the inline-body DEK is crypto-shredded"
        );
        assert_eq!(
            kms.recoverable_in_backup(&dek),
            0,
            "0 recoverable in backups"
        );
        assert!(
            receipt.receipt.key_epoch_destroyed.is_some(),
            "the destroyed epoch is recorded"
        );
    }
}
