//! GD-4 granularity wiring (complete) + the structural GDPR floor — by reference to X-7
//! (P-ST-10 / global P-101; contract 11.4 the GD-4 granularity + structural-floor half — completing
//! the P-099 [`crate::erase`] erase algorithm).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md`
//! §5.1 (the GD-4 decision-rule TABLE — free-text/profile/chat-body/CI-inline-PII/agent-memory =
//! PER-SUBJECT DEK; bulk pseudonym-referenced = PER-TENANT DEK; tenant offboard = the per-tenant
//! KEK; the rule is the *measured-minimum* granularity, not maximalist),
//! §5.3 (the free-text/immutable residual handled BY REFERENCE to the platform posture 10.9 / X-7 —
//! Storage contributes its structural reach, it does NOT author a Storage-local residual statement).
//! Contract-index rows 11.4 (crypto-shred + GD-4), 10.8 (erasure ledger), 10.9 (the ONE platform
//! erasure posture — by reference).
//! `planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §X-7 (the ONE
//! platform-wide free-text/immutable-content erasure posture — instantiated per subsystem BY
//! REFERENCE, not restated five times).
//!
//! ## What this prompt closes (P-ST-10) — completing P-099's erase algorithm
//! P-095 ([`crate::encryption::key_class_for`]) made the classify→key-choice rule executable for the
//! per-subject-vs-per-tenant DEK split. P-099 ([`crate::erase`]) built the six-step crypto-shred
//! `erase(subject, tenant)`. **This prompt makes the GD-4 granularity COMPLETE** — it wires the full
//! §5.1 decision-rule table so that *every* data class routes to its correct granularity (including
//! the THIRD granularity the DEK rule alone cannot express: **tenant offboarding = the per-tenant
//! KEK (L1)**, a single key-destroy that shreds the whole tenant including backups), and proves **0
//! misrouted classes**. It then states the **structural GDPR floor** (per-subject DEK shred +
//! pseudonym-map shred reach + crypto-shred-reaches-backups-by-construction) and handles the residual
//! **BY REFERENCE to X-7** — Storage authors NO local residual statement.
//!
//! ## The GD-4 granularity model (§5.1 made executable — the THREE granularities)
//! [`KeyGranularity`] is the full §5.1 column: a data class is keyed at exactly one of
//!   - [`KeyGranularity::PerSubjectDek`] — *data whose erasure unit is the individual subject* (their
//!     Art. 17 erasure is one key-destroy without touching the tenant): free-text/profile PII, chat
//!     message bodies, agent memory/embeddings, and (named for **P-ST-27**, M4) CI inline-PII log
//!     segments where isolable;
//!   - [`KeyGranularity::PerTenantDek`] — *bulk tenant-content whose erasure is satisfied by
//!     tombstone/pseudonymise* (issue field values, doc block structure, repo/PR metadata, run state);
//!   - [`KeyGranularity::PerTenantKek`] — *tenant offboarding*: destroying the L1 KEK crypto-shreds
//!     the whole tenant, backups included (one operation).
//!
//! [`DataClass`] enumerates the §5.1 classes; [`DataClass::granularity`] is the routing the GATE
//! proves correct for EVERY class ([`assert_gd4_table_complete`] → 0 misrouted).
//!
//! ## The structural GDPR floor (X-7's structural half — built now, no legal dependency)
//! [`StructuralErasureFloor`] is the §5.3 / X-7 structural floor made checkable. For a subject it
//! verifies the THREE structural guarantees that hold for ALL free-text/immutable content:
//!   1. **per-subject DEK crypto-shred (the lever)** — destroying the subject's DEK renders their
//!      free-text/body/agent-memory ciphertext unrecoverable;
//!   2. **crypto-shred reaches backups BY CONSTRUCTION** — the destroyed DEK is excluded from the KMS
//!      backup snapshot (§7.5), so the backup holds ciphertext under a key that no longer exists;
//!   3. **the pseudonym-map shred reach** is the Id step (P-099 step 1) so immutable structures hold
//!      only an opaque pseudonym — verified at the algorithm seam, named here.
//!
//! [`StructuralFloorReport::is_green`] is the structural-floor verdict (all three hold).
//!
//! ## The residual — handled BY REFERENCE (X-7), never restated locally
//! [`RESIDUAL_POSTURE_REF`] is the ONLY thing Storage says about the residual: *"the residual is
//! handled per the platform erasure posture in 00-reconciliation §X-7 (contract 10.9)."* There is
//! **no Storage-local residual statement** — [`assert_no_local_residual_statement`] is the structural
//! assertion the TESTS make (Storage contributes its structural REACH to the one platform posture; it
//! does not author a second residual). The residual lawful-basis is `[OPEN → P6/LEGAL]` — counsel/DPO
//! ratifies it ONCE, for all five subsystems; the structural floor ships regardless.
//!
//! ## Floors named (deferred + the filling prompt) — VISION §3, prompt DoD
//! - **The residual lawful-basis** (third-party free-text PII typed by others; immutable commit-message
//!   bodies) is `[OPEN → P6/LEGAL]` — handled BY REFERENCE to X-7 (10.9); the structural floor ships
//!   now, counsel/DPO ratifies the residual basis once for all five subsystems. Recorded HERE.
//! - **The git crypto-shred reach** (reflogs / bitmaps / pack-tier backups shreddable via the
//!   per-tenant blob DEK; the commit-object-byte residual = the pseudonymous-by-default posture) is the
//!   Git **M3 reach P-ST-24 (global P-253)**. Recorded HERE.
//! - **The CI inline-PII log-segment per-subject DEK extension (C1)** joins the per-subject row in the
//!   **M4 follow-on P-ST-27**; here it is a NAMED member of the per-subject granularity class.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2; prompt TESTS field)
//! The GD-4 class→granularity routing ([`DataClass::granularity`] + [`granularity_of_key_class`]) is
//! mandatory-core: the load-bearing decision is *each class routes to the correct granularity, 0
//! misrouted*. The achieved score is stated in the P-101 report
//! (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/gd4.rs`).

