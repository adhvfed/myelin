//! # The CDC pair for contract 1.7 — chat's TE-21 connection-tier language pin (CHAT-P3 / P-245)
//!
//! **Contract:** `contract-index.md` row 1.7 (the cross-language harness shim, frozen — three-surface
//! topology, liveness≠readiness, no-fire-and-forget emit, `PersonalDataHolder`, resilient-client,
//! shed order, forward-only migrations: the contract a NON-Rust subsystem must satisfy; owner = the
//! diverging subsystem, "Chat connection tier likely, TE-21"). Owning architecture: chat
//! `00-overview.md` §0 (the TE-21 connection-tier divergence call) + `03 §9` (the gateway must not
//! regress to fire-and-forget even if it diverges to BEAM).
//!
//! ## The seam this pair pins (chat PINS the language; the harness shim is the divergence contract)
//! - **PROVIDER (chat — [`myelin_chat::glue`])** PINS the connection-tier language: **Rust** by
//!   default; the BEAM/Phoenix hatch is written-but-CLOSED (opened only if CHAT-D3/D4 prove Rust
//!   intractable — CHAT-P26). It RECORDS the TE-21 obligation against the 1.7 shim.
//! - **CONSUMER (the 1.7 harness shim)** is the contract a diverging (non-Rust) subsystem must
//!   satisfy. In the all-Rust default the shim is a NO-OP — there is no cross-language boundary, so
//!   the shim's obligation is satisfied trivially (recorded, never silently skipped). The shim's
//!   obligations BIND only when the BEAM hatch is selected (CHAT-P26).

use myelin_chat::glue::{te21_harness_shim_obligation, Te21LanguagePin};

/// **PROVIDER side of 1.7** — chat pins the connection-tier language. The provider's promise: the
/// M2-C0 pin is Rust (the all-Rust default), and the BEAM hatch is written-but-closed.
fn provider_te21_pin() -> Te21LanguagePin {
    te21_harness_shim_obligation()
}

/// **CONSUMER side of 1.7** — the harness shim's obligation. The consumer's promise: in the all-Rust
/// default the shim is a NO-OP (no cross-language boundary); the obligation binds only on divergence
/// (the BEAM hatch, CHAT-P26).
fn consumer_shim_is_no_op(pin: Te21LanguagePin) -> bool {
    pin.is_no_op()
}

/// The 1.7 pair, end-to-end: the PROVIDER (chat) pins Rust + records the TE-21 obligation, and the
/// CONSUMER (the harness shim) confirms the obligation is a NO-OP in the all-Rust default — the dated
/// green artifact (the shim's no-op obligation is satisfied; the contract-coverage scanner's 1.7 row).
#[test]
fn cdc_1_7_chat_provider_pins_rust_consumer_shim_is_a_no_op() {
    let pin = provider_te21_pin();
    assert_eq!(pin, Te21LanguagePin::Rust, "the M2-C0 TE-21 pin is Rust");
    assert!(consumer_shim_is_no_op(pin), "the all-Rust default makes the 1.7 harness shim a NO-OP");
    assert_eq!(Te21LanguagePin::PINNED, Te21LanguagePin::Rust);
}

/// The BEAM hatch is written-but-CLOSED: it EXISTS as a variant (the divergence is designed, not
/// erased) but is NOT a no-op — its cross-language harness-shim obligations would BIND when selected
/// (CHAT-P26). The negative half of the seam: divergence carries the shim obligation; the Rust
/// default does not.
#[test]
fn cdc_1_7_the_beam_hatch_carries_the_shim_obligation_when_selected() {
    assert!(
        !Te21LanguagePin::Beam.is_no_op(),
        "the BEAM hatch is written-but-closed — its 1.7 harness-shim obligations bind (CHAT-P26)"
    );
}
