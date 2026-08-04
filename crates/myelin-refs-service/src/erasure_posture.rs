#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErasurePosture {
    pub origin_actor_is_pseudonymous: bool,
    pub holds_no_free_text_bodies: bool,
    pub instantiates_x7_by_reference: bool,
    pub adds_no_new_open_legal_residual: bool,
}

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

    #[test]
    fn refs_adds_no_new_open_legal_residual() {
        let p = erasure_posture();
        assert!(
            p.origin_actor_is_pseudonymous,
            "origin_actor is an opaque pseudonym (§3.2/§4.6)"
        );
        assert!(
            p.holds_no_free_text_bodies,
            "references-not-payloads - no free-text bodies in Refs"
        );
        assert!(
            p.instantiates_x7_by_reference,
            "Refs uses the ONE platform posture (10.9), not a 2nd"
        );
        assert!(
            p.adds_no_new_open_legal_residual,
            "no new [OPEN - LEGAL] residual"
        );
    }
}
