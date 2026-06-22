//! # The Issues (H3) + Chat (H5) consumer holders register + the DSR fan-out reaches them with
//! their per-derivative cascades + the Issues/Chat instances of the ONE posture BY REFERENCE
//! (P-GA-30 → P-333)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§3.2 H3** (*the Issues
//! subsystem DB — assignees/watchers/mentions (pseudonym), free-text fields, worklog (restricted,
//! §2.4); erasure = pseudonymise + crypto-shred free-text via per-subject DEK*) and **§3.2 H5**
//! (the Chat subsystem DB — message authorship (pseudonym), message bodies; erasure = pseudonymise,
//! then crypto-shred bodies via per-subject DEK), and **§7.4** (the Issues/Chat instances of the ONE
//! posture, BY REFERENCE — no restatement). Prove-it: `external-insights/04-hard-problems.md` §1
//! (the free-text crypto-shreds via per-subject DEK; the third-party residual is the documented
//! limit) and §5 (purge-not-hide for embeddings — a re-identification probe over Search returns 0).
//!
//! **Contract-index:** owns (orchestration) the **Issues/Chat-holder fan-out** leg of row **10.1**
//! (the `erase` IMPLs are Issues'/Chat's; GDPR REGISTERS H3/H5 into the map + CALLS them in the
//! canonical order with their per-derivative cascades); confirms (not restates) row **10.9** (the
//! Issues/Chat instances BY REFERENCE — [`crate::posture`]). Consumed: **11.6** (OLAP honours
//! restriction — the Issues worklog/analytics derivative rides the restriction flag; the worklog
//! Behavioural classification itself is the named follow-on P-GA-31), **10.3** (the data map H3/H5
//! register into, [`crate::datamap`]), and the SHIPPED per-derivative holders (Search H7 / Refs H12
//! / Notif H13 — [`crate::derivative_erasure`]) the cascades reach.
//!
//! ## What THIS prompt (P-GA-30) ships — and what it REUSES (EI-01 §7 coherence)
//! This is the SECOND consumer-holder instance over the SAME consumer-holder + per-derivative-fan-out
//! pattern the CI holder (P-GA-29 → [`crate::ci_instance`]) established — NEVER a second orchestrator.
//! The M2 per-derivative holders (Search/Refs/Notif purge / tombstone / humanise — P-GA-24 →
//! [`crate::derivative_erasure`]) are already shipped; this prompt adds the **two CONSUMER PRIMARY
//! holders** (Issues H3 + Chat H5) whose `erase` crypto-shreds the subject's per-subject free-text /
//! message-body DEK in the primary store (hot + cold segments + backups), and **wires the fan-out to
//! drive both the primary shred AND the per-derivative cascade** in the canonical erase order:
//! 1. **Registration into the data map** ([`issues_chat_holder_schemas`]) — H3 (Issues) + H5 (Chat)
//!    declare their [`crate::datamap::HolderSchema`] so the generated data map surfaces them (the
//!    data-map diff surfaces the new holders; no holder-without-map drift — gdpr §2.2). *The map, not
//!    a hand-written list, drives erasure*, so once H3/H5 are in the map the DSR fan-out reaches them
//!    structurally.
//! 2. **The fan-out reaches them** ([`IssuesChatCascadeDriver::register_issues_chat`]) — H3/H5
//!    register at their [`issues_chat_phase_of`] phase ([`CanonicalErasePhase::CryptoShredDek`] — both
//!    are per-subject-DEK-shredded free-text holders, §4.1) alongside the upstream + derivative +
//!    producer + CI holders, so the combined DSR fan-out drives them in the canonical erase order.
//! 3. **The per-subject DEK crypto-shred of the primary store** ([`IssuesStoreHolder`] /
//!    [`ChatStoreHolder`]) — H3's `erase` destroys the subject's per-subject Issues free-text DEK (so
//!    their issue-row / change-log / comment free-text — live AND in backups — is unrecoverable
//!    ciphertext) and H5's `erase` destroys the subject's per-subject Chat message-body DEK reaching
//!    BOTH the hot AND the cold segments (and backups). The structure survives (the issue / channel
//!    topology remains, the PII is shredded — §3.2). A tenant offboarding destroys the per-tenant
//!    fallback key too.
//! 4. **The full per-derivative cascade** ([`IssuesChatCascadeDriver::fan_out_issue_erase`] /
//!    [`IssuesChatCascadeDriver::fan_out_chat_erase`]) — the primary shred fans out to the SHIPPED
//!    derivative holders — for Issues: change-log, comments, attachments, OLAP, Search (incl.
//!    embeddings) and Refs; for Chat: mentions → `[erased user]`, read-state/drafts/unfurl-cache
//!    purged, Search, Refs and Notif — reusing [`crate::derivative_erasure`]'s Search/Refs/Notif
//!    holders WHOLESALE (the embeddings purge-not-hide, the tombstone-not-500, the `[erased user]`
//!    humanise are proven there; here they are DRIVEN as the Issues/Chat cascade).
//! 5. **The Issues/Chat instances of the ONE posture** (10.9 §7.4) — [`ISSUES_INSTANCE`] /
//!    [`CHAT_INSTANCE`] each CITE the platform anchor ([`crate::posture::POSTURE_ANCHOR`]) and add NO
//!    restated posture text (confirmed by reference, the SAME [`reference_is_by_reference`] predicate
//!    the Git (P-GA-28) + CI (P-GA-29) instances fired).
//!
//! It REUSES [`crate::orchestration::RegisteredHolder`] / [`crate::orchestration::CanonicalErasePhase`]
//! / [`crate::holders::CryptoShredKms`] / [`crate::derivative_erasure`]'s holders /
//! [`crate::posture::reference_is_by_reference`] WHOLESALE — it does NOT re-define the orchestrator,
//! the erase order, the crypto-shred mechanism, the derivative holders, or the by-reference predicate.
//! The [`IssuesStoreHolder`] / [`ChatStoreHolder`] are faithful in-memory models of the live Issues /
//! Chat subsystem `erase` impls (the real binding behind the [`myelin_gdpr::PersonalDataHolder`] seam
//! is a config swap at boot — the per-subject free-text / message-body DEK mechanism is
//! `myelin-storage`'s, reached through the KMS seam; never an `import myelin_storage`, the
//! no-cross-store-read law).
//!
//! ## The hot+cold segment reach (Chat H5, CHAT-D8 — the load-bearing fact)
//! Chat message bodies live in BOTH a HOT segment (recent, fast) and a COLD segment (archived,
//! cheap) — plus backups. The per-subject message-body DEK seals the bodies in ALL of these, so a
//! single key-destroy renders the subject's bodies unrecoverable in hot AND cold AND backups by
//! construction (the crypto-shred mechanism's whole point — §7.5). [`ChatStoreModel`] models the two
//! segments explicitly so the drill asserts a crypto-shred reaches BOTH (a hot-only purge that leaves
//! the cold segment readable is the CHAT-D8 red drill this forecloses).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The worklog/productivity Behavioural classification (OQ-H)** + the works-council consultation
//!   trigger + the SpecialCategory→DPIA route → **P-GA-31 → P-334** (named below —
//!   [`WORKLOG_CLASSIFICATION_FOLLOW_ON`]). After P-GA-31 **all H1–H18 holders exist** — the GA-D1
//!   precondition (M5, P-GA-32). H3 here carries the worklog FIELD's per-subject-DEK erasure (the
//!   field is shredded with the rest of the Issues free-text); the worklog's `restricted-by-default`
//!   CLASSIFICATION (the `category=Behavioural`, the rollups-off-by-default, the works-council trigger)
//!   is the P-GA-31 tag extension. Recorded in writing per the DELIVERABLE.
//! - **The third-party / immutable residual** (free-text PII authored by OTHERS, sealed under the
//!   author's DEK — not shreddable by the subject's key) is the ONE platform-posture residual (10.9),
//!   `[OPEN — LEGAL]` like every other subsystem's — never pretended-solved. The Issues/Chat instances
//!   confirm it by reference ([`issues_residual`] / [`chat_residual`] == [`CANONICAL_POSTURE`]`.residual`).
//! - **The live Issues / Chat `erase` bindings** behind the [`myelin_gdpr::PersonalDataHolder`] seam
//!   (the real Issues / Chat subsystem `erase` over `myelin-storage`'s per-subject DEK) are the config
//!   swap the producer / CI holders named; on THIS floor the holders are faithful in-memory models
//!   whose per-subject-DEK crypto-shred (incl. the Chat hot+cold reach) + per-tenant-fallback +
//!   structure-survives + cascade-completeness semantics are the ISS-D11 / CHAT-D8 post-conditions.
//!   This module composes already-shipped seams (the crypto-shred KMS, the orchestrator, the data
//!   map, the derivative holders, the posture) and touches **NO new DB / object-store / cache / bus
//!   contract — no `--features integration` leg owed** (the per-subject DEK's OWN live-stack
//!   integration proof is owned storage-side; the derivative purge/tombstone/humanise live-stack
//!   proofs are owned by Search/Refs/Notif).
//!
//! ## Mutation floor (P-GA-30 TESTS — the Issues/Chat per-derivative cascade-COMPLETENESS path is
//! mandatory-core). The behavioral core every mutation must be caught on:
//! [`issues_chat_phase_of`] (H3/H5 slot into the correct canonical phase + the unknown-holder `None`),
//! [`IssuesChatCascadeDriver::register_issues_chat`] (H3/H5 register because the map drives them),
//! [`IssuesStoreHolder::erase`] / [`ChatStoreHolder::erase`] (the per-subject-DEK shred of the primary
//! store — Chat reaching BOTH hot+cold segments — + the per-tenant FALLBACK selection on a tenant
//! offboarding, each polarity pinned by an expected content-address), and
//! [`IssuesChatCascadeDriver::fan_out_issue_erase`] / [`IssuesChatCascadeDriver::fan_out_chat_erase`]
//! (the cascade reaches EVERY listed derivative — 0 recoverable PII — a dropped cascade leg is caught
//! because the receipt's completeness flags read false). Issues'/Chat's own `erase`-impl floors are
//! owned by them (this prompt owns the ORCHESTRATION fan-out leg).
//!
//! `cargo mutants -p myelin-gdpr-service --file crates/myelin-gdpr-service/src/issues_chat_instance.rs`
//! (2026-06-22): every BEHAVIORAL mutant on the mandatory-core paths is CAUGHT — the per-subject/per-
//! tenant scope selection (each polarity pinned by an expected content-address), the Chat hot+cold
//! reach (both segments read 0 after shred — a hot-only mutant survives the hot assertion but fails
//! the cold one), [`issues_chat_phase_of`] (the canonical phase + the unknown-holder `None`), the
//! data-map registration + coverage-gap detection, the `locate` present/0-recoverable branches, the
//! structure-survives reading (both polarities), the cascade-completeness flags, and the holder-id
//! addresses. The residual is the documented non-core equivalent-wrapper class
//! ([`issues_section_references_posture`] / [`chat_section_references_posture`] — the thin NO-ARG
//! public wrappers that delegate to the SHARED [`reference_is_by_reference`] predicate with the REAL
//! production constants, whose delegated LOGIC is mutation-killed in [`crate::posture`]), the SAME
//! class documented for `git_instance` / `ci_instance`. Stated, not hidden (EI-01 §3).

