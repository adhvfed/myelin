//! Unit tests for the cross-language shim conformance mechanism (P-S30). These test the
//! MECHANISM with a fake divergent-tier probe — the chat-specific PROVIDER read + the loud
//! N/A recording (the all-Rust default today) live in
//! `tests/shim_conformance_p_s30.rs` (a dev-dependency on `myelin-chat`).

use super::*;

/// A fake divergent tier whose probe answers per-clause from a fixed set of FAILING clauses —
/// the test harness for the enforcement mechanism (stands in for the real BEAM-tier probe).
struct FakeBeamTier {
    name: &'static str,
    failing: &'static [Nonnegotiable],
}

impl DivergentTierProbe for FakeBeamTier {
    fn tier_name(&self) -> &str {
        self.name
    }
    fn probe(&self, n: Nonnegotiable) -> bool {
        !self.failing.contains(&n)
    }
}

/// The frozen set is EXACTLY the seven §3.7 non-negotiables — no more, no fewer. A drop is an
/// enum edit, not a silent shrink (EI-01 §5).
#[test]
fn the_frozen_set_is_exactly_seven() {
    assert_eq!(
        Nonnegotiable::ALL.len(),
        7,
        "§3.7 freezes exactly seven non-negotiables"
    );
    // Clause indices are 1..=7, each distinct.
    let mut clauses: Vec<u8> = Nonnegotiable::ALL.iter().map(|n| n.clause()).collect();
    clauses.sort_unstable();
    assert_eq!(clauses, vec![1, 2, 3, 4, 5, 6, 7]);
    // Labels are distinct.
    let mut labels: Vec<&str> = Nonnegotiable::ALL.iter().map(|n| n.label()).collect();
    let n = labels.len();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), n, "every non-negotiable has a distinct label");
}

/// A fully-conformant divergent tier yields all-green and ENFORCED — the shim binds and passes
/// (the CHAT-P26 success path).
#[test]
fn a_conformant_divergent_tier_is_enforced_green() {
    let tier = FakeBeamTier {
        name: "chat-beam-connection-tier",
        failing: &[],
    };
    let conformance = ShimConformance::check(&tier);
    assert!(conformance.all_green(), "all seven clauses pass");
    assert!(conformance.failures().is_empty());
    assert_eq!(conformance.results().len(), 7);

    let enforced = ShimEnforcement::enforce(&tier).expect("a conformant tier enforces green");
    assert!(enforced.is_enforced());
    assert!(!enforced.is_na());
    assert!(enforced.artifact_row().contains("×7 GREEN"));
    assert!(enforced
        .artifact_row()
        .contains("chat-beam-connection-tier"));
}

/// A divergent tier that DROPS one non-negotiable does NOT enforce — `enforce` returns the red
/// verdict naming the exact failing clause; the gate reads RED. There is no path from a failing
/// suite to an `Enforced` record (the "a green must be earned" ratchet).
#[test]
fn a_dropped_non_negotiable_fails_loudly_and_names_the_clause() {
    let tier = FakeBeamTier {
        name: "chat-beam-connection-tier",
        // The classic boundary drop: the divergent tier keeps a fire-and-forget publish path.
        failing: &[Nonnegotiable::NoFireAndForgetEmit],
    };
    let conformance = ShimConformance::check(&tier);
    assert!(!conformance.all_green());
    assert_eq!(
        conformance.failures(),
        vec![Nonnegotiable::NoFireAndForgetEmit]
    );

    let err = ShimEnforcement::enforce(&tier).expect_err("a dropped clause cannot enforce green");
    assert_eq!(err.failures(), vec![Nonnegotiable::NoFireAndForgetEmit]);
    // The failing clause is named with its §3.7 index.
    assert_eq!(Nonnegotiable::NoFireAndForgetEmit.clause(), 3);
    assert!(format!("{}", Nonnegotiable::NoFireAndForgetEmit).contains("§3.7.3"));
}

/// Multiple drops are ALL reported — the red names every failing clause, not just the first
/// (no early-exit that hides a second violation).
#[test]
fn multiple_drops_are_all_named() {
    let tier = FakeBeamTier {
        name: "chat-beam-connection-tier",
        failing: &[
            Nonnegotiable::ThreeSurfaceTopology,
            Nonnegotiable::ForwardOnlyMigrations,
        ],
    };
    let conformance = ShimConformance::check(&tier);
    assert!(!conformance.all_green());
    let mut failures = conformance.failures();
    failures.sort_unstable();
    assert_eq!(
        failures,
        vec![
            Nonnegotiable::ThreeSurfaceTopology,
            Nonnegotiable::ForwardOnlyMigrations
        ]
    );
}

/// The all-Rust default: a loudly-recorded N/A — NOT a silent skip. The record carries the
/// subsystem, the pinned language, and a date; the artifact row is LOUD (`N/A` + reason).
#[test]
fn the_rust_default_records_a_loud_dated_na() {
    let na = ShimEnforcement::recorded_na(
        "chat-connection-tier",
        "Rust (TE-21 pin; BEAM hatch written-but-closed)",
        "2026-06-22",
    );
    assert!(na.is_na());
    assert!(!na.is_enforced());
    let row = na.artifact_row();
    assert!(row.contains("N/A"));
    assert!(row.contains("2026-06-22"));
    assert!(row.contains("chat-connection-tier"));
    assert!(row.contains("NO-OP"));
    // The loudness invariant: the row explicitly states it is NOT a silent skip.
    assert!(row.contains("NOT a silent skip"));
}
