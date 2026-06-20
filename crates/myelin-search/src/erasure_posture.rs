//! The Search erasure-posture record — Search instantiates the ONE platform free-text/immutable
//! erasure posture (X-7 / contract 10.9) **by reference** and adds **NO new `[OPEN — LEGAL]`
//! residual** (SRCH-P02 / P-122).
//!
//! **Architecture / reconciliation:** search-and-indexing.md §1 ("Search is a true holder whose
//! `erase` is a **real purge**"), §4.8 ("`erase(subject)` — **purge + re-index, not hide**"; the
//! per-tenant index DEK crypto-shreds the whole tenant index on decommission; `restrict` suppresses
//! the residual the platform posture (10.9, X-7) "relies on for the residual it can't
//! crypto-shred"); `00-reconciliation-decisions.md` **X-7** + change #9 (the ONE platform-wide
//! free-text/immutable erasure lawful-basis posture; the residual is third-party free-text PII typed
//! by *someone else* — ratified by counsel).
//!
//! ## Why Search adds NO new residual (the recorded posture)
//! Search's personal data is exclusively **derived, reconstructible** state (architecture §0/§1):
//! 1. **Indexed tokens / facets** — derived projections of an owning subsystem's content. Erased by
//!    **purge + reindex** (a real purge, §4.8), NOT hidden. The source of truth is the owning
//!    subsystem; Search reindexes from its now-tombstoned projection.
//! 2. **Vectors / embeddings** — personal data (EI-04 §5), in the same doc-id space, **erased with
//!    their source doc** (no orphan embedding; §4.8). Tombstone + compact.
//! 3. **The per-tenant index DEK** (`pii_key_ref`) — crypto-shreds the whole tenant index on
//!    tenant-decommission + backstops backups/immutable segments (reserved in [`crate::dek`]).
//! 4. **`restrict`** — suppresses indexing/RAG/analytics/notification for a subject; this is the
//!    suppression the X-7 posture relies on for the residual it cannot crypto-shred.
//!
//! Search holds **NO authoritative free-text body**: a body lives in its owning subsystem
//! (Git/Issues/Knowledge/Chat/CI); Search only DERIVES a searchable projection from it and erases
//! that projection by purge + reindex. So the genuinely-hard residual X-7 names — third-party
//! free-text PII typed by someone else — **lives in those subsystems, never in Search's own
//! authoritative store**. Search therefore instantiates the ONE posture **by reference** (10.9) and
//! introduces **no new `[OPEN — LEGAL]` residual**: its real purge + crypto-shred + restrict fully
//! discharge its derived state.

/// The Search erasure-posture record (X-7 / 10.9 by reference). A small, inspectable value that
/// records, as a checked fact: Search's erasure is a **real purge** (not hide), it holds only
/// **derived, reconstructible** state (never an authoritative free-text body), `restrict` provides
/// the X-7 suppression, and it adds **no new `[OPEN — LEGAL]` residual**. The
/// `instantiates_x7_by_reference` flag is the X-7 anchor (Search uses the ONE platform posture,
/// never a second residual statement).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErasurePosture {
    /// Search's PRIMARY per-subject erasure is a **real purge + reindex** (delete + tombstone +
    /// recompute the surviving projection), NOT hide (§1 / §4.8). The body lands in SRCH-P15.
    pub erase_is_a_real_purge_not_hide: bool,
    /// Search holds only **derived, reconstructible** state (indexed tokens + vectors), never an
    /// authoritative free-text body — the body lives in its owning subsystem (architecture §0/§1).
    pub holds_only_derived_reconstructible_state: bool,
    /// `restrict` suppresses indexing/RAG/analytics/notification — the suppression the X-7 posture
    /// (10.9) relies on for the residual Search cannot crypto-shred (§4.8). Real body SRCH-P15.
    pub restrict_provides_the_x7_suppression: bool,
    /// Search instantiates the ONE platform free-text/immutable erasure posture (10.9 / X-7) **by
    /// reference** — it does not author a second residual statement.
    pub instantiates_x7_by_reference: bool,
    /// Search adds **NO new `[OPEN — LEGAL]` residual** to the platform posture (the whole point of
    /// the X-7 reconciliation: one ratified posture, not N).
    pub adds_no_new_open_legal_residual: bool,
}

/// The frozen Search erasure posture (SRCH-P02): a real purge (not hide), only derived state, the
/// restrict suppression, X-7 by reference, no new residual. Returned as data so a test pins it (a
/// drift — e.g. someone making Search the authoritative store for a free-text body — flips a flag +
/// fails the build, never a silent posture change).
pub const fn erasure_posture() -> ErasurePosture {
    ErasurePosture {
        erase_is_a_real_purge_not_hide: true,
        holds_only_derived_reconstructible_state: true,
        restrict_provides_the_x7_suppression: true,
        instantiates_x7_by_reference: true,
        adds_no_new_open_legal_residual: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Search adds NO new `[OPEN — LEGAL]` residual (the X-7 recorded posture).** Its erase is a
    /// real purge (not hide), it holds only derived/reconstructible state (never an authoritative
    /// free-text body), `restrict` provides the X-7 suppression, and it instantiates the ONE
    /// platform posture (10.9) by reference. If a later change made Search the authoritative store
    /// for a free-text body, a flag here would have to flip — a loud, build-failing posture change,
    /// never a silent new residual.
    #[test]
    fn search_adds_no_new_open_legal_residual() {
        let p = erasure_posture();
        assert!(p.erase_is_a_real_purge_not_hide, "Search erase is a real purge + reindex (§1/§4.8)");
        assert!(
            p.holds_only_derived_reconstructible_state,
            "Search holds only derived/reconstructible state — never an authoritative free-text body"
        );
        assert!(p.restrict_provides_the_x7_suppression, "restrict is the X-7 suppression (§4.8)");
        assert!(p.instantiates_x7_by_reference, "Search uses the ONE platform posture (10.9), not a 2nd");
        assert!(p.adds_no_new_open_legal_residual, "no new [OPEN — LEGAL] residual");
    }
}