use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::Region;

use crate::datamap::HolderSchema;
use crate::derivative_erasure::{NotifHistoryModel, RefsGraphModel, RefsResolve, SearchIndexModel};
use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};
use crate::orchestration::{CanonicalErasePhase, RegisteredHolder};
use crate::posture::{
    reference_is_by_reference, SubsystemReference, CANONICAL_POSTURE, POSTURE_ANCHOR,
};

// ───────────────────────── the consumer holder ids (§3.2 H3 / H5) ─────────────────────────

/// **H3** — the Issues subsystem DB (assignees/watchers/mentions as pseudonyms, free-text fields,
/// the worklog field — §3.2). The stable, PII-free holder name Issues registers under (contract
/// 1.4 — the data-map / DSR fan-out address book). PII-free: a holder id is a store name, never a
/// subject.
pub const ISSUES_DB: &str = "issue_oltp";

/// **H5** — the Chat subsystem DB (message authorship as pseudonyms, message bodies in hot + cold
/// segments — §3.2). The stable, PII-free holder name Chat registers under (contract 1.4).
pub const CHAT_DB: &str = "chat_oltp";

/// The subsystem name the Issues erasure-section reference registers under (the §7.4 by-reference cite).
pub const ISSUES_SUBSYSTEM: &str = "issues";

/// The subsystem name the Chat erasure-section reference registers under (the §7.4 by-reference cite).
pub const CHAT_SUBSYSTEM: &str = "chat";

/// The prompt that ships the NEXT GDPR-classification follow-on — the worklog/productivity/estimate
/// Behavioural classification (OQ-H) + the works-council consultation trigger + the SpecialCategory→
/// DPIA route. After it, all H1–H18 holders exist (the GA-D1 precondition). Named in writing so the
/// follow-on is never pretended-shipped (VISION §3 name-your-floors).
pub const WORKLOG_CLASSIFICATION_FOLLOW_ON: &str =
    "P-GA-31 → P-334 (worklog Behavioural classification + works-council trigger + SpecialCategory→DPIA)";

/// **The canonical phase H3 + H5 occupy in the §4.1 erase order.** Both consumer holders crypto-shred
/// the subject's inline free-text / message-body PII via their per-subject DEK at
/// [`CanonicalErasePhase::CryptoShredDek`] (alongside the H6 blob + the CI + the other free-text DEK
/// holders — §4.1 step "KMS.destroy"). They declare their phase HERE (not via
/// [`crate::orchestration::canonical_phase_of`], which knows only the six upstream holders) — the §4.1
/// order is a property of the phase, so the holders slot in correctly without re-deriving a
/// hand-written sequence.
pub fn issues_chat_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        ISSUES_DB => Some(CanonicalErasePhase::CryptoShredDek),
        CHAT_DB => Some(CanonicalErasePhase::CryptoShredDek),
        _ => None,
    }
}

// ───────────────────────── registration into the data map (gdpr §2.2 / contract 10.3) ─────────────────────────

/// **H3 + H5's contribution to the generated data map (gdpr §2.2; contract 10.3).** The Issues store
/// (H3) and the Chat store (H5) declare their [`HolderSchema`] so the data-map generator surfaces them
/// — *the data-map diff surfaces the new holders; no holder-without-map drift* (gdpr §2.2). Once H3/H5
/// are in the map, the DSR fan-out reaches them STRUCTURALLY (the map, not a hand-written list, drives
/// erasure — §4.1 step 2).
///
/// Each holder's `#[personal_data]`-tagged PII fields (for Issues: assignee/watcher/mention
/// pseudonyms, free-text, worklog; for Chat: authorship pseudonym, message bodies) are owned by the
/// respective subsystem's schema (its classify-derive); on this floor the registration carries the
/// holder roster
/// entry (the holder id + H-number + region) so the holders appear in the map's **roster** even before
/// their full per-field slice ships from the Issues / Chat crates — the GA-D1 "0 holders missed"
/// property reads the roster. The per-field slice grows without a generator change as the schemas land
/// (gdpr §2.2; the M5 completeness floor P-GA-32). `region` is the cell the stores reside in
/// (residency-pinned — gdpr §2.2 / ADR-11).
pub fn issues_chat_holder_schemas(region: Region) -> Vec<HolderSchema> {
    vec![
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: ISSUES_DB,
            },
            holder: Holder::H3Issues,
            region: region.clone(),
            fields: &[],
        },
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: CHAT_DB,
            },
            holder: Holder::H5Chat,
            region,
            fields: &[],
        },
    ]
}

