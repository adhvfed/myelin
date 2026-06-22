//! # The ONE free-text / immutable-content erasure posture (X-7 / OQ-G) — contract 10.9
//!
//! **P-GA-16 → P-116.** This module is the **single canonical artifact** for the
//! free-text / immutable-content erasure posture. The same legal seam was named **five times** —
//! Git (immutable commit bytes), CI (inline log PII), Issues (third-party free-text mentions),
//! Knowledge (free-text blocks), Chat (a name typed into another user's message body). Phase 5
//! generalised it to **ONE platform-wide posture, instantiated per subsystem BY REFERENCE, never
//! restated five times** (the named "Erasure vs. Immutability reconciliation" deliverable GD-1 /
//! L-2). This module IS that one artifact: the structured posture data + the doc text it renders.
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §7 (§7.1 the structural
//! floor; §7.2 the residual; §7.3 the ratified engineering posture `[OPEN — LEGAL]`; §7.4
//! instantiation per subsystem BY REFERENCE). The decision is
//! `00-reconciliation-decisions.md` X-7 / OQ-G; the contract row is `contract-index.md` 10.9.
//!
//! ## What this module ships (the canonical artifact, contract 10.9)
//! 1. [`ErasurePosture`] — the structured posture: the **structural floor** (the three levers
//!    §7.1: per-subject DEK crypto-shred + pseudonym-map shred + `restrict` suppression), the
//!    **residual** (third-party / immutable-byte free-text PII authored by *others*, encrypted
//!    under the AUTHOR's DEK — NOT crypto-shreddable by the *subject's* key, §7.2), and the
//!    **ratified engineering posture** (§7.3: the documented lawful-basis limit, best-effort
//!    `rectify`/tombstone, the standing `restrict` guarantee). The residual lawful-basis
//!    ratification is the [`LegalStatus::OpenLegal`] tag — **ONE statement, not five** — and the
//!    structural floor ships regardless.
//! 2. [`CANONICAL_POSTURE`] — the ONE in-process instance. Every subsystem (M3/M4: Git P-GA-28,
//!    CI/Issues/Knowledge/Chat P-GA-29/-31) references THIS, never restates it.
//! 3. [`ErasurePosture::render`] — the doc text the artifact GENERATES (the prompt: "a documented
//!    module + the doc text it generates"). The render is the single source the per-subsystem
//!    instances cite by anchor ([`POSTURE_ANCHOR`]).
//! 4. [`SubsystemReference`] / [`reference_is_by_reference`] — the **architecture-test scaffolding**
//!    for the GATE: a subsystem erasure section must REFERENCE the posture (cite the canonical
//!    anchor) and **never restate** it. The scaffolding lands here; the assertions FIRE when the
//!    M3/M4 subsystem instances register their references (P-GA-28 Git is the first — its CDC
//!    consumer completes the 10.9 pair).
//!
//! ## Floors named (deferred → filling prompt) — name-your-floors doctrine
//! - The **structural-floor PROOF on the M1 stores** (per-subject DEK shred renders self-authored
//!   free-text unrecoverable + pseudonym-map shred leaves only `<pseudonym>@<tenant>.noreply` +
//!   `restrict` suppresses) → **P-GA-17 → P-117**. This module STATES the floor; P-117 PROVES it
//!   end-to-end on the live M1 stores.
//! - The **pseudonymous-by-default commit-identity prerequisite** for Git (the commit-time
//!   prerequisite GIT-1 so the immutable hash never bakes in erasable PII) → **P-GA-18 → P-118**;
//!   its architecture test FIRES when Git commits hold only the pseudonym form (P-GA-28).
//! - The **audited history-rewrite erasure path** (gdpr §6.6, GA-10 — the rare case where a body
//!   must be expunged, with the disruptive changed-hash consequence) → **M5 P-GA-35**.
//! - The **per-subsystem reference assertions** (the GATE's "references it, never restates"
//!   architecture test fully bites): the consumer half of the 10.9 CDC pair + the first real
//!   [`SubsystemReference`] register land in **P-GA-28 → P-256/P-257** (Git, by reference);
//!   CI/Issues/Knowledge/Chat in **P-GA-29/-31**. The scaffolding + the predicate are complete and
//!   tested here against an in-module exemplar reference.
//! - The **residual lawful-basis ratification** (the `[OPEN — LEGAL]` tag flips to ratified) is
//!   **parallel-legal** — the DPO ratifies; the structural floor ships regardless (§7.3). Not a
//!   code floor: a legal status this module carries explicitly so it is never pretended-solved.
//!
//! ## Mutation floor — not core logic (a documented artifact)
//! Per the prompt TESTS: this is a documented canonical artifact, not a behavioral algorithm, so
//! there is **no mutation floor** — NAMED. The one predicate with behavior
//! ([`reference_is_by_reference`]) is covered by the unit tests (a by-reference cite passes; a
//! restatement is rejected); its real assertions fire when M3/M4 register references.

use std::collections::BTreeSet;

/// The canonical anchor every subsystem erasure section cites — the ONE source of truth. A
/// by-reference instantiation (§7.4) names this anchor and adds NO restated posture text.
///
/// This is the platform-wide string the M3/M4 subsystem docs reference verbatim (Git P-GA-28,
/// CI/Issues/Knowledge/Chat P-GA-29/-31) — `[`reference_is_by_reference`]` checks a subsystem
/// reference cites it.
pub const POSTURE_ANCHOR: &str =
    "00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9)";

/// The contract-index row this artifact owns.
pub const POSTURE_CONTRACT_ROW: &str = "10.9";

// ───────────────────────── the legal status of the residual (§7.3) ─────────────────────────

/// The lawful-basis ratification status of the residual posture. The structural floor (§7.1) has
/// **no legal dependency** and ships regardless; the residual third-party / immutable free-text
/// PII basis is `[OPEN — LEGAL]` until the DPO ratifies (§7.3 — ONE statement, not five). This is
/// carried explicitly so the open question is **named, never pretended-solved** (name-your-floors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegalStatus {
    /// The DPO/counsel ratification is pending. The residual lawful basis + the Art. 17 reach into
    /// immutable git bytes + the history-rewrite-vs-documented-limit choice + the audit-log
    /// retention carve-out + the worklog-sensitivity classification are what counsel must ratify —
    /// ONE statement (§7.3). The structural floor ships regardless.
    OpenLegal,
    /// The DPO has ratified the residual lawful-basis posture. (Set by the parallel-legal track,
    /// not by code; this variant exists so the artifact can record the ratified state without a
    /// shape change when it lands.)
    Ratified,
}