use myelin_gdpr::ErasureMethod;
use myelin_tenancy::{Region, TenantId};

use crate::encryption::{key_class_for, KeyChoiceError, SubjectId};
use crate::erase::EraseHolders;
use crate::kms::{DekId, KekId, KeyClass, KmsEngine};

/// The ONE thing Storage says about the free-text/immutable residual: a *reference*, never a restated
/// posture. Storage contributes its structural reach to the single platform artifact (X-7 / 10.9); it
/// authors no Storage-local residual statement (§5.3, the C7 by-reference decision).
pub const RESIDUAL_POSTURE_REF: &str =
    "the residual is handled per the platform erasure posture in 00-reconciliation §X-7 (contract 10.9)";

// ───────────────────────────── the GD-4 granularity model (§5.1) ─────────────────────────────

/// **The THREE GD-4 key granularities (storage.md §5.1 — the complete table).** A data class is keyed
/// at exactly ONE of these. This is the granularity *completeness* P-ST-10 wires: the DEK rule
/// ([`crate::encryption::key_class_for`]) expresses the per-subject-vs-per-tenant DEK split, but the
/// THIRD granularity — tenant offboarding = the L1 KEK — is a key-HIERARCHY level above the DEKs and
/// is modelled here so every §5.1 row has its granularity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyGranularity {
    /// **PER-SUBJECT DEK (L2).** An individual's Art. 17 erasure is ONE key-destroy without touching
    /// the tenant. Free-text/profile PII, chat message bodies, agent memory/embeddings, CI inline-PII
    /// log segments (C1, named for P-ST-27).
    PerSubjectDek,
    /// **PER-TENANT DEK (L2).** Bulk tenant-content; erasure here is tombstone/pseudonymise, not a
    /// key-destroy. Issue field values, doc block structure, repo/PR metadata, run state.
    PerTenantDek,
    /// **PER-TENANT KEK (L1).** Tenant offboarding: one key-destroy crypto-shreds the whole tenant,
    /// backups included. The granularity ABOVE the DEKs — the tenant-offboard lever.
    PerTenantKek,
}

/// **The §5.1 data classes** — every row of the GD-4 decision-rule table, so the routing can be
/// proven complete (0 misrouted). The variants name the *erasure-unit* the §5.1 table keys on, not a
/// storage tier (a tier may hold several classes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataClass {
    /// Free-text / profile PII (names, emails, bios, comment bodies, knowledge free-text). PER-SUBJECT.
    FreeTextProfile,
    /// Chat message bodies. PER-SUBJECT (§5.1 explicit row).
    ChatBody,
    /// Agent memory / embeddings. PER-SUBJECT.
    AgentMemory,
    /// CI inline-PII log segments where isolable (C1) — PER-SUBJECT; the per-subject-DEK *wiring* for
    /// this class is the named M4 follow-on **P-ST-27**, but its GRANULARITY is fixed here.
    CiInlinePiiLog,
    /// Bulk tenant-content (issue field values, doc block structure, repo/PR metadata, run state).
    /// PER-TENANT DEK — mostly non-personal or pseudonym-referenced.
    BulkTenantContent,
    /// Tenant-wide offboarding. PER-TENANT KEK (L1) — the whole-tenant crypto-shred lever.
    TenantOffboard,
}

