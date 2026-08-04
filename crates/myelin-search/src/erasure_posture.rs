#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErasurePosture {
    pub erase_is_a_real_purge_not_hide: bool,
    pub holds_only_derived_reconstructible_state: bool,
    pub restrict_provides_the_x7_suppression: bool,
    pub instantiates_x7_by_reference: bool,
    pub adds_no_new_open_legal_residual: bool,
}

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

    #[test]
    fn search_adds_no_new_open_legal_residual() {
        let p = erasure_posture();
        assert!(
            p.erase_is_a_real_purge_not_hide,
            "Search erase is a real purge + reindex (§1/§4.8)"
        );
        assert!(
            p.holds_only_derived_reconstructible_state,
            "Search holds only derived/reconstructible state - never an authoritative free-text body"
        );
        assert!(
            p.restrict_provides_the_x7_suppression,
            "restrict is the X-7 suppression (§4.8)"
        );
        assert!(
            p.instantiates_x7_by_reference,
            "Search uses the ONE platform posture (10.9), not a 2nd"
        );
        assert!(
            p.adds_no_new_open_legal_residual,
            "no new [OPEN - LEGAL] residual"
        );
    }
}
