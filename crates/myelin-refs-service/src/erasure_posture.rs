//! The Refs erasure-posture record — Refs instantiates the ONE platform free-text/immutable
//! erasure posture (X-7 / contract 10.9) **by reference** and adds **NO new `[OPEN — LEGAL]`
//! residual** (REF-P3 / P-120).
//!
//! **Architecture / reconciliation:** reference-graph.md §4.6 ("Refs **never holds the PII itself**
//! for the references-not-payloads case — its erasure surface is small and structural. This is the
//! platform free-text/immutable erasure posture …"); `00-reconciliation-decisions.md` **X-7** (the
//! ONE platform-wide free-text/immutable erasure lawful-basis posture; the residual is third-party
//! free-text PII typed by *someone else* — the part the floor does NOT erase, ratified by counsel).
//!
//! ## Why Refs adds NO new residual (the recorded posture)
//! Refs' only personal data is:
//! 1. **`origin_actor`** — a stable, opaque **pseudonymous** Principal id (§3.2; EI-04 §1). Erasing
//!    the *person* needs **no edge mutation**: Identity's pseudonym-map shred (contract 4.8) makes
//!    the id unresolvable to a human. The edge keeps the opaque id; the human becomes unrenderable.
//! 2. **R2 projection cache titles** — derived, reconstructible render hints (§3.6), purged on
//!    erase (REF-P15) + crypto-shred-able under the per-tenant DEK (REF-P4).
//!
//! Refs holds **no third-party free-text bodies** (references-not-payloads): a body lives in its
//! authoritative subsystem (Git/Issues/Knowledge/Chat), and Refs degrades a denied/erased reference
//! to a tombstone (§4.6). So the genuinely-hard residual X-7 names — third-party free-text PII typed
//! by someone else — **lives in those subsystems, never in Refs**. Refs therefore instantiates the
//! ONE posture **by reference** (10.9) and introduces **no new `[OPEN — LEGAL]` residual**.

/// The Refs erasure-posture record (X-7 / 10.9 by reference). A small, inspectable value that
/// records, as a checked fact: Refs' erasure surface is **small + structural** (pseudonymous ids +
/// derived cache titles only), it holds **no third-party free-text bodies**, and it adds **no new
/// `[OPEN — LEGAL]` residual**. The `instantiates_by_reference` flag is the X-7 anchor (Refs uses
/// the ONE platform posture, never a second residual statement).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErasurePosture {
    /// Refs' personal data is only pseudonymous opaque ids (`origin_actor`) — erased via Identity's
    /// pseudonym-map shred (contract 4.8), no edge mutation needed (§4.6).
    pub origin_actor_is_pseudonymous: bool,
    /// Refs holds NO third-party free-text bodies (references-not-payloads — the body lives in its
    /// authoritative subsystem; Refs degrades to a tombstone, §4.6).
    pub holds_no_free_text_bodies: bool,
    /// Refs instantiates the ONE platform free-text/immutable erasure posture (10.9 / X-7) **by
    /// reference** — it does not author a second residual statement.
    pub instantiates_x7_by_reference: bool,
    /// Refs adds **NO new `[OPEN — LEGAL]` residual** to the platform posture (the whole point of
    /// the X-7 reconciliation: one ratified posture, not N).
    pub adds_no_new_open_legal_residual: bool,
}

/// The frozen Refs erasure posture (REF-P3): small + structural surface, no free-text bodies, X-7
/// by reference, no new residual. Returned as data so a test pins it (a drift — e.g. someone making
/// Refs hold a free-text body — flips a flag + fails the build, never a silent posture change).
pub const fn erasure_posture() -> ErasurePosture {
    ErasurePosture {
        origin_actor_is_pseudonymous: true,
        holds_no_free_text_bodies: true,
        instantiates_x7_by_reference: true,
        adds_no_new_open_legal_residual: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Refs adds NO new `[OPEN — LEGAL]` residual (the X-7 recorded posture).** Its erasure
    /// surface is small + structural (pseudonymous ids + derived cache titles), it holds no
    /// third-party free-text bodies, and it instantiates the ONE platform posture (10.9) by
    /// reference. If a later change made Refs hold a free-text body, a flag here would have to flip
    /// — a loud, build-failing posture change, never a silent new residual.
    #[test]
    fn refs_adds_no_new_open_legal_residual() {
        let p = erasure_posture();
        assert!(p.origin_actor_is_pseudonymous, "origin_actor is an opaque pseudonym (§3.2/§4.6)");
        assert!(p.holds_no_free_text_bodies, "references-not-payloads — no free-text bodies in Refs");
        assert!(p.instantiates_x7_by_reference, "Refs uses the ONE platform posture (10.9), not a 2nd");
        assert!(p.adds_no_new_open_legal_residual, "no new [OPEN — LEGAL] residual");
    }
}
