//! # The CDC pair for CI's OWNED `ToolDef` X-6 row 8.1 (CI-P26 → P-369, M4)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row **8.1**
//! (`ToolSurface::register_tool(ToolDef)`); arch
//! `04-subsystem-architectures/continuous-integration/architecture/04-views-cli-and-api.md` §3 (the
//! FROZEN X-6 `requires_approval` defaults table); `00-reconciliation-decisions.md` §X-6.
//!
//! ## What this CDC pins (PROVIDER ↔ CONSUMER no-drift)
//! - **PROVIDER** (CI, the OWNER of its action-consequence classification, X-6): the complete CI
//!   agent-tool set with the frozen X-6 gating —
//!   `myelin_ci_controlplane::{ci_tool_defs, ci_requires_approval_default, CI_TOOL_NAMES}`.
//! - **CONSUMER** (the Fabric seed table): `myelin_agent_service::defaults::requires_approval_default`
//!   — the seed `seed_requires_approval` stamps onto a registering `ToolDef`. The Fabric seed MUST
//!   agree with CI's owned classification for every CI tool name (a gating drift on either side — CI
//!   un-gating a deploy, or the seed gating a read — is a TEST BREAK here).
//!
//! This is the SAME provider↔consumer no-drift shape the KN 8.1 CDC
//! (`myelin_knowledge::agent` ↔ `myelin_agent_service::knowledge_tools`) proves, restated for CI's
//! full X-6 surface (CI-P26 completes the read/run/cancel/retry/validate/plan/rollback rows over the
//! four privileged rows CI-P24 / P-347 shipped).

use myelin_agent::EffectKind;
use myelin_agent_service::defaults::requires_approval_default;
use myelin_ci_controlplane::{ci_requires_approval_default, ci_tool_defs, CI_TOOL_NAMES};

/// **THE X-6 GATE (8.1): the Fabric seed table AGREES with CI's owned classification for EVERY CI
/// tool (deploy/secret/rollback/approve = yes; run/read/validate/plan/cancel/retry = no).** A drift
/// on either side flips a value here.
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

/// **The frozen-correct values are pinned absolutely (not just "they agree").** deploy / approve /
/// rollback / write_secret = YES; run / run_pipeline / cancel / retry / read_log / read_run /
/// validate / plan = NO — on BOTH the Fabric seed and CI's owned table.
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

/// **The PROVIDER `ToolDef` set carries the seeded X-6 gating + the read/mutate split (8.1).** Every
/// CI def's `requires_approval` equals the frozen default; the reads are `Read`/not-side-effecting,
/// the rest `Mutate`/side-effecting. `ToolHands::exec` (the runner) is absent (X-6).
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
        // the read/mutate split.
        let is_read = matches!(d.effect_kind, EffectKind::Read);
        assert_eq!(
            is_read, !d.side_effecting,
            "ci.{}: a read is not side-effecting; a mutate is",
            d.name.0
        );
    }
    // exec is never a CI ToolDef (the runner itself, X-6 / 05 §HP-5).
    assert!(!defs.iter().any(|d| d.name.0 == "exec"));
}

/// **The four privileged CI gates are exactly deploy/approve_deploy/rollback/write_secret** (the
/// consequential set; everything else is suggest-by-default / read).
#[test]
fn cdc_8_1_the_gated_set_is_exactly_the_four_privileged_ci_gates() {
    let gated: Vec<&str> = ci_tool_defs()
        .iter()
        .filter(|d| d.requires_approval)
        .map(|d| {
            // leak the static name set membership (the names are 'static)
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
