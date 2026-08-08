use myelin_chat::glue::{te21_harness_shim_obligation, Te21LanguagePin};
use myelin_harness::cross_language_shim::{
    DivergentTierProbe, Nonnegotiable, ShimConformance, ShimEnforcement,
};

fn today() -> String {
    myelin_harness::today_iso()
}

fn discharge_shim_against_chat_pin() -> ShimEnforcement {
    let pin = te21_harness_shim_obligation();
    match pin {
        Te21LanguagePin::Rust => ShimEnforcement::recorded_na(
            "chat-connection-tier",
            "Rust (TE-21 pin at M2-C0; BEAM/Phoenix hatch written-but-closed, CHAT-P26)",
            today(),
        ),
        Te21LanguagePin::Beam => panic!(
            "chat diverged to BEAM (TE-21) but no DivergentTierProbe was supplied - the \
             cross-language harness shim (contract 1.7 / §3.7) MUST be enforced against the \
             BEAM connection tier (all seven non-negotiables); the shim cannot be quietly \
             dropped at the language boundary (EI-01 §5). Supply the probe at CHAT-P26."
        ),
    }
}

#[test]
fn p_s30_gate_shim_discharged_against_chat_real_pin() {
    let enforcement = discharge_shim_against_chat_pin();

    assert!(
        enforcement.is_na(),
        "chat's TE-21 pin is Rust today; the 1.7 shim is a NO-OP recorded as a loud N/A"
    );
    assert!(!enforcement.is_enforced());

    let row = enforcement.artifact_row();
    assert!(row.contains("contract 1.7"));
    assert!(row.contains("chat-connection-tier"));
    assert!(row.contains("Rust"));
    assert!(row.contains("NO-OP"));
    assert!(row.contains("NOT a silent skip"));
    assert!(row.contains(&today()));

    println!("P-S30 shim-conformance artifact: {row}");
}

#[test]
fn cdc_1_7_provider_pins_rust_consumer_shim_is_a_no_op() {
    let pin = te21_harness_shim_obligation();
    assert_eq!(pin, Te21LanguagePin::Rust, "the M2-C0 TE-21 pin is Rust");
    assert_eq!(Te21LanguagePin::PINNED, Te21LanguagePin::Rust);

    let enforcement = discharge_shim_against_chat_pin();
    assert!(
        enforcement.is_na(),
        "the all-Rust default makes the 1.7 harness shim a NO-OP (loud N/A)"
    );
}

struct ConformantBeamTier;
impl DivergentTierProbe for ConformantBeamTier {
    fn tier_name(&self) -> &str {
        "chat-beam-connection-tier"
    }
    fn probe(&self, _n: Nonnegotiable) -> bool {
        true
    }
}

#[test]
fn the_suite_binds_a_conformant_beam_tier_enforces_green() {
    let conformance = ShimConformance::check(&ConformantBeamTier);
    assert!(conformance.all_green(), "all seven §3.7 clauses pass");
    assert_eq!(conformance.results().len(), 7);

    let enforced =
        ShimEnforcement::enforce(&ConformantBeamTier).expect("a conformant BEAM tier enforces");
    assert!(enforced.is_enforced());
    assert!(enforced.artifact_row().contains("×7 GREEN"));
}

struct NonConformantBeamTier;
impl DivergentTierProbe for NonConformantBeamTier {
    fn tier_name(&self) -> &str {
        "chat-beam-connection-tier"
    }
    fn probe(&self, n: Nonnegotiable) -> bool {
        n != Nonnegotiable::NoFireAndForgetEmit
    }
}

#[test]
fn the_suite_binds_a_non_conformant_beam_tier_fails_loudly() {
    let err = ShimEnforcement::enforce(&NonConformantBeamTier)
        .expect_err("a tier dropping a non-negotiable cannot enforce green");
    assert_eq!(err.failures(), vec![Nonnegotiable::NoFireAndForgetEmit]);
    assert_eq!(Nonnegotiable::NoFireAndForgetEmit.clause(), 3);
}
