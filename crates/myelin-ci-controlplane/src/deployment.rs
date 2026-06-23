//! # Deployments & the protected-env HITL gate (CI-P24 / P-367, M4).
//!
//! Architecture: `continuous-integration/architecture/03-events-contracts-and-glue.md` §1.2 (the
//! `ci.deployment.*` HITL flow); `00-reconciliation-decisions.md` §OQ-F (the per-effect `idem_key` for
//! batch approval cards), §X-6 (the `requires_approval` defaults — deploy/secret = yes).
//!
//! ## What this module OWNS vs CONSUMES (EI-01 §7 coherence — reconcile-in-place)
//!
//! - **CONSUMES** the FROZEN `myelin-flow` HITL substrate: [`per_effect_idem_key`] (the §6.4 rule —
//!   `card_id` single / `card_id:idx` multi), [`ApprovalDecision`], [`EffectOutcome`] (the apply/withhold
//!   outcome). It builds NO second approval mechanism. The durable signal wait
//!   (`wait_for_signal("approval:<stage>")`) lives in [`crate::ci_pipeline`] (already landed) — this
//!   module owns the deploy-domain shaping: the approver-set resolution, the per-effect gated apply, and
//!   the `ci.deployment.*` event drafts.
//! - **CONSUMES** the FROZEN [`IdentityService::list_subjects`] (contract 4.4) over the already-declared
//!   `environment#approve` ReBAC target (`crate::rebac_fragment`) — the HITL approver set.
//! - **CONSUMES** the FROZEN `ci.deployment.*` event tokens (`myelin_ci_sandbox::events`) and the
//!   [`EventDraft`] envelope shape — the OUTBOX is the only emit path (no `publish_now`).
//! - **OWNS** only the deploy state machine + the protected-env gate composition.
//!
//! ## The protected-env HITL flow (arch §1.2)
//!
//! 1. A deploy to a `protected` environment emits `ci.deployment.requested` →
//!    `ci.deployment.approval_required` (the gate opens; the card is rendered via `humanise`, 7.3).
//! 2. The approver set resolves via `list_subjects(environment, approve)` (4.4) — exactly the subjects
//!    holding the `approve` permission on THIS environment.
//! 3. The human decision lands as a durable signal (9.4). The per-effect `idem_key` (OQ-F) makes a
//!    **double-click ONE approval** and a **declined effect WITHHELD** (returns `Denied`, 0 mutation,
//!    AG-8). On approve → `ci.deployment.approved` → `ci.deployment.started`/`succeeded`. On decline →
//!    `ci.deployment.rejected` (withheld, never mutated).
//! 4. **Rollback is first-class** (`ci.deployment.rolled_back` — reversibility, not "are you sure?").
//!
//! An UNprotected environment skips the gate (no approval required) — it deploys directly.
//!
//! FLOOR named: **none new.** The deploy gate composes the FROZEN `myelin-flow` signals
//! ([`per_effect_idem_key`] / [`EffectOutcome`]) + the FROZEN `requires_approval` defaults (X-6,
//! deploy = yes) + the FROZEN `ci.deployment.*` tokens — it introduces no stubbed/deferred surface.

use myelin_ci_sandbox::events;
use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_flow::{per_effect_idem_key, ApprovalDecision, EffectOutcome};
use myelin_identity::{Consistency, IdentityService, ObjectId, Permission, Result as IdResult};

/// **The APPROVE permission on a `ci_environment` (the FROZEN HITL target, contract 4.4 / 4.9).** The
/// `list_subjects(environment, approve)` target that resolves the protected-env approver set (the
/// `approver` relation on `environment` — `crate::rebac_fragment::APPROVE`).
pub const ENVIRONMENT_APPROVE_PERMISSION: &str = "approve";

/// **A deployment's lifecycle state (arch §1.2 / the `deployment` table CHECK).** The protected-env
/// gate parks in [`DeployState::AwaitingApproval`] until the durable approval signal lands; an
/// unprotected env skips straight to [`DeployState::Deploying`]. `RolledBack` is first-class
/// (reversibility).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeployState {
    /// The protected-env gate is OPEN — parked on the durable approval signal (no runtime held).
    AwaitingApproval,
    /// Approved (or unprotected) — the deploy is in flight.
    Deploying,
    /// The deploy succeeded.
    Deployed,
    /// The deploy failed.
    Failed,
    /// The deploy was rolled back (first-class reversibility, arch §1.2).
    RolledBack,
}

impl DeployState {
    /// The `deployment.state` column token (the migration CHECK set — `migrations.rs`).
    pub fn as_token(self) -> &'static str {
        match self {
            DeployState::AwaitingApproval => "awaiting_approval",
            DeployState::Deploying => "deploying",
            DeployState::Deployed => "deployed",
            DeployState::Failed => "failed",
            DeployState::RolledBack => "rolled_back",
        }
    }
}