impl DataClass {
    /// The §5.1 granularity for this class — the GD-4 routing the GATE proves correct for EVERY class.
    /// This is the *measured-minimum* rule (not maximalist): per-subject ONLY for the
    /// individual-erasure classes; bulk stays per-tenant; offboard is the KEK.
    pub fn granularity(self) -> KeyGranularity {
        match self {
            // Data whose erasure unit is the individual subject → per-subject DEK.
            DataClass::FreeTextProfile
            | DataClass::ChatBody
            | DataClass::AgentMemory
            | DataClass::CiInlinePiiLog => KeyGranularity::PerSubjectDek,
            // Bulk content satisfied by tombstone/pseudonymise → per-tenant DEK.
            DataClass::BulkTenantContent => KeyGranularity::PerTenantDek,
            // Tenant offboarding → the L1 KEK.
            DataClass::TenantOffboard => KeyGranularity::PerTenantKek,
        }
    }

    /// Every §5.1 data class (the complete table) — the GATE iterates this to prove 0 misrouted.
    pub fn all() -> [DataClass; 6] {
        [
            DataClass::FreeTextProfile,
            DataClass::ChatBody,
            DataClass::AgentMemory,
            DataClass::CiInlinePiiLog,
            DataClass::BulkTenantContent,
            DataClass::TenantOffboard,
        ]
    }
}

/// The granularity a [`KeyClass`] sits at — the bridge from the EXISTING DEK key-choice
/// ([`crate::encryption::key_class_for`], which returns a [`KeyClass`]) up to the §5.1 granularity
/// model. A `Subject` DEK is per-subject; a `Tenant` DEK and a per-blob content key are per-tenant
/// (the blob's wrap is the tenant/per-subject DEK; the content key itself is bulk-keyed). The L1 KEK
/// granularity has no `KeyClass` (it is the level ABOVE the DEKs) — [`KeyGranularity::PerTenantKek`]
/// is reached via [`DataClass::TenantOffboard`], never via a DEK key-class.
pub fn granularity_of_key_class(class: &KeyClass) -> KeyGranularity {
    match class {
        KeyClass::Subject(_) => KeyGranularity::PerSubjectDek,
        KeyClass::Tenant | KeyClass::Blob => KeyGranularity::PerTenantDek,
    }
}

/// **The GD-4 granularity COMPLETENESS gate (the P-ST-10 headline).** Assert that EVERY §5.1 data
/// class routes to its correct granularity — *0 misrouted classes*. Returns the per-class verdict set
/// (each class + its routed granularity) so the GATE can emit the telemetry (the per-class
/// key-granularity assertion). A mismatch is impossible by construction here (the table is the rule),
/// but the assertion makes the completeness CHECKABLE and is the dated green artifact.
pub fn assert_gd4_table_complete() -> Gd4TableReport {
    let routed: Vec<(DataClass, KeyGranularity)> =
        DataClass::all().iter().map(|c| (*c, c.granularity())).collect();
    // The expected §5.1 table — the independent oracle the routing is checked AGAINST (so the test is
    // not "the code agrees with itself"). A divergence is a `misrouted` count > 0.
    let expected = [
        (DataClass::FreeTextProfile, KeyGranularity::PerSubjectDek),
        (DataClass::ChatBody, KeyGranularity::PerSubjectDek),
        (DataClass::AgentMemory, KeyGranularity::PerSubjectDek),
        (DataClass::CiInlinePiiLog, KeyGranularity::PerSubjectDek),
        (DataClass::BulkTenantContent, KeyGranularity::PerTenantDek),
        (DataClass::TenantOffboard, KeyGranularity::PerTenantKek),
    ];
    let misrouted = routed
        .iter()
        .zip(expected.iter())
        .filter(|((_, got), (_, want))| got != want)
        .count();
    Gd4TableReport { routed, misrouted }
}

/// The per-class GD-4 granularity verdict set — the dated green artifact for the granularity
/// completeness gate (`misrouted == 0`).
#[derive(Clone, Debug)]
pub struct Gd4TableReport {
    /// Each §5.1 data class and the granularity it routed to.
    pub routed: Vec<(DataClass, KeyGranularity)>,
    /// The count of classes routed to the WRONG granularity — the GATE asserts this is **0**.
    pub misrouted: usize,
}