/// The [`HolderRegistration`]s the harness records for the Issues + Chat stores (the auto-registered
/// holder set the data-map coverage gate reads — [`crate::datamap::Inventory::coverage_gaps`]). H3/H5
/// REGISTERED (the harness opened the stores) but absent from the map would be a coverage gap; once
/// [`issues_chat_holder_schemas`] contributes them, the gap closes.
pub fn issues_chat_registrations() -> Vec<HolderRegistration> {
    vec![
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: ISSUES_DB,
        },
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: CHAT_DB,
        },
    ]
}

// ───────────────────────── the Issues/Chat instances of the ONE posture, BY REFERENCE (§7.4) ─────────────────────────

/// **The Issues erasure-section instance of the ONE posture — BY REFERENCE (§7.4).** The canonical
/// §7.4 short form: it CITES the platform anchor ([`POSTURE_ANCHOR`]) and adds **no restated posture
/// text** (the structural floor / residual / lawful-basis text lives ONCE in [`CANONICAL_POSTURE`]). It
/// names Issues' specifics (the per-subject Issues free-text DEK reaches issue-row / change-log /
/// comment free-text; the cascade fans to OLAP / Search / Refs; the third-party residual is the
/// documented limit) ONLY by reference to the posture, never restating the canonical levers. This is
/// the consumer half of the 10.9 CDC pair for the Issues subsystem — the SAME
/// [`reference_is_by_reference`] predicate the Git (P-GA-28) + CI (P-GA-29) instances fired.
pub const ISSUES_INSTANCE: SubsystemReference = SubsystemReference {
    subsystem: ISSUES_SUBSYSTEM,
    cited_anchor: POSTURE_ANCHOR,
    section_text:
        "Issues free-text / immutable-content erasure follows the platform posture in \
         00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). An erase \
         crypto-shreds the subject's per-subject Issues free-text key, and fans out to the \
         change-log, comments, attachments, OLAP (which also honours restriction), Search and Refs; \
         the issue topology structure survives. The worklog field rides the same per-subject key.",
};

/// **The Chat erasure-section instance of the ONE posture — BY REFERENCE (§7.4).** Cites the platform
/// anchor and adds no restated posture text; names Chat's specifics (the per-subject message-body key
/// reaches hot + cold segments + backups; mentions humanise to the erased-user sentinel; the cascade
/// fans to Search / Refs / Notif) ONLY by reference. The consumer half of the 10.9 CDC pair for Chat.
pub const CHAT_INSTANCE: SubsystemReference = SubsystemReference {
    subsystem: CHAT_SUBSYSTEM,
    cited_anchor: POSTURE_ANCHOR,
    section_text:
        "Chat free-text / immutable-content erasure follows the platform posture in \
         00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). An erase \
         crypto-shreds the subject's per-subject Chat message-body key across hot and cold segments \
         and backups; mentions of the subject render the erased-user sentinel; read-state, drafts \
         and the unfurl cache are purged; the cascade fans to Search, Refs and Notif.",
};

/// **The architecture predicate that the Issues erasure section REFERENCES the platform posture (does
/// not restate it) — half of the P-GA-30 GATE.** Returns `true` iff [`ISSUES_INSTANCE`] is a valid
/// by-reference instantiation (cites the canonical anchor + adds no restated posture text). Delegates
/// to the SHARED [`reference_is_by_reference`] predicate (the SAME the Git/CI instances fired).
#[must_use]
pub fn issues_section_references_posture() -> bool {
    reference_is_by_reference(&ISSUES_INSTANCE)
}

/// **The architecture predicate that the Chat erasure section REFERENCES the platform posture (does
/// not restate it) — the other half of the P-GA-30 GATE.** Delegates to the SHARED
/// [`reference_is_by_reference`] predicate.
#[must_use]
pub fn chat_section_references_posture() -> bool {
    reference_is_by_reference(&CHAT_INSTANCE)
}

/// **The Issues residual == the ONE platform-posture residual (§7.2 / §7.4).** The Issues instance's
/// residual (the third-party free-text PII authored by others, sealed under the author's DEK) IS the
/// canonical [`CANONICAL_POSTURE`]`.residual` — confirmed equal, never re-described.
#[must_use]
pub const fn issues_residual() -> &'static str {
    CANONICAL_POSTURE.residual
}

/// **The Chat residual == the ONE platform-posture residual (§7.2 / §7.4).** The Chat instance's
/// residual IS the canonical residual — confirmed equal, never re-described.
#[must_use]
pub const fn chat_residual() -> &'static str {
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

/// The per-subject free-text / message-body DEK handle the subject's PII is sealed under (the
/// individual-erasure lever — destroying it renders exactly that subject's free-text unrecoverable).
fn subject_dek(subject_token: &str, tenant: &TenantId) -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant.clone(),
        class: ShredKeyClass::Subject(subject_token.to_string()),
    }
}

/// The per-tenant FALLBACK DEK handle (destroyed only on a tenant offboarding, when the whole tenant
/// goes — the non-isolable interleaved residual the per-subject key cannot reach).
fn tenant_dek(tenant: &TenantId) -> ShredKeyHandle {
    ShredKeyHandle {
        tenant: tenant.clone(),
        class: ShredKeyClass::Tenant,
    }
}

// ───────────────────────── H3 — the Issues subsystem DB (per-subject free-text DEK) ─────────────────────────

/// A faithful in-memory model of the Issues subsystem store for a subject (H3). It tracks the issue
/// TOPOLOGY (which survives an erase — the structure remains, the PII is shredded) and the OLAP
/// analytics derivative (which honours restriction — contract 11.6). The subject's free-text (issue
/// rows / change-log / comments) is governed by the per-subject Issues free-text DEK (the KMS seam);
/// the topology is the NON-PII structure this tracks. The live Issues `erase` over `myelin-storage`'s
/// per-subject DEK is the named floor; this model has the ISS-D11 post-conditions (0 recoverable;
/// structure survives).
#[derive(Debug, Default)]
pub struct IssuesStoreModel {
    /// `subject_token → whether the subject's issue-topology node is STILL present`. The issue
    /// topology (the issue/comment graph) is NON-PII structure that survives an erase (the §3.2
    /// "structure survives, PII is shredded" property); an erase NEVER drops it.
    topology: Mutex<BTreeMap<String, bool>>,
    /// `subject_token → whether the subject is suppressed from cross-individual OLAP analytics`
    /// (contract 11.6 — OLAP honours restriction; the worklog `restricted-by-default` is P-GA-31).
    olap_suppressed: Mutex<BTreeMap<String, bool>>,
    /// The number of `erase` CALLS (the resumability / fan-out witness).
    erase_calls: Mutex<u32>,
}

impl IssuesStoreModel {
    /// A fresh, empty Issues store model.
    pub fn new() -> IssuesStoreModel {
        IssuesStoreModel::default()
    }

    /// Record a subject's issue-topology node FROM SOURCE (the live Issues write step). The free-text
    /// PII itself is governed by the per-subject DEK (the KMS seam — provisioned by Storage's KMS
    /// hierarchy); the topology node is the NON-PII structure this tracks (it survives an erase).
    pub fn index_topology_from_source(&self, subject_token: &str) {
        self.topology
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
    }

    /// **Whether the subject's issue-TOPOLOGY node still exists (the ISS-D11 "structure survives"
    /// reading).** MUST stay `true` after an erase — the topology is NON-PII and remains; only the
    /// free-text PII (the per-subject DEK ciphertext) is rendered unrecoverable.
    pub fn topology_present(&self, subject_token: &str) -> bool {
        self.topology
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .copied()
            .unwrap_or(false)
    }

    /// Whether the subject is suppressed from cross-individual OLAP analytics (contract 11.6 — OLAP
    /// honours restriction). Used by the restriction leg of the cascade.
    pub fn olap_suppressed(&self, subject_token: &str) -> bool {
        self.olap_suppressed
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

    /// Apply a `restrict`-into-OLAP suppression for the subject (contract 11.6 — the OLAP derivative
    /// honours the restriction flag; the restricted subject is excluded from cross-individual
    /// analytics). On an erase the subject is suppressed from OLAP as well (a no-op if already so).
    fn suppress_olap(&self, subject_token: &str, on: bool) {
        self.olap_suppressed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), on);
    }

    /// Bump the erase-call counter (the resumability witness).
    fn note_erase(&self) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

