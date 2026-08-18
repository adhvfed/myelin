use super::*;

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

#[test]
fn the_frozen_set_is_exactly_seven() {
    assert_eq!(
        Nonnegotiable::ALL.len(),
        7,
        "§3.7 freezes exactly seven non-negotiables"
    );
    let mut clauses: Vec<u8> = Nonnegotiable::ALL.iter().map(|n| n.clause()).collect();
    clauses.sort_unstable();
    assert_eq!(clauses, vec![1, 2, 3, 4, 5, 6, 7]);
    let mut labels: Vec<&str> = Nonnegotiable::ALL.iter().map(|n| n.label()).collect();
    let n = labels.len();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), n, "every non-negotiable has a distinct label");
}

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

#[test]
fn a_dropped_non_negotiable_fails_loudly_and_names_the_clause() {
    let tier = FakeBeamTier {
        name: "chat-beam-connection-tier",
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
    assert_eq!(Nonnegotiable::NoFireAndForgetEmit.clause(), 3);
    assert!(format!("{}", Nonnegotiable::NoFireAndForgetEmit).contains("§3.7.3"));
}

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

