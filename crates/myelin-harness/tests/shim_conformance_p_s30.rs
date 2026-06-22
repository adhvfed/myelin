//! # The cross-language harness shim — enforcement / loud-N/A gate (P-S30 / global P-319)
//!
//! **Contract:** `contract-index.md` row 1.7 (the cross-language harness shim — the contract a
//! NON-Rust subsystem must satisfy; owner = the diverging subsystem, "Chat connection tier
//! likely, TE-21"). **Architecture:** `00-platform-substrate.md` §3.7 (the frozen divergence
//! contract — the SEVEN non-negotiables, to the SAME guarantee the Rust harness provides).
//! **Doctrine:** EI-01 §5 (*an uncommitted gate is no gate* — the shim cannot be quietly
//! dropped at a language boundary) + §4 (*an N/A is recorded loudly, never silently skipped*).
//!
//! ## What this gate asserts
//! This is the GATE for P-S30. The mechanism (the seven-non-negotiable conformance suite +
//! the loud enforcement/N/A record) lives in `myelin_harness::cross_language_shim` and is
//! unit-tested there with a fake divergent-tier probe. THIS test wires the mechanism to chat's
//! REAL TE-21 connection-tier pin (`myelin_chat::glue`) and records the discharge:
//!
//! - **Today (the N/A path):** chat's TE-21 pin is **Rust** — the BEAM/Phoenix hatch is
//!   written-but-CLOSED. There is therefore NO cross-language boundary, so the shim is a
//!   NO-OP. The gate is a **loudly-recorded, dated N/A** ([`ShimEnforcement::recorded_na`]) —
//!   NOT a silent skip. The dated artifact row is printed (the committed N/A row).
//! - **The CDC pair for 1.7:** PROVIDER = chat pins the language (`te21_harness_shim_obligation`
//!   returns `Rust`); CONSUMER = the harness shim confirms the obligation is a NO-OP in the
//!   all-Rust default. (This mirrors `myelin-chat/tests/cdc_1_7_chat_te21_pin.rs`, asserting the
//!   SAME seam from the harness/consumer side so a divergence flips BOTH sides at once.)
//! - **When chat diverges (CHAT-P26):** the suite BINDS — see the `the_suite_binds_*` tests,
//!   which prove that a BEAM tier MUST satisfy all seven clauses to enforce green, and that the
//!   gate flips from the N/A path to the enforcement path automatically off the pin value.
//!
//! `shim-conformance` ×7 is green in the divergent tier (only if chat diverged); in the
//! all-Rust default the gate is the loudly-recorded N/A. Either way it is COMMITTED — the shim
//! cannot be quietly dropped.

use myelin_chat::glue::{te21_harness_shim_obligation, Te21LanguagePin};
use myelin_harness::cross_language_shim::{
    DivergentTierProbe, Nonnegotiable, ShimConformance, ShimEnforcement,
};

/// Today's date as ISO-8601 — reused from the harness so the N/A row is dated consistently.
fn today() -> String {
    myelin_harness::scorecard::today_iso()
}

/// **The P-S30 GATE — discharge the 1.7 shim against chat's REAL pin.** Reads chat's TE-21 pin
/// and produces the shim-enforcement record: a loudly-recorded N/A in the all-Rust default, or
/// (if chat ever diverges) the enforcement path. This is the committed gate artifact.
fn discharge_shim_against_chat_pin() -> ShimEnforcement {
    let pin = te21_harness_shim_obligation();
    match pin {
        // The all-Rust default: no cross-language boundary → the shim is a NO-OP, recorded
        // LOUDLY (dated, with the pinned-language reason). NOT a silent skip.
        Te21LanguagePin::Rust => ShimEnforcement::recorded_na(
            "chat-connection-tier",
            "Rust (TE-21 pin at M2-C0; BEAM/Phoenix hatch written-but-closed, CHAT-P26)",
            today(),
        ),
        // The divergent path (CHAT-P26): the BEAM tier's conformance probe MUST be supplied and
        // pass all seven clauses. There is no probe yet (the BEAM tier does not exist); when it
        // is selected, the rewrite prompt supplies `impl DivergentTierProbe` and enforces here.
        // We make the unreachable-today branch LOUD rather than a silent `unreachable!()`:
        Te21LanguagePin::Beam => panic!(
            "chat diverged to BEAM (TE-21) but no DivergentTierProbe was supplied — the \
             cross-language harness shim (contract 1.7 / §3.7) MUST be enforced against the \
             BEAM connection tier (all seven non-negotiables); the shim cannot be quietly \
             dropped at the language boundary (EI-01 §5). Supply the probe at CHAT-P26."
        ),
    }
}

