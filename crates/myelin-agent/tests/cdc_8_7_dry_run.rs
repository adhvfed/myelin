//! # The CDC pair for contract 8.7 — `run --dry-run(InboxEvent) -> Vec<ProposedEffect>`
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.7
//! (`run --dry-run(InboxEvent) → Vec<ProposedEffect>` — plan-then-apply testability). Owning
//! architecture: `agent-fabric.md` §7.1. AG-P1 / P-130 ships the SIGNATURE half (the [`DryRun`]
//! trait); the `run --dry-run` plan-lever body lands in AG-P8 (→ P-220).
//!
//! ## What this pair pins (the signature half of 8.7)
//! - the **PROVIDER** is the agent fabric: `dry_run` returns the proposed effects a run WOULD make,
//!   WITHOUT applying any (no mutation, no `EffectApi::apply`). This is the plan-then-apply
//!   testability lever.
//! - the **CONSUMER** is the CLI / a test: it reads the plan (the `Vec<ProposedEffect>`) and
//!   asserts on it without side effects — the lever that makes plan-then-apply testable.

use myelin_agent::{DryRun, InboxEvent, ProposedEffect};

/// **PROVIDER side of 8.7 (agent fabric).** A dry-run that plans two effects for a mention and
/// applies NONE. The plan-lever body lands in AG-P8 (→ P-220); this pins the plan-without-mutate
/// shape.
struct ProviderDryRun;

impl DryRun for ProviderDryRun {
    fn dry_run(&self, inbox: InboxEvent) -> Vec<ProposedEffect> {
        if inbox.0 == "mention" {
            vec![
                ProposedEffect("comment".into()),
                ProposedEffect("label".into()),
            ]
        } else {
            vec![]
        }
    }
}

#[test]
fn cdc_8_7_dry_run_plans_without_applying() {
    let provider = ProviderDryRun;

    // CONSUMER (the CLI / a test): read the plan; no mutation happened (plan-then-apply testability).
    let plan = provider.dry_run(InboxEvent("mention".into()));
    assert_eq!(
        plan,
        vec![
            ProposedEffect("comment".into()),
            ProposedEffect("label".into()),
        ]
    );

    // An unmatched event plans nothing.
    assert!(provider.dry_run(InboxEvent("noise".into())).is_empty());
}