impl LegalStatus {
    /// The `[OPEN — LEGAL]` tag text (the ONE statement — never five). Empty once ratified.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            LegalStatus::OpenLegal => "[OPEN — LEGAL]",
            LegalStatus::Ratified => "",
        }
    }
}

// ───────────────────────── the structural floor (§7.1) — the three levers ─────────────────────

/// One of the three structural-floor levers (§7.1). Each is **fully built, no legal dependency**;
/// the PROOF that each works end-to-end on the M1 stores is P-GA-17 → P-117 (named floor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StructuralLever {
    /// **Per-subject DEK crypto-shred** (contract 11.4, GD-4). Free-text / body / op-log /
    /// agent-trace columns — and CI log segments (§3.2 H2) — are encrypted with a per-subject DEK;
    /// a subject's erasure destroys their DEK, so their **self-authored** content in DBs, backups,
    /// and immutable logs becomes unrecoverable ciphertext. The primary erasure mechanism for
    /// *their own* content. (Consumed contract 11.4, referenced — Storage owns the DEK.)
    PerSubjectDekShred,
    /// **Pseudonym-map shred** (contract 4.8, identity erasure). Author/subject identity in
    /// immutable structures is the stable opaque pseudonym `<pseudonym>@<tenant>.noreply`; the
    /// person↔pseudonym map is the erasable record — DSR fan-out **step 1**. Erasing the map means
    /// the immutable bytes (commit author, event actor, audit entry) hold only a pseudonym. The
    /// answer for Git commit-author metadata: commits **pseudonymous-by-default** (GIT-1, the
    /// commit-time prerequisite → P-GA-18) so the immutable hash never bakes in erasable PII.
    /// (Consumed contract 4.8, referenced — Identity owns the map.)
    PseudonymMapShred,
    /// **`restrict` suppression** (contract 10.1 / 1.4). Every store auto-registers as a
    /// `PersonalDataHolder`; `restrict` suppresses indexing / agent-use / analytics / notification
    /// for a subject pending erasure. "We forgot a store" is structurally impossible (the
    /// `no-untagged-personal-data` lint + harness auto-registration). The standing guarantee that
    /// also covers the residual (§7.3) — a restricted subject's residual is never processed.
    RestrictSuppression,
}