/// **The outcome of gating ONE deploy effect through the protected-env HITL gate (OQ-F / §6.4).**
/// Composes the FROZEN `myelin-flow` [`EffectOutcome`]: an APPROVED deploy applies exactly once (a
/// double-click re-sends the SAME per-effect key → one apply); a DECLINED deploy is WITHHELD (returns
/// `Denied`, 0 mutation, AG-8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeployGateOutcome {
    /// The deploy was APPROVED and applied (the effect ran exactly once) — carries the deployment id.
    Approved(String),
    /// The deploy was DECLINED and WITHHELD — `Denied`, 0 mutation (AG-8). Carries the decline token.
    Withheld(String),
}

impl DeployGateOutcome {
    /// Did this gate APPLY the deploy (an approval that ran)?
    pub fn is_applied(&self) -> bool {
        matches!(self, DeployGateOutcome::Approved(_))
    }
}

/// **The protected-env deploy gate over the FROZEN per-effect `idem_key` (OQ-F).** Models the deploy as
/// a single gated effect whose `idem_key` is the §6.4 [`per_effect_idem_key`] of the deployment's
/// approval card — so a DOUBLE-CLICK on "approve" re-sends the SAME key → the dedup applies the deploy
/// exactly ONCE (the production wiring is the `ON CONFLICT DO NOTHING` on `wf_signal`; here the
/// already-applied set is the dedup), and a DECLINED deploy is WITHHELD (0 mutation, AG-8).
///
/// `applied` is the set of per-effect `idem_key`s already applied (the durable dedup ledger in
/// production; an in-memory set in a unit test). The gate is IDEMPOTENT: re-running it with the same
/// `card_id` + decision over the same `applied` set NEVER double-applies.
pub struct DeployGate;

impl DeployGate {
    /// **Gate ONE deploy through the protected-env HITL (the per-effect `idem_key` rule, OQ-F).**
    ///
    /// - **`Approve`** → if the deploy's `idem_key` is NOT already applied, run `apply` ONCE, record the
    ///   key, and return [`DeployGateOutcome::Approved`]. A double-click re-enters with the SAME key →
    ///   it is already in `applied` → `apply` is NOT re-run → still [`DeployGateOutcome::Approved`] with
    ///   the recorded id (ONE apply — OQ-F).
    /// - **`Decline`** → the deploy is WITHHELD: `apply` is NEVER called, 0 mutation, returns
    ///   [`DeployGateOutcome::Withheld`] (AG-8).
    ///
    /// A single-effect deploy card keys on the bare `card_id` (the §6.4 degenerate case); the
    /// `effect_idx`/`total` ride the FROZEN [`per_effect_idem_key`] so a BATCH of deploys (approve some,
    /// decline others) is well-defined.
    pub fn gate_deploy(
        card_id: &str,
        effect_idx: usize,
        total_effects: usize,
        decision: ApprovalDecision,
        applied: &mut std::collections::HashMap<String, String>,
        apply: impl FnOnce() -> String,
    ) -> DeployGateOutcome {
        let key = per_effect_idem_key(card_id, effect_idx, total_effects);
        match decision {
            // DECLINED (AG-8): WITHHELD — `apply` is NEVER reached, 0 mutation.
            ApprovalDecision::Decline => DeployGateOutcome::Withheld(DECLINE_TOKEN.to_string()),
            ApprovalDecision::Approve => {
                // Already applied (a double-click re-sent the SAME key) → NO second apply (OQ-F).
                if let Some(existing) = applied.get(&key) {
                    return DeployGateOutcome::Approved(existing.clone());
                }
                // First application of this per-effect key → apply EXACTLY ONCE + record it.
                let dep_id = apply();
                applied.insert(key, dep_id.clone());
                DeployGateOutcome::Approved(dep_id)
            }
        }
    }
}

/// **The decline token a withheld deploy carries (a machine token, no PII).** Mirrors the FROZEN
/// `myelin_flow::DECLINE_MARKER` semantics at the CI-deploy domain.
pub const DECLINE_TOKEN: &str = "declined";

/// **Map a `myelin-flow` [`EffectOutcome`] onto a [`DeployGateOutcome`] (the seam to the durable
/// workflow path).** The `ci.pipeline` workflow body (already landed) drives the durable
/// `apply_approved_effects` loop over `wf_signal`; this maps its per-effect outcome onto the deploy
/// domain so the producer side (the `ci.deployment.*` drafts) reads ONE outcome type.
pub fn deploy_outcome_of(effect: &EffectOutcome) -> DeployGateOutcome {
    match effect {
        EffectOutcome::Applied(id) => DeployGateOutcome::Approved(id.clone()),
        EffectOutcome::Withheld(reason) => DeployGateOutcome::Withheld(reason.clone()),
    }
}

