use myelin_agent::EffectKind;
use myelin_agent_service::defaults::requires_approval_default;
use myelin_ci_controlplane::{ci_requires_approval_default, ci_tool_defs, CI_TOOL_NAMES};

#[test]
fn cdc_8_1_fabric_seed_agrees_with_ci_owned_x6_classification() {
    for tool in CI_TOOL_NAMES {
        assert_eq!(
            requires_approval_default("ci", tool),
            ci_requires_approval_default(tool),
            "ci.{tool}: the Fabric seed and CI's owned X-6 classification must agree (no drift)"
        );
    }
}

#[test]
fn cdc_8_1_x6_values_are_frozen_correct_on_both_sides() {
    for gated in ["deploy", "approve_deploy", "rollback", "write_secret"] {
        assert!(
            requires_approval_default("ci", gated),
            "fabric: ci.{gated} gated"
        );
        assert!(
            ci_requires_approval_default(gated),
            "ci-owned: ci.{gated} gated"
        );
    }
    for not_gated in [
        "run",
        "run_pipeline",
        "cancel_run",
        "retry_run",
        "read_log",
        "read_run",
        "validate",
        "plan",
    ] {
        assert!(
            !requires_approval_default("ci", not_gated),
            "fabric: ci.{not_gated} NOT gated"
        );
        assert!(
            !ci_requires_approval_default(not_gated),
            "ci-owned: ci.{not_gated} NOT gated"
        );
    }
}

#[test]
fn cdc_8_1_provider_tool_defs_carry_the_seeded_x6_shape() {
    let defs = ci_tool_defs();
    assert_eq!(defs.len(), CI_TOOL_NAMES.len(), "the complete X-6 CI set");
    for d in &defs {
        assert_eq!(d.subsystem, "ci");
        assert_eq!(
            d.requires_approval,
            requires_approval_default("ci", &d.name.0),
            "ci.{} gating IS the frozen X-6 seed",
            d.name.0
        );
        let is_read = matches!(d.effect_kind, EffectKind::Read);
        assert_eq!(
            is_read, !d.side_effecting,
            "ci.{}: a read is not side-effecting; a mutate is",
            d.name.0
        );
    }
    assert!(!defs.iter().any(|d| d.name.0 == "exec"));
}

#[test]
fn cdc_8_1_the_gated_set_is_exactly_the_four_privileged_ci_gates() {
    let gated: Vec<&str> = ci_tool_defs()
        .iter()
        .filter(|d| d.requires_approval)
        .map(|d| {
            CI_TOOL_NAMES
                .iter()
                .copied()
                .find(|n| *n == d.name.0)
                .expect("a registered name")
        })
        .collect();
    assert_eq!(
        gated,
        vec!["deploy", "approve_deploy", "rollback", "write_secret"]
    );
}