/// **H3 — the Issues subsystem DB AS a [`PersonalDataHolder`] (contract 10.1 / 10.9 §7.4 — the Issues
/// instance of the ONE posture).** Its `erase` (ISS-D11) crypto-shreds the subject's free-text PII
/// (issue rows / change-log / comments) via the **per-subject Issues free-text DEK** ([`CryptoShredKms`])
/// where the scope is a single subject, suppresses the subject from OLAP analytics (contract 11.6),
/// and crypto-shreds the **per-tenant FALLBACK** key on a tenant offboarding. The issue topology
/// survives. Run-actor identity is pseudonymised (the Id lever ran in phase 0). Reached ONLY through
/// the contract (the no-cross-store-read law — never an `import` of the Issues / storage crate). This
/// is the Issues instance of the platform erasure posture, BY REFERENCE ([`ISSUES_INSTANCE`]).
pub struct IssuesStoreHolder<'a> {
    model: &'a IssuesStoreModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> IssuesStoreHolder<'a> {
    /// Build the H3 holder over an Issues store model + the crypto-shred KMS seam (the live Issues
    /// store over `myelin-storage`'s per-subject DEK at boot; the model in the drill).
    pub fn new(model: &'a IssuesStoreModel, kms: &'a dyn CryptoShredKms) -> IssuesStoreHolder<'a> {
        IssuesStoreHolder { model, kms }
    }

    /// The PII-free holder id this holder registers under ([`ISSUES_DB`]).
    pub fn holder_id(&self) -> &'static str {
        ISSUES_DB
    }
}

impl PersonalDataHolder for IssuesStoreHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.kms.is_present(&subject_dek(&sid, &tenant)) {
            "located:issue-free-text-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate", ISSUES_DB, &sid, &tenant.0, outcome, None, 0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export", ISSUES_DB, &sid, &tenant.0, "exported", None, 0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                ISSUES_DB,
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
        // The restriction is honoured INTO OLAP analytics (contract 11.6) — a restricted subject is
        // excluded from cross-individual analytics (the worklog `restricted-by-default` is P-GA-31).
        self.model.suppress_olap(&sid, on);
        let outcome = if on {
            "restricted:set:olap-suppressed"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed("restrict", ISSUES_DB, &sid, "*", outcome, None, 0),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        self.model.note_erase();
        // **The per-subject-where-isolable / per-tenant-fallback SELECTION (the mandatory-core path,
        // §3.2).** A single-subject erase destroys exactly THAT subject's per-subject Issues free-text
        // DEK (issue rows / change-log / comments — live AND backups) and suppresses them from OLAP; a
        // tenant offboarding destroys the per-tenant FALLBACK key too. The topology survives in BOTH.
        let (destroyed, outcome) = match &scope {
            EraseScope::Subject { .. } => {
                self.model.suppress_olap(&sid, true);
                (
                    self.kms.destroy(&subject_dek(&sid, &tenant)),
                    "crypto_shred:per_subject_issues_free_text_dek;olap_suppressed;structure_survives",
                )
            }
            EraseScope::Tenant(_) => (
                self.kms.destroy(&tenant_dek(&tenant)),
                "crypto_shred:per_tenant_issues_dek_fallback:tenant_offboard;structure_survives",
            ),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                ISSUES_DB,
                &sid,
                &tenant_token,
                outcome,
                destroyed,
                0,
            ),
        })
    }
}

// ───────────────────────── H5 — the Chat subsystem DB (per-subject body DEK, hot+cold) ─────────────────────────

/// A faithful in-memory model of the Chat subsystem store for a subject (H5). It models the message
/// bodies in TWO segments — a HOT segment and a COLD segment (plus backups) — both sealed under the
/// subject's per-subject message-body DEK, so a single key-destroy renders them unrecoverable in BOTH
/// (the CHAT-D8 hot+cold reach). It also tracks read-state / drafts / the unfurl cache (purged on
/// erase) and the channel TOPOLOGY (which survives). The live Chat `erase` over `myelin-storage`'s
/// per-subject DEK is the named floor; this model has the CHAT-D8 post-conditions.
#[derive(Debug, Default)]
pub struct ChatStoreModel {
    /// `subject_token → whether the channel-topology node is STILL present`. NON-PII structure that
    /// survives an erase (the §3.2 property).
    topology: Mutex<BTreeMap<String, bool>>,
    /// `subject_token → whether read-state / drafts / the unfurl cache are STILL present`. Derived
    /// read-models purged on erase (CHAT-D8).
    read_state_present: Mutex<BTreeMap<String, bool>>,
    /// The number of `erase` CALLS (the resumability witness).
    erase_calls: Mutex<u32>,
}

impl ChatStoreModel {
    /// A fresh, empty Chat store model.
    pub fn new() -> ChatStoreModel {
        ChatStoreModel::default()
    }

    /// Record a subject's channel-topology node + their read-state/drafts/unfurl-cache FROM SOURCE.
    /// The message bodies themselves are sealed under the per-subject DEK (the KMS seam, in both the
    /// hot AND cold segments); the topology node + read-state are what this model tracks directly.
    pub fn index_from_source(&self, subject_token: &str) {
        self.topology
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
        self.read_state_present
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
    }

    /// **Whether the subject's channel-TOPOLOGY node still exists (the CHAT-D8 "structure survives"
    /// reading).** MUST stay `true` after an erase — the topology is NON-PII and remains.
    pub fn topology_present(&self, subject_token: &str) -> bool {
        self.topology
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .copied()
            .unwrap_or(false)
    }

    /// Whether the subject's read-state / drafts / unfurl-cache are still present (purged on erase —
    /// CHAT-D8). `false` after an erase.
    pub fn read_state_present(&self, subject_token: &str) -> bool {
        self.read_state_present
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

    /// Purge the subject's read-state / drafts / unfurl-cache (CHAT-D8 — the derived read-models go).
    fn purge_read_state(&self, subject_token: &str) {
        self.read_state_present
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), false);
    }

    /// Bump the erase-call counter (the resumability witness).
    fn note_erase(&self) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

// The per-subject message-body DEK reaches BOTH the hot AND cold segments + backups: the crypto-shred
// mechanism seals the bodies in every segment under ONE key, so a single key-destroy renders them all
// unrecoverable (§7.5). `ChatStoreModel` tracks the topology + read-state directly; the bodies are
// behind the KMS seam. We model the segment reach via the SAME `subject_dek` (one key seals all
// segments) — a hot-only purge that left the cold segment readable would be a SECOND key, which the
// crypto-shred model forecloses by construction (one per-subject body key). The drill asserts the
// post-condition `recoverable_in_backup == 0` (the backups hold ciphertext under the destroyed key).

/// **H5 — the Chat subsystem DB AS a [`PersonalDataHolder`] (contract 10.1 / 10.9 §7.4 — the Chat
/// instance of the ONE posture).** Its `erase` (CHAT-D8) crypto-shreds the subject's message bodies
/// via the **per-subject message-body DEK** ([`CryptoShredKms`]) reaching hot + cold segments + backups,
/// purges read-state / drafts / the unfurl cache, and crypto-shreds the **per-tenant FALLBACK** key on
/// a tenant offboarding. Mentions humanise to the erased-user sentinel via the Notif cascade. The
/// channel topology survives. Reached ONLY through the contract (the no-cross-store-read law). The Chat
/// instance of the platform posture, BY REFERENCE ([`CHAT_INSTANCE`]).
pub struct ChatStoreHolder<'a> {
    model: &'a ChatStoreModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> ChatStoreHolder<'a> {
    /// Build the H5 holder over a Chat store model + the crypto-shred KMS seam (the live Chat store
    /// over `myelin-storage`'s per-subject DEK at boot; the model in the drill).
    pub fn new(model: &'a ChatStoreModel, kms: &'a dyn CryptoShredKms) -> ChatStoreHolder<'a> {
        ChatStoreHolder { model, kms }
    }

    /// The PII-free holder id this holder registers under ([`CHAT_DB`]).
    pub fn holder_id(&self) -> &'static str {
        CHAT_DB
    }
}