impl StructuralLever {
    /// The three levers, in the §7.1 order (DEK shred, pseudonym-map shred, restrict).
    #[must_use]
    pub const fn all() -> [StructuralLever; 3] {
        [
            StructuralLever::PerSubjectDekShred,
            StructuralLever::PseudonymMapShred,
            StructuralLever::RestrictSuppression,
        ]
    }

    /// The consumed contract this lever references (4.8 / 11.4) or the owned holder contract (10.1)
    /// — none restated, each referenced (§7.4 / the prompt's CONTRACTS: consumed 4.8, 11.4).
    #[must_use]
    pub const fn referenced_contract(self) -> &'static str {
        match self {
            StructuralLever::PerSubjectDekShred => "11.4",
            StructuralLever::PseudonymMapShred => "4.8",
            StructuralLever::RestrictSuppression => "10.1",
        }
    }

    /// A one-line statement of the lever (for the rendered artifact).
    #[must_use]
    pub const fn statement(self) -> &'static str {
        match self {
            StructuralLever::PerSubjectDekShred =>
                "per-subject DEK crypto-shred renders self-authored free-text unrecoverable (DBs + backups + immutable logs)",
            StructuralLever::PseudonymMapShred =>
                "pseudonym-map shred leaves immutable bytes holding only <pseudonym>@<tenant>.noreply",
            StructuralLever::RestrictSuppression =>
                "restrict suppresses indexing / agent-use / analytics / notification for a subject pending erasure",
        }
    }
}

// ───────────────────────── the canonical posture artifact (10.9) ─────────────────────────

/// The ONE free-text / immutable-content erasure posture — the single canonical artifact
/// (contract 10.9). One in-process instance ([`CANONICAL_POSTURE`]); every subsystem references it,
/// never restates it (§7.4). The structural floor ships regardless; the residual is
/// `[OPEN — LEGAL]` until the DPO ratifies (§7.3) — ONE statement, named not pretended-solved.
#[derive(Debug, Clone)]
pub struct ErasurePosture {
    /// The canonical anchor the per-subsystem instances cite ([`POSTURE_ANCHOR`]).
    pub anchor: &'static str,
    /// The contract-index row owned ([`POSTURE_CONTRACT_ROW`] = `10.9`).
    pub contract_row: &'static str,
    /// The three structural-floor levers (§7.1), in canonical order — fully built, no legal
    /// dependency; the PROOF is P-GA-17 → P-117.
    pub structural_floor: [StructuralLever; 3],
    /// The residual statement (§7.2): third-party / immutable free-text PII typed by *someone
    /// else*, encrypted under the **author's** DEK not the subject's — so the subject's erasure
    /// does NOT crypto-shred it (shredding the author's DEK would destroy the author's legitimate
    /// content). The documented limit, not a defect.
    pub residual: &'static str,
    /// The ratified engineering posture for the residual (§7.3): the documented lawful-basis limit
    /// + best-effort `rectify`/tombstone of the specific span + the standing `restrict` guarantee.
    pub residual_posture: &'static str,
    /// The legal-ratification status of the residual (§7.3). `[OPEN — LEGAL]` — ONE statement, not
    /// five. The structural floor ships regardless.
    pub legal_status: LegalStatus,
}