/// **Resolve the protected-env approver set via `list_subjects(environment, approve)` (contract 4.4).**
/// The HITL approver set is EXACTLY the subjects holding `approve` on THIS environment — the FROZEN
/// `environment#approve` ReBAC target. Returns the flattened subject ids (the card's approver
/// audience). A non-protected environment does NOT call this (no gate).
pub fn resolve_approvers<I: IdentityService>(
    identity: &I,
    environment: &ObjectId,
    at: &Consistency,
) -> IdResult<Vec<String>> {
    let tree = identity.list_subjects(
        environment,
        &Permission(ENVIRONMENT_APPROVE_PERMISSION.to_string()),
        at,
    )?;
    Ok(tree.members.into_iter().map(|p| p.0).collect())
}

/// **Is a deploy to this environment gated (the protected-env HITL)?** A `protected` environment
/// requires approval (X-6 default: deploy = yes); an unprotected environment deploys directly. This is
/// the single decision point that opens (or skips) the gate.
pub fn deploy_requires_approval(protected: bool) -> bool {
    // The X-6 frozen default: a protected-env deploy requires approval; an unprotected one does not.
    protected
}

// ===========================================================================
// The `ci.deployment.*` event drafts (emitted via the OUTBOX only — arch §3).
// ===========================================================================

/// **The subject ArtifactRef for a deployment (`myelin://<tenant>/ci/deployment/<dep_id>`).** The
/// aggregate is `deployment:<dep_id>` (per-deployment ordering, arch §1.2).
fn deployment_subject(tenant_id: &str, dep_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant_id}/ci/deployment/{dep_id}"))
}

/// Build a `ci.deployment.*` [`EventDraft`] for `event_type` (one of the FROZEN
/// `myelin_ci_sandbox::events::CI_DEPLOYMENT_*` tokens). The payload is references-not-payloads
/// (deployment id / env / run / state — opaque ids + tokens, NO PII inline); the OUTBOX derives the
/// causality correct-by-construction. `approved_by` (when present) is a PSEUDONYM subject (contract
/// 4.8), carried as a reference token only — never a clear PII body.
#[allow(clippy::too_many_arguments)]
fn deployment_draft(
    event_type: &str,
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
    state: DeployState,
    approved_by: Option<&str>,
) -> EventDraft {
    EventDraft {
        type_: EventType(event_type.to_string()),
        subject: deployment_subject(tenant_id, dep_id),
        aggregate: AggregateKey(format!("deployment:{dep_id}")),
        payload: serde_json::json!({
            "dep_id": dep_id,
            "env_id": env_id,
            "run_id": run_id,
            "state": state.as_token(),
            // The approver pseudonym ref (contract 4.8) — a token, never a clear PII body.
            "approved_by": approved_by,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        // The payload carries opaque ids + a pseudonym subject TOKEN — no inline clear PII (the
        // pseudonym is resolved Id-side via `resolve_pseudonym`, 4.8; the event carries only the ref).
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// `ci.deployment.requested` — a deploy of `run_id`'s version to `env_id` was requested.
pub fn deployment_requested_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_REQUESTED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::AwaitingApproval,
        None,
    )
}

/// `ci.deployment.approval_required` — the protected-env HITL gate OPENED (the card is rendered via
/// `humanise`, 7.3; the approver set resolved via `list_subjects`, 4.4).
pub fn deployment_approval_required_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_APPROVAL_REQUIRED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::AwaitingApproval,
        None,
    )
}

/// `ci.deployment.approved` — the durable approval signal landed (per-effect `idem_key`, OQ-F);
/// `approved_by` is the PSEUDONYM subject (4.8) who approved.
pub fn deployment_approved_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
    approved_by: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_APPROVED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::Deploying,
        Some(approved_by),
    )
}

/// `ci.deployment.rejected` — the deploy was DECLINED (withheld; 0 mutation, AG-8).
pub fn deployment_rejected_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_REJECTED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::AwaitingApproval,
        None,
    )
}

/// `ci.deployment.started` — the deploy is in flight (approved or unprotected).
pub fn deployment_started_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_STARTED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::Deploying,
        None,
    )
}

/// `ci.deployment.succeeded` — the deploy completed (at-most-once-in-effect, arch §3).
pub fn deployment_succeeded_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_SUCCEEDED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::Deployed,
        None,
    )
}

/// `ci.deployment.failed` — the deploy failed.
pub fn deployment_failed_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_FAILED,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::Failed,
        None,
    )
}

/// `ci.deployment.rolled_back` — first-class reversibility (arch §1.2, NOT "are you sure?").
pub fn deployment_rolled_back_draft(
    tenant_id: &str,
    dep_id: &str,
    env_id: &str,
    run_id: &str,
) -> EventDraft {
    deployment_draft(
        events::CI_DEPLOYMENT_ROLLED_BACK,
        tenant_id,
        dep_id,
        env_id,
        run_id,
        DeployState::RolledBack,
        None,
    )
}

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;