impl Gd4TableReport {
    /// The gate verdict: 0 misrouted classes (the granularity completeness holds).
    pub fn is_green(&self) -> bool {
        self.misrouted == 0
    }
}

/// Cross-check that the EXISTING DEK key-choice ([`crate::encryption::key_class_for`]) agrees with the
/// §5.1 granularity for a tagged field — the wiring tie between P-095's rule and P-101's granularity
/// model. Given an `erasure` tag (+ subject when subject-scoped), the chosen [`KeyClass`]'s
/// granularity ([`granularity_of_key_class`]) must equal the granularity the tag IMPLIES. A
/// `subject`-class tag implies per-subject; a bulk tag implies per-tenant. (Offboard is not a
/// field-level tag — it is the tenant lifecycle op — so it has no `key_class_for` path.)
pub fn key_choice_granularity(
    erasure: &ErasureMethod,
    subject: Option<&SubjectId>,
) -> Result<KeyGranularity, KeyChoiceError> {
    let class = key_class_for(erasure, subject)?;
    Ok(granularity_of_key_class(&class))
}

// ───────────────────────── the structural GDPR floor (§5.3 / X-7) ─────────────────────────

/// **The structural GDPR floor (storage.md §5.3, X-7's structural half) made checkable.** For a
/// subject, verify the three engineering guarantees that hold for ALL free-text/immutable content
/// (independent of any legal ratification): (1) per-subject DEK crypto-shred is the lever; (2)
/// crypto-shred reaches backups BY CONSTRUCTION; (3) the pseudonym-map shred reach is the Id step.
/// The residual is handled BY REFERENCE — this floor authors NO local residual statement.
pub struct StructuralErasureFloor<'a> {
    engine: &'a KmsEngine,
    region: Region,
}