/// The ONE canonical posture instance (contract 10.9). The single source the M3/M4 subsystem
/// instances reference — never restate (§7.4).
pub const CANONICAL_POSTURE: ErasurePosture = ErasurePosture {
    anchor: POSTURE_ANCHOR,
    contract_row: POSTURE_CONTRACT_ROW,
    structural_floor: StructuralLever::all(),
    residual:
        "third-party / immutable free-text PII typed by someone else into that other person's \
         content (a chat body, an issue comment, a doc block, a CI log line, a commit message by a \
         different author), encrypted under the AUTHOR's DEK not the subject's — the subject's \
         erasure does not crypto-shred it (shredding the author's DEK would destroy the author's \
         legitimate content)",
    residual_posture:
        "handled under a documented lawful-basis limit: best-effort on-request redaction (a \
         targeted rectify / tombstone of the specific span the subject identifies) PLUS the \
         standing structural guarantee that the residual is never indexed, never agent-readable, \
         never in analytics for a restricted subject (the restrict suppression). For git history: \
         (a) the pseudonymous-by-default floor covers author identity, and (b) the audited, \
         tamper-evident, rate-limited history-rewrite erasure path (§6.6) covers the rare body \
         expunge, with the understood disruptive changed-hash consequence",
    legal_status: LegalStatus::OpenLegal,
};

impl ErasurePosture {
    /// Render the canonical artifact as the doc text it GENERATES (the prompt: a documented module
    /// plus the doc text it generates). This is the single source the per-subsystem instances cite
    /// by [`POSTURE_ANCHOR`]; no subsystem restates it. Stable, deterministic text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# The ONE free-text / immutable-content erasure posture (X-7 / OQ-G) — contract ",
        );
        out.push_str(self.contract_row);
        let tag = self.legal_status.tag();
        if !tag.is_empty() {
            out.push(' ');
            out.push_str(tag);
        }
        out.push_str("\n\nCanonical anchor: ");
        out.push_str(self.anchor);
        out.push_str(
            "\n\nThis is ONE posture, instantiated per subsystem BY REFERENCE (§7.4). No \
             subsystem doc restates it; each cites the anchor above.\n",
        );

        out.push_str("\n## Structural floor (built now, no legal dependency — §7.1)\n");
        for (i, lever) in self.structural_floor.iter().enumerate() {
            out.push_str(&format!(
                "{}. {} [references contract {}]\n",
                i + 1,
                lever.statement(),
                lever.referenced_contract(),
            ));
        }

        out.push_str("\n## Residual (the part the floor does NOT erase — for counsel, §7.2)\n");
        out.push_str(self.residual);
        out.push('\n');

        out.push_str("\n## Ratified engineering posture (defensible; FLAG FOR COUNSEL — §7.3) ");
        out.push_str(self.legal_status.tag());
        out.push('\n');
        out.push_str(self.residual_posture);
        out.push('\n');
        out.push_str(
            "\nWhat counsel must ratify (ONE statement, not five): the residual lawful basis; the \
             Art. 17 reach into immutable git bytes; the history-rewrite-vs-documented-limit \
             choice; the audit-log retention carve-out; and the worklog-sensitivity \
             classification. The DPO ratifies; the structural floor ships regardless.\n",
        );
        out
    }

    /// The structural floor is built regardless of legal ratification (§7.1 / §7.3) — `true`
    /// always; the legal status governs only the *publication-as-ratified* of the residual, never
    /// whether the floor ships. Encodes the doctrine "the structural floor ships regardless".
    #[must_use]
    pub const fn structural_floor_ships(&self) -> bool {
        true
    }
}

// ───────────────────── the per-subsystem reference scaffolding (the GATE) ─────────────────────

