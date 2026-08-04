use std::collections::BTreeSet;

pub const POSTURE_ANCHOR: &str =
    "00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9)";

pub const POSTURE_CONTRACT_ROW: &str = "10.9";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegalStatus {
    OpenLegal,
    Ratified,
}

impl LegalStatus {
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            LegalStatus::OpenLegal => "[OPEN - LEGAL]",
            LegalStatus::Ratified => "",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StructuralLever {
    PerSubjectDekShred,
    PseudonymMapShred,
    RestrictSuppression,
}

impl StructuralLever {
    #[must_use]
    pub const fn all() -> [StructuralLever; 3] {
        [
            StructuralLever::PerSubjectDekShred,
            StructuralLever::PseudonymMapShred,
            StructuralLever::RestrictSuppression,
        ]
    }

    #[must_use]
    pub const fn referenced_contract(self) -> &'static str {
        match self {
            StructuralLever::PerSubjectDekShred => "11.4",
            StructuralLever::PseudonymMapShred => "4.8",
            StructuralLever::RestrictSuppression => "10.1",
        }
    }

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

#[derive(Debug, Clone)]
pub struct ErasurePosture {
    pub anchor: &'static str,
    pub contract_row: &'static str,
    pub structural_floor: [StructuralLever; 3],
    pub residual: &'static str,
    pub residual_posture: &'static str,
    pub legal_status: LegalStatus,
}

pub const CANONICAL_POSTURE: ErasurePosture = ErasurePosture {
    anchor: POSTURE_ANCHOR,
    contract_row: POSTURE_CONTRACT_ROW,
    structural_floor: StructuralLever::all(),
    residual:
        "third-party / immutable free-text PII typed by someone else into that other person's \
         content (a chat body, an issue comment, a doc block, a CI log line, a commit message by a \
         different author), encrypted under the AUTHOR's DEK not the subject's - the subject's \
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
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "# The ONE free-text / immutable-content erasure posture (X-7 / OQ-G) - contract ",
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

        out.push_str("\n## Structural floor (built now, no legal dependency - §7.1)\n");
        for (i, lever) in self.structural_floor.iter().enumerate() {
            out.push_str(&format!(
                "{}. {} [references contract {}]\n",
                i + 1,
                lever.statement(),
                lever.referenced_contract(),
            ));
        }

        out.push_str("\n## Residual (the part the floor does NOT erase - for counsel, §7.2)\n");
        out.push_str(self.residual);
        out.push('\n');

        out.push_str("\n## Ratified engineering posture (defensible; FLAG FOR COUNSEL - §7.3) ");
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

    #[must_use]
    pub const fn structural_floor_ships(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
pub struct SubsystemReference {
    pub subsystem: &'static str,
    pub cited_anchor: &'static str,
    pub section_text: &'static str,
}

#[must_use]
pub fn reference_is_by_reference(r: &SubsystemReference) -> bool {
    if r.cited_anchor != POSTURE_ANCHOR {
        return false;
    }
    let restatement_markers = restatement_markers();
    let lowered = r.section_text.to_ascii_lowercase();
    !restatement_markers
        .iter()
        .any(|m| lowered.contains(&m.to_ascii_lowercase()))
}

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

    #[test]
    fn the_residual_is_the_documented_author_dek_limit() {
        assert!(
            CANONICAL_POSTURE.residual.contains("AUTHOR's DEK")
                && CANONICAL_POSTURE.residual.contains("not the subject's"),
            "the residual is third-party PII under the AUTHOR's DEK - not shreddable by the subject's key"
        );
    }

    #[test]
    fn the_residual_is_open_legal_and_the_floor_ships_regardless() {
        assert_eq!(CANONICAL_POSTURE.legal_status, LegalStatus::OpenLegal);
        assert_eq!(CANONICAL_POSTURE.legal_status.tag(), "[OPEN - LEGAL]");
        assert!(
            CANONICAL_POSTURE.structural_floor_ships(),
            "the structural floor ships regardless of legal ratification (§7.1 / §7.3)"
        );
        assert_eq!(LegalStatus::Ratified.tag(), "");
    }

    #[test]
    fn render_is_the_single_source_text() {
        let doc = CANONICAL_POSTURE.render();
        assert!(
            doc.contains(POSTURE_ANCHOR),
            "the render cites the canonical anchor"
        );
        assert!(
            doc.contains("[OPEN - LEGAL]"),
            "the render carries the one [OPEN - LEGAL] tag"
        );
        assert!(
            doc.to_ascii_lowercase().contains("by reference"),
            "the render states the by-reference instantiation rule"
        );
        for lever in StructuralLever::all() {
            assert!(
                doc.contains(lever.statement()),
                "the render states lever {lever:?}"
            );
        }
        for marker in restatement_markers() {
            assert!(
                doc.contains(marker),
                "marker {marker:?} is canonical text in the render"
            );
        }
    }

    #[test]
    fn a_valid_subsystem_reference_cites_the_anchor_and_does_not_restate() {
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

    #[test]
    fn a_restating_subsystem_reference_is_rejected() {
        let restating = SubsystemReference {
            subsystem: "chat",
            cited_anchor: POSTURE_ANCHOR,
            section_text:
                "Chat erasure: per-subject DEK crypto-shred renders self-authored messages \
                 unrecoverable; the documented lawful-basis limit covers third-party mentions ...",
        };
        assert!(
            !reference_is_by_reference(&restating),
            "a section that restates the posture (a canonical marker phrase) is rejected"
        );
    }

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