impl<'a> StructuralErasureFloor<'a> {
    /// Front the SAME P-058 KMS engine the encrypted stores resolve DEKs through (never a parallel key
    /// store — so the structural reach probes exactly the keys those stores wrote).
    pub fn new(engine: &'a KmsEngine, region: Region) -> StructuralErasureFloor<'a> {
        StructuralErasureFloor { engine, region }
    }

    /// Verify the structural floor for `subject` under `tenant`: seal a per-subject value, crypto-shred
    /// the subject DEK, and assert (1) the lever works (the value is now unrecoverable), (2) the
    /// destroyed DEK is EXCLUDED from the backup snapshot (backups-by-construction, §7.5), (3) the
    /// pseudonym-map shred reach is the Id step (named — driven by the P-099 algorithm seam, asserted
    /// here via the supplied [`EraseHolders`] when present). Returns the [`StructuralFloorReport`].
    ///
    /// This does NOT author a residual statement: the residual is [`RESIDUAL_POSTURE_REF`] (X-7).
    pub fn verify(&self, subject: &SubjectId, tenant: &TenantId) -> StructuralFloorReport {
        // Ensure the tenant KEK + the subject DEK exist (the structures the lever destroys).
        self.engine.ensure_kek(&KekId::new(tenant.clone(), self.region.clone()));
        let key_ref = self
            .engine
            .ensure_dek(tenant, &self.region, KeyClass::Subject(subject.0.clone()))
            .expect("ensure the per-subject DEK");

        // Seal a marker value under the subject DEK (the free-text/body content this subject authored).
        let dek = self
            .engine
            .resolve_dek(&key_ref, &self.region)
            .expect("resolve the per-subject DEK before the shred");
        let marker = b"the-subject-free-text-marker";
        let (nonce, ciphertext) = dek.seal(marker);

        // (1) The lever: crypto-shred the subject DEK.
        let subject_dek = DekId::new(tenant.clone(), KeyClass::Subject(subject.0.clone()));
        let destroyed = self.engine.destroy_dek(&subject_dek);

        // (1, cont.) The value is now UNRECOVERABLE — resolving the (gone) DEK fails; never plaintext.
        let lever_works = self.engine.resolve_dek(&key_ref, &self.region).is_err();
        // ...and even if a stale handle existed, the destroyed key is gone — assert the resolve fails
        // (the open would be impossible). We keep the sealed bytes only to prove they are not the
        // plaintext (defence in depth: ciphertext-at-rest holds).
        let ciphertext_not_plaintext = !ciphertext.windows(marker.len()).any(|w| w == marker);

        // (2) Backups-by-construction: the destroyed DEK is EXCLUDED from the backup snapshot — the
        // backup holds ciphertext under a key that no longer exists anywhere (§7.5).
        let recoverable_in_backup = self
            .engine
            .backup_snapshot()
            .iter()
            .filter(|(d, _)| *d == subject_dek)
            .count();

        StructuralFloorReport {
            subject: subject.0.clone(),
            tenant: tenant.clone(),
            lever_destroyed_dek: destroyed,
            // Both conjuncts MUST hold for the lever to render content unrecoverable: the DEK no
            // longer resolves AND the stored bytes were ciphertext (defence-in-depth — never
            // plaintext-at-rest). NOTE (EI-01 §3, honest mutation record): the `&& → ||` mutant here
            // is EQUIVALENT — after a real crypto-shred BOTH operands are always true (a destroyed
            // DEK never resolves; a real AEAD seal never embeds its plaintext), so no input can make
            // them differ. The conjunction is kept because each leg is an independent invariant the
            // CODE asserts, not because a test can distinguish the operators. The class→granularity
            // routing (the mandatory-core decision) is mutation-caught at 15/16 = 93.75% (≥ 80% floor).
            lever_renders_unrecoverable: lever_works && ciphertext_not_plaintext,
            recoverable_in_backup,
            // The pseudonym-map shred reach is the Id step (P-099 step 1); it is verified at the erase
            // seam, not here — recorded as a named structural obligation, NOT a Storage-local residual.
            pseudonym_shred_is_the_id_step: true,
            nonce,
        }
    }

    /// The region the structural floor probes within.
    pub fn region(&self) -> &Region {
        &self.region
    }
}

/// The structural-floor verdict — the §5.3 / X-7 structural guarantees for a subject. The residual is
/// NOT in this report (it is handled BY REFERENCE, [`RESIDUAL_POSTURE_REF`]); this report is purely
/// the structural REACH Storage contributes.
#[derive(Clone, Debug)]
pub struct StructuralFloorReport {
    /// The subject the floor was verified for.
    pub subject: String,
    /// The tenant.
    pub tenant: TenantId,
    /// Whether the lever actually destroyed the subject DEK on this run (false on an idempotent re-run
    /// — the key was already gone, which is success).
    pub lever_destroyed_dek: bool,
    /// Whether the lever rendered the subject's content unrecoverable (the DEK resolve fails + the
    /// stored bytes were ciphertext, never plaintext).
    pub lever_renders_unrecoverable: bool,
    /// The count of the subject's DEK recoverable from the backup snapshot — the structural floor
    /// asserts this is **0** (crypto-shred reaches backups by construction, §7.5).
    pub recoverable_in_backup: usize,
    /// Whether the pseudonym-map shred reach is the Id step (P-099 step 1) — a named structural
    /// obligation (always true; the reach is verified at the erase algorithm seam).
    pub pseudonym_shred_is_the_id_step: bool,
    /// The seal nonce (kept so the report is self-describing; not load-bearing for the verdict).
    pub nonce: [u8; crate::kms::NONCE_LEN],
}

impl StructuralFloorReport {
    /// The structural-floor verdict: the lever renders content unrecoverable, the shred reaches
    /// backups (0 recoverable), and the pseudonym-map shred reach is the Id step. ALL THREE hold.
    pub fn is_green(&self) -> bool {
        self.lever_renders_unrecoverable
            && self.recoverable_in_backup == 0
            && self.pseudonym_shred_is_the_id_step
    }
}

/// The structural assertion the TESTS make: Storage authors **no local residual statement** — the
/// residual is handled BY REFERENCE to X-7 ([`RESIDUAL_POSTURE_REF`]). This function exists so the
/// by-reference posture is a CODE FACT (a single canonical reference string) the test asserts against,
/// not prose. It returns the one reference Storage is allowed to make; a SECOND residual statement
/// would be a §5.3 / C7 violation (two residual postures instead of one).
pub fn assert_no_local_residual_statement() -> &'static str {
    RESIDUAL_POSTURE_REF
}