/// A subsystem's erasure-section instantiation of the posture — BY REFERENCE (§7.4). A subsystem
/// (Git / CI / Issues / Knowledge / Chat) registers ONE of these when it reaches its erasure
/// section; the architecture-test predicate [`reference_is_by_reference`] asserts it **cites the
/// canonical anchor and adds no restated posture text**.
///
/// The scaffolding lands here (P-GA-16); the first real register is Git (P-GA-28, the consumer half
/// of the 10.9 CDC pair). The M1 unit tests exercise the predicate against an in-module exemplar.
#[derive(Debug, Clone)]
pub struct SubsystemReference {
    /// The subsystem name (e.g. `"git"`, `"chat"`, `"ci"`).
    pub subsystem: &'static str,
    /// The anchor the subsystem's erasure section CITES — must equal [`POSTURE_ANCHOR`] for the
    /// reference to be valid (a by-reference instantiation, not a restatement).
    pub cited_anchor: &'static str,
    /// The subsystem's own erasure-section text. For a valid by-reference instantiation this is the
    /// short "follows the platform posture in <anchor>" form (§7.4) — it must NOT restate the
    /// structural floor / residual / lawful-basis text (those live ONCE in [`CANONICAL_POSTURE`]).
    pub section_text: &'static str,
}

/// The §7.4 by-reference predicate (the GATE's architecture test): a subsystem erasure section is a
/// valid **by-reference** instantiation IFF it cites the canonical anchor AND does not RESTATE the
/// posture. A restatement is detected by the presence of the structural-floor / residual marker
/// phrases that belong ONLY in the ONE canonical artifact — a subsystem section that contains them
/// has restated the posture (the thing X-7 forbids: "five different residual statements instead of
/// one ratified posture").
///
/// This is the predicate the M3/M4 reference assertions fire on. The scaffolding + the predicate
/// are complete and tested here; the real subsystem registers land in P-GA-28 (Git) / P-GA-29/-31.
#[must_use]
pub fn reference_is_by_reference(r: &SubsystemReference) -> bool {
    // (1) It must cite the ONE canonical anchor.
    if r.cited_anchor != POSTURE_ANCHOR {
        return false;
    }
    // (2) It must NOT restate the posture — a by-reference instantiation says only "follows the
    //     platform posture in <anchor>", never the structural-floor / residual text itself. These
    //     marker phrases are the load-bearing canonical sentences; their presence in a *subsystem*
    //     section means the posture was restated (the X-7 anti-pattern).
    let restatement_markers = restatement_markers();
    let lowered = r.section_text.to_ascii_lowercase();
    !restatement_markers
        .iter()
        .any(|m| lowered.contains(&m.to_ascii_lowercase()))
}