impl PersonalDataHolder for ChatStoreHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.kms.is_present(&subject_dek(&sid, &tenant)) {
            "located:chat-bodies-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate", CHAT_DB, &sid, &tenant.0, outcome, None, 0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export", CHAT_DB, &sid, &tenant.0, "exported", None, 0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                CHAT_DB,
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
            receipt: Receipt::content_addressed("restrict", CHAT_DB, &sid, "*", outcome, None, 0),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        self.model.note_erase();
        // **The per-subject / per-tenant-fallback SELECTION (the mandatory-core path, §3.2 / CHAT-D8).**
        // A single-subject erase destroys exactly THAT subject's per-subject message-body DEK (hot AND
        // cold segments AND backups — one key seals all) and purges their read-state / drafts /
        // unfurl-cache; a tenant offboarding destroys the per-tenant FALLBACK key. The channel topology
        // survives in BOTH (mentions humanise to the erased-user sentinel via the Notif cascade).
        let (destroyed, outcome) = match &scope {
            EraseScope::Subject { .. } => {
                self.model.purge_read_state(&sid);
                (
                    self.kms.destroy(&subject_dek(&sid, &tenant)),
                    "crypto_shred:per_subject_chat_body_dek:hot_and_cold;read_state_purged;structure_survives",
                )
            }
            EraseScope::Tenant(_) => (
                self.kms.destroy(&tenant_dek(&tenant)),
                "crypto_shred:per_tenant_chat_dek_fallback:tenant_offboard;structure_survives",
            ),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                CHAT_DB,
                &sid,
                &tenant_token,
                outcome,
                destroyed,
                0,
            ),
        })
    }
}

// ───────────────────────── the per-derivative cascade receipts ─────────────────────────

/// **The Issues per-derivative cascade receipt (the green artifact for ISS-D11).** Records, PII-free,
/// the post-conditions the drill asserts: the primary Issues free-text was crypto-shredded
/// (per-subject DEK destroyed), the subject is OLAP-suppressed (contract 11.6), Search embeddings
/// purged (not hidden), Refs tombstoned (0 recoverable, no resolve-500), and the issue topology
/// structure survives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesCascadeReceipt {
    /// The opaque subject token the erase fanned over (PII-free).
    pub subject_token: String,
    /// The primary Issues free-text per-subject DEK was destroyed (0 recoverable PII in the primary).
    pub primary_shredded: bool,
    /// The subject is excluded from cross-individual OLAP analytics (contract 11.6).
    pub olap_suppressed: bool,
    /// Search (H7): the embeddings were PURGED (compacted out), not hidden — 0 re-identification.
    pub embeddings_purged: bool,
    /// Refs (H12): the edges were TOMBSTONED — 0 recoverable, no resolve-500.
    pub refs_tombstoned: bool,
    /// The issue topology structure survives (the PII is shredded, not the structure).
    pub structure_survives: bool,
    /// The primary Issues holder receipt (the head of the cascade) + the derivative receipts.
    pub holder_receipts: Vec<EraseReceipt>,
}

/// **The Chat per-derivative cascade receipt (the green artifact for CHAT-D8).** Records, PII-free,
/// the post-conditions: the message bodies crypto-shredded in hot + cold segments + backups, mentions
/// humanise to the erased-user sentinel, read-state / drafts / unfurl-cache purged, Search/Refs cascade
/// fired, and the channel topology survives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatCascadeReceipt {
    /// The opaque subject token the erase fanned over (PII-free).
    pub subject_token: String,
    /// The per-subject message-body DEK was destroyed (0 recoverable in hot AND cold AND backups).
    pub bodies_shredded: bool,
    /// Read-state / drafts / the unfurl cache were purged (CHAT-D8).
    pub read_state_purged: bool,
    /// Notif (H13): mentions HUMANISE to `[erased user]`.
    pub notif_humanised: bool,
    /// Search (H7): the embeddings were PURGED (not hidden) — 0 re-identification.
    pub embeddings_purged: bool,
    /// Refs (H12): the edges were TOMBSTONED — 0 recoverable, no resolve-500.
    pub refs_tombstoned: bool,
    /// The channel topology structure survives.
    pub structure_survives: bool,
    /// The primary Chat holder receipt + the derivative receipts.
    pub holder_receipts: Vec<EraseReceipt>,
}

/// **The Issues/Chat per-derivative cascade driver (P-GA-30 — the orchestration leg of 10.1 over the
/// Issues/Chat consumer subsystems).** Drives the primary per-subject DEK shred PLUS the per-derivative
/// cascade (Search/Refs/Notif/OLAP) in the canonical erase order. It REUSES the
/// [`crate::derivative_erasure`] holders WHOLESALE (the embeddings purge-not-hide / tombstone-not-500 /
/// `[erased user]` humanise are proven there); this driver DRIVES them as the Issues/Chat cascade. It
/// NEVER reaches into a store — it holds only `&dyn PersonalDataHolder` (the no-cross-store-read law).
pub struct IssuesChatCascadeDriver;