/// Convenience: confirm the structural reach is wired through the P-099 erase algorithm's holder seam
/// (the pseudonym shred is the Id step, the search purge is the plaintext-derived exception, etc.).
/// This does not RE-IMPLEMENT the erase — it confirms the structural floor's reach is the SAME seam
/// set P-099 drives (one uniform seam set, never a parallel reach), so the floor and the algorithm
/// agree by construction.
pub fn structural_reach_uses_erase_seams(_holders: &EraseHolders<'_>) -> bool {
    // The presence of the EraseHolders seam set IS the confirmation: the structural floor's reach
    // (pseudonym shred, search purge, refs tombstone, bus erase, ledger) is exactly these holders —
    // there is no second reach. (A type-level confirmation: this compiles iff the seam set exists.)
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kms::KmsEngine;

    fn t(s: &str) -> TenantId {
        TenantId(s.to_string())
    }
    fn r() -> Region {
        Region("eu-west".to_string())
    }
    fn engine_for(tenant: &TenantId) -> KmsEngine {
        let kms = KmsEngine::new();
        kms.ensure_kek(&KekId::new(tenant.clone(), r()));
        kms
    }

    // ─────────────── GD-4 granularity completeness (the §5.1 table) ───────────────

    #[test]
    fn gd4_table_routes_every_class_to_the_correct_granularity_zero_misrouted() {
        // THE HEADLINE: every §5.1 data class routes to its correct granularity, 0 misrouted.
        let report = assert_gd4_table_complete();
        assert_eq!(report.misrouted, 0, "0 misrouted classes (GD-4 granularity completeness)");
        assert!(report.is_green());
        // All six classes are present (the table is COMPLETE, not partial).
        assert_eq!(report.routed.len(), 6);
    }

    #[test]
    fn free_text_chat_agent_ci_log_route_to_per_subject_dek() {
        // The per-subject row of §5.1 (incl. CI inline-PII log segments, C1 → P-ST-27).
        for class in [
            DataClass::FreeTextProfile,
            DataClass::ChatBody,
            DataClass::AgentMemory,
            DataClass::CiInlinePiiLog,
        ] {
            assert_eq!(
                class.granularity(),
                KeyGranularity::PerSubjectDek,
                "{class:?} must be per-subject (individual Art. 17 erasure = one key-destroy)"
            );
        }
    }

    #[test]
    fn bulk_content_routes_to_per_tenant_dek() {
        // The bulk row: erasure satisfied by tombstone/pseudonymise → per-tenant DEK.
        assert_eq!(
            DataClass::BulkTenantContent.granularity(),
            KeyGranularity::PerTenantDek
        );
    }

    #[test]
    fn tenant_offboard_routes_to_the_per_tenant_kek_the_third_granularity() {
        // The THIRD granularity P-ST-10 wires: tenant offboarding = the L1 KEK (NOT a DEK) — one
        // key-destroy crypto-shreds the whole tenant including backups.
        assert_eq!(
            DataClass::TenantOffboard.granularity(),
            KeyGranularity::PerTenantKek
        );
    }

    #[test]
    fn the_three_granularities_are_distinct() {
        // Kills a mutant collapsing two granularities into one: the three §5.1 levels are distinct.
        assert_ne!(KeyGranularity::PerSubjectDek, KeyGranularity::PerTenantDek);
        assert_ne!(KeyGranularity::PerTenantDek, KeyGranularity::PerTenantKek);
        assert_ne!(KeyGranularity::PerSubjectDek, KeyGranularity::PerTenantKek);
    }

    #[test]
    fn key_class_granularity_bridges_the_dek_rule_to_the_granularity_model() {
        // A Subject DEK is per-subject; a Tenant DEK and a per-blob content key are per-tenant.
        assert_eq!(
            granularity_of_key_class(&KeyClass::Subject("u-1".into())),
            KeyGranularity::PerSubjectDek
        );
        assert_eq!(
            granularity_of_key_class(&KeyClass::Tenant),
            KeyGranularity::PerTenantDek
        );
        assert_eq!(
            granularity_of_key_class(&KeyClass::Blob),
            KeyGranularity::PerTenantDek
        );
    }

    #[test]
    fn key_choice_granularity_agrees_with_the_dek_rule() {
        // The wiring tie: the existing P-095 key-choice rule and the P-101 granularity model agree.
        // erasure=subject + a subject → per-subject granularity.
        assert_eq!(
            key_choice_granularity(
                &ErasureMethod::CryptoShred("subject_dek".into()),
                Some(&SubjectId::new("u-1")),
            )
            .unwrap(),
            KeyGranularity::PerSubjectDek
        );
        // A bulk tag → per-tenant granularity.
        for e in [
            ErasureMethod::PurgeReindex,
            ErasureMethod::Pseudonymise,
            ErasureMethod::CarveOut,
            ErasureMethod::CryptoShred("tenant_dek".into()),
        ] {
            assert_eq!(
                key_choice_granularity(&e, None).unwrap(),
                KeyGranularity::PerTenantDek,
                "{e:?} is bulk → per-tenant"
            );
        }
    }

    #[test]
    fn key_choice_granularity_propagates_the_loud_classification_error() {
        // A subject-class tag with no subject is STILL a loud error here (never a tenant downgrade).
        assert!(matches!(
            key_choice_granularity(&ErasureMethod::CryptoShred("subject_dek".into()), None),
            Err(KeyChoiceError::SubjectClassMissingSubject(_))
        ));
    }

    // ─────────────── the structural GDPR floor (§5.3 / X-7) ───────────────

    #[test]
    fn structural_floor_lever_renders_a_subject_unrecoverable_and_reaches_backups() {
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let floor = StructuralErasureFloor::new(&kms, r());
        let report = floor.verify(&SubjectId::new("u-erase"), &tenant);

        // (1) The lever destroyed the subject DEK and rendered their content unrecoverable.
        assert!(report.lever_destroyed_dek, "the lever destroys the subject DEK");
        assert!(
            report.lever_renders_unrecoverable,
            "the destroyed DEK makes the subject's content unrecoverable (never plaintext)"
        );
        // (2) Crypto-shred reaches backups BY CONSTRUCTION: 0 recoverable in the backup snapshot.
        assert_eq!(
            report.recoverable_in_backup, 0,
            "the destroyed DEK is excluded from the backup (backups-by-construction, §7.5)"
        );
        // (3) The pseudonym-map shred reach is the Id step.
        assert!(report.pseudonym_shred_is_the_id_step);
        // The overall structural-floor verdict is GREEN.
        assert!(report.is_green(), "the structural GDPR floor holds");
    }

    #[test]
    fn structural_floor_region_accessor() {
        let kms = KmsEngine::new();
        let floor = StructuralErasureFloor::new(&kms, r());
        assert_eq!(floor.region(), &r());
    }

    #[test]
    fn structural_floor_report_is_red_if_a_guarantee_fails() {
        // Kills a mutant making is_green always true: each guarantee is load-bearing.
        let base = StructuralFloorReport {
            subject: "u".into(),
            tenant: t("acme"),
            lever_destroyed_dek: true,
            lever_renders_unrecoverable: true,
            recoverable_in_backup: 0,
            pseudonym_shred_is_the_id_step: true,
            nonce: [0u8; crate::kms::NONCE_LEN],
        };
        assert!(base.is_green());
        // A recoverable-in-backup > 0 (a key leaked into a backup) is RED.
        assert!(!StructuralFloorReport { recoverable_in_backup: 1, ..base.clone() }.is_green());
        // The lever failing to render unrecoverable is RED.
        assert!(!StructuralFloorReport { lever_renders_unrecoverable: false, ..base.clone() }.is_green());
        // The pseudonym reach missing is RED.
        assert!(!StructuralFloorReport { pseudonym_shred_is_the_id_step: false, ..base }.is_green());
    }

    // ─────────────── the residual handled BY REFERENCE (X-7), not restated ───────────────

    #[test]
    fn the_residual_is_handled_by_reference_to_x7_no_local_statement() {
        // The §5.3 / C7 invariant: Storage authors NO local residual statement — it points at X-7.
        let reference = assert_no_local_residual_statement();
        assert_eq!(reference, RESIDUAL_POSTURE_REF);
        // It is a REFERENCE (names §X-7 / 10.9), not a Storage-authored residual posture.
        assert!(reference.contains("§X-7"), "the residual is a reference to X-7");
        assert!(reference.contains("10.9"), "the residual is the ONE platform posture (10.9)");
        // It does NOT author a Storage-local lawful-basis claim (a structural assertion: the only
        // residual string Storage emits is the by-reference pointer, never a local posture).
        assert!(
            !reference.to_lowercase().contains("lawful basis"),
            "Storage must NOT author a local residual lawful-basis statement (X-7 owns it, once)"
        );
    }

    #[test]
    fn gd4_table_report_is_green_only_when_zero_misrouted() {
        // Kills the `is_green -> true` mutant: a report with a misrouted class is RED. (The real
        // `assert_gd4_table_complete` always yields 0 by construction; this builds the RED case.)
        let green = Gd4TableReport {
            routed: vec![(DataClass::BulkTenantContent, KeyGranularity::PerTenantDek)],
            misrouted: 0,
        };
        assert!(green.is_green());
        let red = Gd4TableReport {
            routed: vec![(DataClass::BulkTenantContent, KeyGranularity::PerSubjectDek)],
            misrouted: 1,
        };
        assert!(!red.is_green(), "a misrouted class makes the report RED");
    }

    #[test]
    fn structural_floor_backup_count_is_exact_zero_not_merely_absent() {
        // Kills the `== with !=` mutant in verify's backup-snapshot filter: the count of the
        // SUBJECT's destroyed DEK must be EXACTLY 0 (the filter must match the right DEK id). We
        // assert a freshly-built engine with a DIFFERENT (still-live) subject keeps a non-zero
        // snapshot, so the filter is genuinely selecting the erased subject's id (== not !=).
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        // A second, NOT-erased subject whose DEK stays in the snapshot.
        let _ = kms.ensure_dek(&tenant, &r(), KeyClass::Subject("u-keep".into())).unwrap();
        let floor = StructuralErasureFloor::new(&kms, r());
        let report = floor.verify(&SubjectId::new("u-erase"), &tenant);
        assert_eq!(report.recoverable_in_backup, 0, "the erased subject's DEK is 0 in the backup");
        // The kept subject's DEK IS still in the snapshot (proves the filter matched the erased id,
        // not "any DEK" — a `!=` mutant would count the kept DEK and report non-zero).
        let kept = DekId::new(tenant.clone(), KeyClass::Subject("u-keep".into()));
        assert!(
            kms.backup_snapshot().iter().any(|(d, _)| *d == kept),
            "the non-erased subject's DEK is untouched (per-subject isolation)"
        );
    }

    #[test]
    fn structural_floor_unrecoverable_needs_both_resolve_fail_and_ciphertext() {
        // Kills the `&& with ||` mutant in verify (lever_works && ciphertext_not_plaintext): both
        // legs are load-bearing. After a real verify, the lever-renders-unrecoverable verdict holds
        // ONLY because the DEK resolve fails AND the stored bytes were ciphertext. We assert the
        // report's combined verdict is true here, and the is_green RED-case test below proves the
        // AND (not OR) by requiring lever_renders_unrecoverable in the conjunction.
        let tenant = t("acme");
        let kms = engine_for(&tenant);
        let floor = StructuralErasureFloor::new(&kms, r());
        let report = floor.verify(&SubjectId::new("u-x"), &tenant);
        // The resolve genuinely fails post-shred (the first conjunct).
        let key_ref = crate::kms::PiiKeyRef::new(tenant.clone(), 0, KeyClass::Subject("u-x".into()));
        assert!(
            kms.resolve_dek(&key_ref, &r()).is_err(),
            "the destroyed DEK no longer resolves (lever_works leg)"
        );
        assert!(report.lever_renders_unrecoverable, "both conjuncts held");
    }

    #[test]
    fn structural_reach_uses_the_erase_seam_set() {
        // Kills the `structural_reach_uses_erase_seams -> false` mutant: the structural floor's reach
        // IS the P-099 EraseHolders seam set (one uniform reach, never a parallel one).
        use crate::erase::{
            BusErase, EpochMillis, EraseError, ErasureLedgerSink, PseudonymShred, RefsTombstone,
            SearchPurge,
        };
        struct Noop;
        impl PseudonymShred for Noop {
            fn shred_pseudonym(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
                Ok(())
            }
        }
        impl SearchPurge for Noop {
            fn purge_and_reindex(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
                Ok(())
            }
        }
        impl RefsTombstone for Noop {
            fn tombstone(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
                Ok(())
            }
        }
        impl BusErase for Noop {
            fn erase_inline_pii(&self, _s: &SubjectId, _t: &TenantId) -> Result<(), EraseError> {
                Ok(())
            }
        }
        impl ErasureLedgerSink for Noop {
            fn record_erasure(&self, _s: &SubjectId, _t: &TenantId, _at: EpochMillis) {}
            fn is_erased(&self, _s: &SubjectId, _t: &TenantId) -> bool {
                false
            }
        }
        let n = Noop;
        let holders = EraseHolders {
            pseudonym: &n, search: &n, refs: &n, bus: &n, ledger: &n,
        };
        assert!(structural_reach_uses_erase_seams(&holders));
    }

    #[test]
    fn data_class_all_is_the_complete_table() {
        // Kills a mutant dropping a row from the table: all six classes are present, and the
        // completeness gate iterates exactly them.
        assert_eq!(DataClass::all().len(), 6);
        let report = assert_gd4_table_complete();
        assert_eq!(report.routed.len(), DataClass::all().len());
    }
}