/// The canonical marker phrases that may appear ONLY in the ONE artifact ([`CANONICAL_POSTURE`] /
/// [`ErasurePosture::render`]) — never in a subsystem's by-reference section. A subsystem section
/// containing any of these has RESTATED the posture (the X-7 anti-pattern). Returned as a set so
/// the architecture test can prove each marker is present in the canonical render (the markers are
/// real, load-bearing canonical text) and absent from every by-reference section.
#[must_use]
pub fn restatement_markers() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "per-subject DEK crypto-shred",
        "documented lawful-basis limit",
        "What counsel must ratify",
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical artifact is a SINGLE source: exactly one in-process instance, owning row 10.9,
    /// citing the one anchor, with the three §7.1 levers in canonical order.
    #[test]
    fn the_posture_is_the_one_canonical_artifact() {
        assert_eq!(CANONICAL_POSTURE.contract_row, "10.9");
        assert_eq!(CANONICAL_POSTURE.anchor, POSTURE_ANCHOR);
        assert_eq!(
            CANONICAL_POSTURE.structural_floor,
            [
                StructuralLever::PerSubjectDekShred,
                StructuralLever::PseudonymMapShred,
                StructuralLever::RestrictSuppression,
            ],
            "the three structural-floor levers in the §7.1 order"
        );
    }

    /// The residual is documented as correctly NOT crypto-shredded by the subject's key (the
    /// documented limit, §7.2) — the author's-DEK-encrypted third-party mention. Stated as a limit,
    /// never pretended-solved.
    #[test]
    fn the_residual_is_the_documented_author_dek_limit() {
        assert!(
            CANONICAL_POSTURE.residual.contains("AUTHOR's DEK")
                && CANONICAL_POSTURE.residual.contains("not the subject's"),
            "the residual is third-party PII under the AUTHOR's DEK — not shreddable by the subject's key"
        );
    }

    /// The residual lawful-basis ratification is `[OPEN — LEGAL]` — ONE statement, named not gated;
    /// the structural floor ships regardless.
    #[test]
    fn the_residual_is_open_legal_and_the_floor_ships_regardless() {
        assert_eq!(CANONICAL_POSTURE.legal_status, LegalStatus::OpenLegal);
        assert_eq!(CANONICAL_POSTURE.legal_status.tag(), "[OPEN — LEGAL]");
        assert!(
            CANONICAL_POSTURE.structural_floor_ships(),
            "the structural floor ships regardless of legal ratification (§7.1 / §7.3)"
        );
        // Once ratified the tag is empty (the parallel-legal track flips it; no shape change).
        assert_eq!(LegalStatus::Ratified.tag(), "");
    }

    /// The rendered artifact is the single source text: it cites the anchor, states the three
    /// levers, the residual, and the `[OPEN — LEGAL]` lawful-basis limit — ONCE.
    #[test]
    fn render_is_the_single_source_text() {
        let doc = CANONICAL_POSTURE.render();
        assert!(
            doc.contains(POSTURE_ANCHOR),
            "the render cites the canonical anchor"
        );
        assert!(
            doc.contains("[OPEN — LEGAL]"),
            "the render carries the one [OPEN — LEGAL] tag"
        );
        assert!(
            doc.to_ascii_lowercase().contains("by reference"),
            "the render states the by-reference instantiation rule"
        );
        // Every structural-floor lever statement appears once.
        for lever in StructuralLever::all() {
            assert!(
                doc.contains(lever.statement()),
                "the render states lever {lever:?}"
            );
        }
        // The marker phrases are real, load-bearing canonical text (present in the ONE artifact).
        for marker in restatement_markers() {
            assert!(
                doc.contains(marker),
                "marker {marker:?} is canonical text in the render"
            );
        }
    }

    /// **The GATE (architecture-test scaffolding): a subsystem reference is a valid by-reference
    /// instantiation** — it cites the anchor and does NOT restate the posture (§7.4). This is the
    /// shape the M3/M4 subsystem instances register; the assertion FIRES fully when P-GA-28 (Git)
    /// and P-GA-29/-31 register their real references. Exercised here against an exemplar.
    #[test]
    fn a_valid_subsystem_reference_cites_the_anchor_and_does_not_restate() {
        // The §7.4 short form: "follows the platform posture in <anchor>" — by reference.
        let git_like = SubsystemReference {
            subsystem: "git",
            cited_anchor: POSTURE_ANCHOR,
            section_text: "Free-text / immutable-content erasure follows the platform posture in \
                 00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). \
                 Git commits are pseudonymous-by-default; the immutable hash holds only the \
                 pseudonym form.",
        };
        assert!(
            reference_is_by_reference(&git_like),
            "a by-reference section that cites the anchor and adds no restated posture text is valid"
        );
    }

    /// A subsystem section that RESTATES the posture (contains a canonical marker phrase) is
    /// REJECTED — the X-7 anti-pattern ("five different residual statements instead of one ratified
    /// posture"). The gate forbids restatement.
    #[test]
    fn a_restating_subsystem_reference_is_rejected() {
        let restating = SubsystemReference {
            subsystem: "chat",
            cited_anchor: POSTURE_ANCHOR,
            // Restates the structural floor — the forbidden duplication.
            section_text:
                "Chat erasure: per-subject DEK crypto-shred renders self-authored messages \
                 unrecoverable; the documented lawful-basis limit covers third-party mentions ...",
        };
        assert!(
            !reference_is_by_reference(&restating),
            "a section that restates the posture (a canonical marker phrase) is rejected"
        );
    }

    /// A reference that cites the WRONG anchor (not the ONE canonical source) is rejected — the
    /// posture is a single source, every reference points at the same anchor.
    #[test]
    fn a_reference_to_the_wrong_anchor_is_rejected() {
        let wrong = SubsystemReference {
            subsystem: "issues",
            cited_anchor: "some-other-doc.md §99",
            section_text: "follows the platform posture in some-other-doc.md §99",
        };
        assert!(
            !reference_is_by_reference(&wrong),
            "a reference that does not cite the ONE canonical anchor is rejected"
        );
    }
}