/// THE GATE: discharge the 1.7 shim against chat's real pin. Today this is the loud N/A path.
/// The test asserts the discharge is RECORDED (the `#[must_use]` enforcement record is
/// consumed) and prints the dated artifact row — the committed, loud N/A (EI-01 §4/§5).
#[test]
fn p_s30_gate_shim_discharged_against_chat_real_pin() {
    let enforcement = discharge_shim_against_chat_pin();

    // Today chat is Rust → the discharge is a loudly-recorded N/A, not an enforcement.
    assert!(
        enforcement.is_na(),
        "chat's TE-21 pin is Rust today; the 1.7 shim is a NO-OP recorded as a loud N/A"
    );
    assert!(!enforcement.is_enforced());

    let row = enforcement.artifact_row();
    // The loudness invariant: the row names the contract, the pinned language, the date, and
    // explicitly states it is NOT a silent skip.
    assert!(row.contains("contract 1.7"));
    assert!(row.contains("chat-connection-tier"));
    assert!(row.contains("Rust"));
    assert!(row.contains("NO-OP"));
    assert!(row.contains("NOT a silent skip"));
    assert!(row.contains(&today()));

    // Emit the committed artifact row (loud, never swallowed).
    println!("P-S30 shim-conformance artifact: {row}");
}

/// **CDC pair for 1.7 (harness/consumer side).** PROVIDER = chat pins the language (Rust);
/// CONSUMER = the harness shim confirms the all-Rust default makes the shim a NO-OP. This pins
/// the SAME seam `myelin-chat/tests/cdc_1_7_chat_te21_pin.rs` pins, from the consumer side, so
/// a future divergence cannot satisfy one side while silently regressing the other.
#[test]
fn cdc_1_7_provider_pins_rust_consumer_shim_is_a_no_op() {
    // PROVIDER side: chat pins Rust.
    let pin = te21_harness_shim_obligation();
    assert_eq!(pin, Te21LanguagePin::Rust, "the M2-C0 TE-21 pin is Rust");
    assert_eq!(Te21LanguagePin::PINNED, Te21LanguagePin::Rust);

    // CONSUMER side: the harness shim's obligation is a NO-OP in the all-Rust default — i.e. the
    // discharge is the loud N/A, not an enforced divergent tier.
    let enforcement = discharge_shim_against_chat_pin();
    assert!(
        enforcement.is_na(),
        "the all-Rust default makes the 1.7 harness shim a NO-OP (loud N/A)"
    );
}

/// A fully-conformant BEAM tier (the CHAT-P26 success path): supplies the probe, passes all
/// seven §3.7 non-negotiables, and ENFORCES green. Proves the suite BINDS on divergence — the
/// gate is not a permanent no-op; it has real teeth the moment a non-Rust tier appears.
struct ConformantBeamTier;
impl DivergentTierProbe for ConformantBeamTier {
    fn tier_name(&self) -> &str {
        "chat-beam-connection-tier"
    }
    // A real probe runs a conformance test per clause against the running BEAM tier; here we
    // model the success path (every clause provided to the same guarantee).
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

/// A BEAM tier that drops a non-negotiable (e.g. keeps a fire-and-forget publish path) CANNOT
/// enforce green — `enforce` returns the red verdict naming the dropped clause. Proves the shim
/// cannot be quietly dropped at the language boundary (EI-01 §5): the divergent tier ships only
/// when all seven are green.
struct NonConformantBeamTier;
impl DivergentTierProbe for NonConformantBeamTier {
    fn tier_name(&self) -> &str {
        "chat-beam-connection-tier"
    }
    fn probe(&self, n: Nonnegotiable) -> bool {
        // Drops §3.7.3 (no-fire-and-forget) — the BUS-2 shortcut that "exists will be used".
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