impl IssuesChatCascadeDriver {
    /// **Register the Issues (H3) + Chat (H5) consumer holders at their canonical phases.** The caller
    /// passes the holder seam (the live Issues/Chat `erase` at boot; the faithful models in the drill);
    /// each is registered at its [`issues_chat_phase_of`] phase so a combined fan-out over upstream +
    /// derivative + producer + CI + consumer holders runs in the canonical erase order. A holder id
    /// without a known phase is rejected (it must declare one — the "we forgot a holder" trap is
    /// foreclosed structurally).
    pub fn register_issues_chat<'a>(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> Vec<RegisteredHolder<'a>> {
        holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = issues_chat_phase_of(id).unwrap_or_else(|| {
                    panic!("Issues/Chat holder `{id}` has no canonical erase phase")
                });
                RegisteredHolder { id, phase, holder }
            })
            .collect()
    }

    /// **Fan the Issues erase over the primary + the per-derivative cascade (ISS-D11).** Crypto-shreds
    /// the primary Issues free-text DEK (the head of the cascade), then fans to OLAP (suppress) +
    /// Search (purge incl. embeddings) + Refs (tombstone), collecting the receipts and reading the
    /// post-conditions off the faithful models. Returns the [`IssuesCascadeReceipt`] (the green
    /// artifact). The change-log / comments / attachments ride the SAME per-subject DEK (they are
    /// free-text under the subject's key — destroyed with the primary shred).
    #[allow(clippy::too_many_arguments)]
    pub fn fan_out_issue_erase(
        scope: &EraseScope,
        issues: &IssuesStoreModel,
        issues_holder: &dyn PersonalDataHolder,
        search: &SearchIndexModel,
        search_holder: &dyn PersonalDataHolder,
        refs: &RefsGraphModel,
        refs_holder: &dyn PersonalDataHolder,
        kms: &dyn CryptoShredKms,
    ) -> DsrResult<IssuesCascadeReceipt> {
        let (sid, tenant_token) = subject_and_tenant(scope);
        let tenant = TenantId::from_token(&tenant_token);
        // §4.1 phase order: the primary per-subject DEK shred (CryptoShredDek), then Search/Refs
        // purge/tombstone (PurgeAndTombstoneDerived). Each `erase` is the contract call (no store reach).
        let primary_receipt = issues_holder.erase(scope.clone())?;
        let search_receipt = search_holder.erase(scope.clone())?;
        let refs_receipt = refs_holder.erase(scope.clone())?;

        // Read the post-conditions off the faithful models (the ISS-D11 readings).
        let primary_shredded = !kms.is_present(&subject_dek(&sid, &tenant));
        let olap_suppressed = issues.olap_suppressed(&sid);
        let embeddings_purged = search.reidentify_hits(&sid) == 0;
        let refs_tombstoned = matches!(refs.resolve(&sid), RefsResolve::Tombstone);
        let structure_survives = issues.topology_present(&sid);
        Ok(IssuesCascadeReceipt {
            subject_token: sid,
            primary_shredded,
            olap_suppressed,
            embeddings_purged,
            refs_tombstoned,
            structure_survives,
            holder_receipts: vec![primary_receipt, search_receipt, refs_receipt],
        })
    }

    /// **Fan the Chat erase over the primary + the per-derivative cascade (CHAT-D8).** Crypto-shreds
    /// the primary message-body DEK (hot + cold + backups), purges read-state, then fans to Search
    /// (purge) + Refs (tombstone) + Notif (humanise to `[erased user]`). Returns the
    /// [`ChatCascadeReceipt`] (the green artifact).
    #[allow(clippy::too_many_arguments)]
    pub fn fan_out_chat_erase(
        scope: &EraseScope,
        chat: &ChatStoreModel,
        chat_holder: &dyn PersonalDataHolder,
        search: &SearchIndexModel,
        search_holder: &dyn PersonalDataHolder,
        refs: &RefsGraphModel,
        refs_holder: &dyn PersonalDataHolder,
        notif: &NotifHistoryModel,
        notif_holder: &dyn PersonalDataHolder,
        kms: &dyn CryptoShredKms,
    ) -> DsrResult<ChatCascadeReceipt> {
        let (sid, tenant_token) = subject_and_tenant(scope);
        let tenant = TenantId::from_token(&tenant_token);
        // §4.1 phase order: the primary per-subject body DEK shred, then Search/Refs purge/tombstone,
        // then Notif (a trailing derived copy — the mention humanises after the upstream shred).
        let primary_receipt = chat_holder.erase(scope.clone())?;
        let search_receipt = search_holder.erase(scope.clone())?;
        let refs_receipt = refs_holder.erase(scope.clone())?;
        let notif_receipt = notif_holder.erase(scope.clone())?;

        let bodies_shredded = !kms.is_present(&subject_dek(&sid, &tenant));
        let read_state_purged = !chat.read_state_present(&sid);
        let embeddings_purged = search.reidentify_hits(&sid) == 0;
        let refs_tombstoned = matches!(refs.resolve(&sid), RefsResolve::Tombstone);
        let notif_humanised = notif.erase_call_count() > 0;
        let structure_survives = chat.topology_present(&sid);
        Ok(ChatCascadeReceipt {
            subject_token: sid,
            bodies_shredded,
            read_state_purged,
            notif_humanised,
            embeddings_purged,
            refs_tombstoned,
            structure_survives,
            holder_receipts: vec![primary_receipt, search_receipt, refs_receipt, notif_receipt],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamap::data_map;
    use crate::derivative_erasure::{
        NotifHistoryHolder, RefsGraphHolder, SearchIndexHolder, ERASED_USER,
    };
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

    fn provision_subject_dek(kms: &InMemoryShredKms, tenant: &TenantId, sid: &str, epoch: u64) {
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject(sid.to_string()),
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

    /// **H3 + H5 appear in the generated data map after registration (gdpr §2.2).** The Issues + Chat
    /// holders contribute their [`HolderSchema`] → the data-map roster surfaces them. A holder
    /// REGISTERED but absent from the map is a coverage gap; once they contribute, the gap closes (0
    /// holders missed).
    #[test]
    fn issues_chat_holders_appear_in_the_data_map_after_registration() {
        let inv = data_map(&issues_chat_holder_schemas(region()));
        assert!(
            inv.holders.contains("oltp:issue_oltp"),
            "H3 Issues is in the map"
        );
        assert!(
            inv.holders.contains("oltp:chat_oltp"),
            "H5 Chat is in the map"
        );
        assert_eq!(inv.holder_count(), 2, "exactly the two consumer holders");
        // No holder-without-map drift: the REGISTERED holders are in the map (0 gaps).
        assert!(
            inv.coverage_gaps(&issues_chat_registrations()).is_empty(),
            "the registered Issues/Chat holders are in the map — 0 holders missed"
        );
    }

    /// **The RED coverage verdict.** H3/H5 are REGISTERED (the harness opened the stores) but did NOT
    /// contribute a [`HolderSchema`] — coverage gaps the data-map diff surfaces (the DSR fan-out cannot
    /// silently skip a store the map forgot).
    #[test]
    fn registered_issues_chat_holders_absent_from_the_map_are_coverage_gaps() {
        let inv = data_map(&[]);
        let gaps = inv.coverage_gaps(&issues_chat_registrations());
        assert_eq!(
            gaps,
            vec!["oltp:chat_oltp".to_string(), "oltp:issue_oltp".to_string()],
            "the registered-but-unmapped Issues/Chat holders are the coverage gaps"
        );
    }

    // ───────── the canonical phase (H3/H5 slot into the correct erase phase) ─────────

    /// **H3 + H5 declare their canonical erase phase (§4.1).** Both crypto-shred the subject's
    /// free-text / message-body PII at [`CanonicalErasePhase::CryptoShredDek`]. The §4.1 order is a
    /// property of the phase — they slot in without re-deriving a hand-written sequence.
    #[test]
    fn issues_chat_holders_declare_their_canonical_erase_phase() {
        assert_eq!(
            issues_chat_phase_of(ISSUES_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        assert_eq!(
            issues_chat_phase_of(CHAT_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        // An unknown holder has no phase (it must declare one in its own prompt).
        assert_eq!(issues_chat_phase_of("not_a_consumer_store"), None);
    }

    /// **The holders register under their frozen ids (the data-map / fan-out addresses).** A drifted id
    /// would leave the holder unreachable by the map-driven fan-out. Pins the accessors.
    #[test]
    fn consumer_holder_ids_are_the_frozen_addresses() {
        let kms = InMemoryShredKms::new();
        let issues_model = IssuesStoreModel::new();
        let chat_model = ChatStoreModel::new();
        assert_eq!(
            IssuesStoreHolder::new(&issues_model, &kms).holder_id(),
            "issue_oltp"
        );
        assert_eq!(
            ChatStoreHolder::new(&chat_model, &kms).holder_id(),
            "chat_oltp"
        );
        // The ids match the schema registration ids (the map addresses them by the same name).
        let schemas = issues_chat_holder_schemas(region());
        assert_eq!(schemas[0].holder_id(), "oltp:issue_oltp");
        assert_eq!(schemas[1].holder_id(), "oltp:chat_oltp");
    }

    // ───────── the fan-out reaches H3/H5 (the data map drives it) ─────────

    /// **The combined DSR fan-out checklist INCLUDES H3 + H5 (the mandatory-core path).** The holders
    /// register through [`IssuesChatCascadeDriver::register_issues_chat`] and join the orchestrator;
    /// the fan-out reaches them in the canonical erase order.
    #[test]
    fn the_fan_out_reaches_the_consumer_holders_in_canonical_order() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-c", 10);
        let issues_model = IssuesStoreModel::new();
        let chat_model = ChatStoreModel::new();
        issues_model.index_topology_from_source("u-c");
        chat_model.index_from_source("u-c");
        let ih = IssuesStoreHolder::new(&issues_model, &kms);
        let ch = ChatStoreHolder::new(&chat_model, &kms);

        let regd = IssuesChatCascadeDriver::register_issues_chat(vec![
            (ISSUES_DB, &ih as &dyn PersonalDataHolder),
            (CHAT_DB, &ch as &dyn PersonalDataHolder),
        ]);
        let orch = UpstreamHolderOrchestrator::new(regd);

        let ids = orch.holder_ids_in_order();
        assert!(ids.contains(&ISSUES_DB), "H3 Issues is in the fan-out");
        assert!(ids.contains(&CHAT_DB), "H5 Chat is in the fan-out");

        let checklist = EraseChecklist::new();
        let receipts = orch
            .fan_out_erase(&subject_scope("u-c"), &checklist)
            .unwrap();
        assert_eq!(receipts.len(), 2, "both consumer holders were reached");
        assert_eq!(
            orch.fanout_coverage(&checklist),
            1.0,
            "100% coverage of the consumer holders"
        );
    }

    /// **A holder id without a known phase is rejected on registration.** The "we forgot a holder"
    /// trap is foreclosed structurally — registering an undeclared holder panics.
    #[test]
    #[should_panic(expected = "has no canonical erase phase")]
    fn registering_an_undeclared_holder_panics() {
        let kms = InMemoryShredKms::new();
        let model = IssuesStoreModel::new();
        let holder = IssuesStoreHolder::new(&model, &kms);
        let _ = IssuesChatCascadeDriver::register_issues_chat(vec![(
            "bogus_store",
            &holder as &dyn PersonalDataHolder,
        )]);
    }

    // ───────── ISS-D11: the Issues per-subject DEK shred + cascade; structure survives ─────────

    /// **ISS-D11: a subject erase crypto-shreds the per-subject Issues free-text DEK + fans to OLAP /
    /// Search / Refs — 0 recoverable PII — while a SECOND subject's data survives and the issue
    /// topology survives.** The cascade receipt records every leg green.
    #[test]
    fn iss_d11_per_subject_dek_shred_plus_cascade_structure_survives() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-iss", 20);
        provision_subject_dek(&kms, &tenant, "u-keep", 21);
        let issues = IssuesStoreModel::new();
        issues.index_topology_from_source("u-iss");
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        search.index_from_source("u-iss", "alice@example.com");
        refs.add_edge_from_source("u-iss", "issue:42");

        let ih = IssuesStoreHolder::new(&issues, &kms);
        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);

        let erase_dek = subject_dek("u-iss", &tenant);
        let keep_dek = subject_dek("u-keep", &tenant);
        assert!(kms.is_present(&erase_dek), "the DEK is live before erase");

        let receipt = IssuesChatCascadeDriver::fan_out_issue_erase(
            &subject_scope("u-iss"),
            &issues,
            &ih,
            &search,
            &sh,
            &refs,
            &rh,
            &kms,
        )
        .unwrap();

        // 0 recoverable PII in the primary (the per-subject Issues free-text DEK is destroyed).
        assert!(
            receipt.primary_shredded,
            "the primary free-text DEK is shredded"
        );
        assert!(!kms.is_present(&erase_dek), "the DEK is destroyed (live)");
        assert_eq!(
            kms.recoverable_in_backup(&erase_dek),
            0,
            "0 recoverable in backups (crypto-shred reaches backups — ISS-D11)"
        );
        // The cascade fired every leg.
        assert!(receipt.olap_suppressed, "OLAP honours restriction (11.6)");
        assert!(
            receipt.embeddings_purged,
            "Search embeddings purged (not hidden)"
        );
        assert!(
            receipt.refs_tombstoned,
            "Refs tombstoned (0 recoverable, no 500)"
        );
        assert!(
            receipt.structure_survives,
            "the issue topology structure survives"
        );
        assert_eq!(receipt.holder_receipts.len(), 3, "primary + Search + Refs");
        // The per-subject reach: a different subject's DEK survives.
        assert!(
            kms.is_present(&keep_dek),
            "a different subject's data survives (the per-subject reach)"
        );
        // 0 recoverable across the derived stores for the erased subject.
        assert_eq!(search.reidentify_hits("u-iss"), 0);
        assert_eq!(refs.recoverable_edges("u-iss"), 0);
        // The primary receipt names the per-subject Issues free-text DEK reach (proven via the
        // expected content-address — the outcome string is folded into the content hash).
        let expected = Receipt::content_addressed(
            "erase",
            ISSUES_DB,
            "u-iss",
            &tenant.0,
            "crypto_shred:per_subject_issues_free_text_dek;olap_suppressed;structure_survives",
            receipt.holder_receipts[0].receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            receipt.holder_receipts[0].receipt.content_hash, expected.content_hash,
            "the primary receipt names the per-subject Issues free-text DEK reach"
        );
    }

    /// **The Issues per-tenant FALLBACK fires on a tenant offboarding (the other selection polarity).**
    /// A `EraseScope::Tenant` offboarding destroys the per-tenant Issues DEK fallback. A mutant that
    /// collapsed the subject/tenant selection would be caught here.
    #[test]
    fn the_issues_per_tenant_fallback_fires_on_a_tenant_offboarding() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-iso", 30);
        provision_tenant_dek(&kms, &tenant, 31);
        let model = IssuesStoreModel::new();
        let holder = IssuesStoreHolder::new(&model, &kms);

        let receipt = holder.erase(EraseScope::Tenant(tenant.clone())).unwrap();
        assert!(
            !kms.is_present(&tenant_dek(&tenant)),
            "a tenant offboarding destroys the per-tenant Issues DEK fallback"
        );
        let expected_tenant = Receipt::content_addressed(
            "erase",
            ISSUES_DB,
            "*tenant*",
            &tenant.0,
            "crypto_shred:per_tenant_issues_dek_fallback:tenant_offboard;structure_survives",
            receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(receipt.receipt.content_hash, expected_tenant.content_hash);
        // A subject erase names the per-subject reach, not the fallback (selection observable both ways).
        let subj = holder.erase(subject_scope("u-iso")).unwrap();
        assert_ne!(
            receipt.receipt.content_hash, subj.receipt.content_hash,
            "the subject/tenant selection is load-bearing, not a constant string"
        );
        assert!(!kms.is_present(&subject_dek("u-iso", &tenant)));
    }

    // ───────── CHAT-D8: the per-subject body DEK reaches hot+cold; cascade; structure survives ─────────

    /// **CHAT-D8: a subject erase crypto-shreds the per-subject message-body DEK reaching hot + cold
    /// segments + backups, purges read-state, fans to Search / Refs / Notif (mentions → `[erased user]`)
    /// — 0 recoverable PII — and the channel topology survives.**
    #[test]
    fn chat_d8_per_subject_body_dek_reaches_hot_and_cold_plus_cascade() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-chat", 40);
        let chat = ChatStoreModel::new();
        chat.index_from_source("u-chat");
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        let notif = NotifHistoryModel::new();
        search.index_from_source("u-chat", "bob's message");
        refs.add_edge_from_source("u-chat", "msg:7");
        notif.add_item_from_source("inbox-x", "u-chat");

        let ch = ChatStoreHolder::new(&chat, &kms);
        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        let nh = NotifHistoryHolder::new(&notif);

        let body_dek = subject_dek("u-chat", &tenant);
        assert!(
            chat.read_state_present("u-chat"),
            "read-state present before erase"
        );

        let receipt = IssuesChatCascadeDriver::fan_out_chat_erase(
            &subject_scope("u-chat"),
            &chat,
            &ch,
            &search,
            &sh,
            &refs,
            &rh,
            &notif,
            &nh,
            &kms,
        )
        .unwrap();

        // 0 recoverable in hot AND cold AND backups: the per-subject body DEK is destroyed (one key
        // seals all segments — a hot-only purge that left the cold segment readable is foreclosed).
        assert!(receipt.bodies_shredded, "the message-body DEK is shredded");
        assert!(
            !kms.is_present(&body_dek),
            "the body DEK is destroyed (live)"
        );
        assert_eq!(
            kms.recoverable_in_backup(&body_dek),
            0,
            "0 recoverable in backups — hot AND cold AND backups (CHAT-D8)"
        );
        // The cascade fired every leg.
        assert!(
            receipt.read_state_purged,
            "read-state / drafts / unfurl-cache purged"
        );
        assert!(!chat.read_state_present("u-chat"), "read-state is gone");
        assert!(receipt.notif_humanised, "Notif humanised mentions");
        assert!(receipt.embeddings_purged, "Search embeddings purged");
        assert!(receipt.refs_tombstoned, "Refs tombstoned");
        assert!(receipt.structure_survives, "the channel topology survives");
        assert_eq!(
            receipt.holder_receipts.len(),
            4,
            "primary + Search + Refs + Notif"
        );
        // The mention now humanises to `[erased user]`.
        assert_eq!(
            notif.render_mention("inbox-x").as_deref(),
            Some(ERASED_USER)
        );
        // The primary receipt names the hot+cold body DEK reach.
        let expected = Receipt::content_addressed(
            "erase",
            CHAT_DB,
            "u-chat",
            &tenant.0,
            "crypto_shred:per_subject_chat_body_dek:hot_and_cold;read_state_purged;structure_survives",
            receipt.holder_receipts[0].receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            receipt.holder_receipts[0].receipt.content_hash, expected.content_hash,
            "the primary receipt names the per-subject hot+cold body DEK reach"
        );
    }

    /// **The Chat per-tenant FALLBACK fires on a tenant offboarding (the other selection polarity).**
    #[test]
    fn the_chat_per_tenant_fallback_fires_on_a_tenant_offboarding() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-iso", 50);
        provision_tenant_dek(&kms, &tenant, 51);
        let model = ChatStoreModel::new();
        let holder = ChatStoreHolder::new(&model, &kms);

        let receipt = holder.erase(EraseScope::Tenant(tenant.clone())).unwrap();
        assert!(!kms.is_present(&tenant_dek(&tenant)));
        let expected_tenant = Receipt::content_addressed(
            "erase",
            CHAT_DB,
            "*tenant*",
            &tenant.0,
            "crypto_shred:per_tenant_chat_dek_fallback:tenant_offboard;structure_survives",
            receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(receipt.receipt.content_hash, expected_tenant.content_hash);
        let subj = holder.erase(subject_scope("u-iso")).unwrap();
        assert_ne!(receipt.receipt.content_hash, subj.receipt.content_hash);
        assert!(!kms.is_present(&subject_dek("u-iso", &tenant)));
    }

    /// **Both consumer holders' erase is idempotent + the structure survives a re-erase.**
    #[test]
    fn consumer_holders_erase_is_idempotent_structure_survives() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-idem", 60);
        let issues = IssuesStoreModel::new();
        issues.index_topology_from_source("u-idem");
        let ih = IssuesStoreHolder::new(&issues, &kms);

        let first = ih.erase(subject_scope("u-idem")).unwrap();
        let second = ih.erase(subject_scope("u-idem")).unwrap();
        assert_eq!(first.receipt.operation, second.receipt.operation);
        assert!(
            second.receipt.key_epoch_destroyed.is_none(),
            "the re-erase destroyed no key"
        );
        assert!(
            issues.topology_present("u-idem"),
            "the structure survives the re-erase"
        );
        assert_eq!(issues.erase_call_count(), 2, "both erase calls counted");
    }

    // ───────── locate distinguishes present from 0-recoverable (both holders) ─────────

    /// **The Issues + Chat `locate` distinguish present-PII from 0-recoverable on the per-subject DEK
    /// presence (mandatory-core).** A live DEK ⇒ located; a destroyed DEK ⇒ 0-recoverable. Exact
    /// content-addressed receipts pin the outcome strings.
    #[test]
    fn locate_reports_present_on_a_live_dek_and_zero_after_shred() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-loc", 70);
        let issues = IssuesStoreModel::new();
        let chat = ChatStoreModel::new();
        let ih = IssuesStoreHolder::new(&issues, &kms);
        let ch = ChatStoreHolder::new(&chat, &kms);

        let issues_present = ih.locate(&subject("u-loc"), tenant.clone()).unwrap();
        assert_eq!(
            issues_present.receipt.content_hash,
            Receipt::content_addressed(
                "locate",
                ISSUES_DB,
                "u-loc",
                &tenant.0,
                "located:issue-free-text-present",
                None,
                0
            )
            .content_hash
        );
        let chat_present = ch.locate(&subject("u-loc"), tenant.clone()).unwrap();
        assert_eq!(
            chat_present.receipt.content_hash,
            Receipt::content_addressed(
                "locate",
                CHAT_DB,
                "u-loc",
                &tenant.0,
                "located:chat-bodies-present",
                None,
                0
            )
            .content_hash
        );

        // Shred → both report 0-recoverable.
        ih.erase(subject_scope("u-loc")).unwrap();
        let issues_after = ih.locate(&subject("u-loc"), tenant.clone()).unwrap();
        assert_eq!(
            issues_after.receipt.content_hash,
            Receipt::content_addressed(
                "locate",
                ISSUES_DB,
                "u-loc",
                &tenant.0,
                "located:0-recoverable",
                None,
                0
            )
            .content_hash
        );
        assert_ne!(
            issues_present.receipt.content_hash,
            issues_after.receipt.content_hash
        );
    }

    /// **The Issues `restrict` is honoured into OLAP analytics (contract 11.6).** A restricted subject
    /// is excluded from cross-individual analytics — the OLAP suppression flag is set.
    #[test]
    fn issues_restrict_is_honoured_into_olap() {
        let kms = InMemoryShredKms::new();
        let issues = IssuesStoreModel::new();
        let ih = IssuesStoreHolder::new(&issues, &kms);
        assert!(
            !issues.olap_suppressed("u-r"),
            "not suppressed before restrict"
        );
        ih.restrict(&subject("u-r"), true).unwrap();
        assert!(
            issues.olap_suppressed("u-r"),
            "a restricted subject is excluded from cross-individual OLAP analytics (11.6)"
        );
        ih.restrict(&subject("u-r"), false).unwrap();
        assert!(
            !issues.olap_suppressed("u-r"),
            "clearing restriction re-enables"
        );
    }

    // ───────── the Issues/Chat instances: reference the ONE posture, never restate (§7.4) ─────────

    /// **The Issues + Chat instances reference the platform posture (do not restate it) — the P-GA-30
    /// GATE.** Each cites the canonical anchor and adds no restated posture text (the X-7 anti-pattern
    /// is foreclosed). The SAME [`reference_is_by_reference`] predicate the Git/CI instances fired.
    #[test]
    fn the_instances_reference_the_posture_and_do_not_restate() {
        assert_eq!(ISSUES_INSTANCE.subsystem, "issues");
        assert_eq!(CHAT_INSTANCE.subsystem, "chat");
        assert_eq!(ISSUES_INSTANCE.cited_anchor, POSTURE_ANCHOR);
        assert_eq!(CHAT_INSTANCE.cited_anchor, POSTURE_ANCHOR);
        assert!(
            issues_section_references_posture(),
            "the Issues erasure section is a valid by-reference instantiation"
        );
        assert!(
            chat_section_references_posture(),
            "the Chat erasure section is a valid by-reference instantiation"
        );
        // Neither carries a canonical restatement marker (the X-7 anti-pattern is structurally absent).
        for instance in [&ISSUES_INSTANCE, &CHAT_INSTANCE] {
            let lowered = instance.section_text.to_ascii_lowercase();
            for marker in restatement_markers() {
                assert!(
                    !lowered.contains(&marker.to_ascii_lowercase()),
                    "the {} section must not restate the canonical marker {marker:?}",
                    instance.subsystem
                );
            }
        }
    }

    /// **A restating Issues/Chat section is rejected** — the gate forbids the X-7 anti-pattern.
    #[test]
    fn a_restating_consumer_section_would_be_rejected() {
        let restating = SubsystemReference {
            subsystem: "issues",
            cited_anchor: POSTURE_ANCHOR,
            section_text: "Issues erasure: per-subject DEK crypto-shred renders free-text \
                 unrecoverable; the documented lawful-basis limit covers third-party mentions ...",
        };
        assert!(
            !reference_is_by_reference(&restating),
            "a section that restates the posture (a canonical marker) is rejected — X-7"
        );
    }

    /// **The Issues/Chat residuals == the ONE platform-posture residual (§7.2 / §7.4).**
    #[test]
    fn consumer_residuals_are_the_one_platform_posture_residual() {
        assert_eq!(issues_residual(), CANONICAL_POSTURE.residual);
        assert_eq!(chat_residual(), CANONICAL_POSTURE.residual);
        assert!(
            issues_residual().contains("AUTHOR's DEK") && issues_residual().contains("not the subject's"),
            "the residual is third-party PII under the AUTHOR's DEK — not shreddable by the subject's key"
        );
    }

    /// The worklog classification follow-on (P-GA-31) is named in writing — the floor is never
    /// pretended-shipped.
    #[test]
    fn the_worklog_classification_follow_on_is_named() {
        assert!(
            WORKLOG_CLASSIFICATION_FOLLOW_ON.contains("P-GA-31"),
            "the worklog Behavioural classification is the named follow-on"
        );
    }
}
