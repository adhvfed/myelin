//! # The CDC pair for contract 5.9 — git's owned `CheckStatus` CONSUMER half (GIT-P6 / P-232)
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 5.9 (the
//! Git↔CI `CheckStatus` seam — CI-owned `CheckStatus` keyed `(commit_oid, context)`, last-writer-wins
//! by `run_attempt`; Git maintains the `check_status` projection + the branch-protection `required`-set
//! policy + fork-endorsement). **Reconciliation:** `00-reconciliation-decisions.md` X-1 (the most
//! load-bearing cross-subsystem seam — the frozen `CheckStatus` shape + the monotonic supersession +
//! the merge gate). Owning architecture: git
//! `04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md` §1.1 (the
//! X-1 consumer) + `00-overview.md` §0.1 Δ1/Δ2/Δ3.
//!
//! ## The seam this pair pins (CI produces; Git is the consumer + gate)
//! Row 5.9 is the seam between the PRODUCER that emits the `CheckStatus` fact (CI — over the Bus, as
//! an OPAQUE `ci.check.updated` payload, `myelin_events::check_seam`) and the CONSUMER that decodes +
//! mirrors it into the Git-owned `check_status` projection, applies the monotonic `run_attempt`
//! supersession, and evaluates the merge gate against the `required`-set policy (Git —
//! [`myelin_git::check_status`]). This pair pins the **CONSUMER half Git owns**:
//!
//! - the **decode** of the Bus's opaque `CheckStatus` payload into the typed consumer view (no second
//!   struct — the opaque `serde_json::Value` the Bus carries decodes to exactly Git's
//!   [`CheckStatus`](myelin_git::check_status::CheckStatus));
//! - the **projection-table schema** keyed `(commit_oid, context)` (exactly one current row per key);
//! - the **monotonic `run_attempt` supersession** rule (a late lower attempt is dropped);
//! - the **`required`-set policy** + the gate outcome (Git decides which contexts gate; an
//!   un-endorsed `untrusted_fork` success is neutral — Δ3).
//!
//! ## FLOOR (the X-1 seam-floor — named, not silent)
//! This is the DECLARED, COMPILING, NOT-YET-LIVE consumer. No event consumer is wired, no migration is
//! run, no merge gate fires here. The live consumer + merge gate are GIT-P20 (against a SYNTHETIC
//! `ci.check.updated` emitter — the seam-floor, roadmap §5); the real CI producer wiring is the M4
//! co-gate (GIT-D10 / CI-D8 end-to-end). **No cargo-mutants floor:** this is a SCHEMA + supersession
//! DECLARATION against the frozen 5.9 shape proven by the unit drills in
//! `myelin_git::check_status::tests`, not new live load-bearing resolution logic (the consumer/gate
//! mutation floors land with GIT-P20).

use myelin_git::check_status::{
    gate_outcome, supersedes, ApplyOutcome, CheckContext, CheckState, CheckStatus,
    CheckStatusProjection, GateOutcome, GitOid, HumanisedRef, RequiredSetPolicy, Timestamp,
    TrustTier, CHECK_STATUS_PROJECTION_DDL,
};
use myelin_tenancy::{ArtifactRef, TenantId};
use std::collections::BTreeMap;

/// **PRODUCER side of 5.9** — CI emits the `CheckStatus` fact carried OPAQUE over the Bus (a
/// `serde_json::Value` payload on `ci.check.updated`, per `myelin_events::check_seam`). This builds
/// the opaque payload a producer would carry — the consumer must decode exactly this.
fn producer_opaque_payload(
    commit: &str,
    provider_name: &str,
    attempt: u32,
    state: CheckState,
    trust: TrustTier,
) -> serde_json::Value {
    // The producer serialises the frozen 5.9 shape; the Bus carries it opaque. We build it from the
    // shared typed view so PRODUCER and CONSUMER are proven to agree on the ONE shape (no drift).
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), provider_name.to_string());
    let fact = CheckStatus {
        tenant: TenantId("acme".into()),
        repo: ArtifactRef("myelin://acme/git/repo/core".into()),
        commit_oid: GitOid(commit.into()),
        context: CheckContext::ci(provider_name),
        state,
        required: true,
        run: ArtifactRef("myelin://acme/ci/run/9".into()),
        run_attempt: attempt,
        trust_tier: trust,
        details_ref: ArtifactRef("myelin://acme/ci/run/9#step-2".into()),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args,
        },
        started_at: Timestamp("2026-06-21T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-21T00:01:00Z".into())),
        cost_settled: true,
    };
    serde_json::to_value(&fact).expect("the 5.9 fact serialises to the opaque Bus payload")
}

/// **CONSUMER side of 5.9** — Git decodes the Bus's opaque payload into the typed consumer view and
/// applies it to the projection. The consumer's promise: it decodes exactly the producer's shape (no
/// second struct), applies monotonic supersession, and never silently clobbers a newer row.
fn consumer_decode(opaque: serde_json::Value) -> CheckStatus {
    serde_json::from_value(opaque).expect("the opaque Bus payload decodes to Git's consumer view")
}

/// **The CDC: the producer's opaque payload decodes to the consumer view, byte-faithful.** CI emits
/// the OPAQUE `CheckStatus`; Git decodes it into the typed view and mirrors it. The round-trip proves
/// PRODUCER and CONSUMER agree on the ONE frozen 5.9 shape (the seam is well-typed across the bus).
#[test]
fn cdc_5_9_producer_opaque_payload_decodes_to_consumer_view() {
    let opaque = producer_opaque_payload(
        "abc123",
        "build",
        1,
        CheckState::Success,
        TrustTier::Trusted,
    );
    // The Bus carries the CheckStatus OPAQUE — it round-trips untouched as a serde_json::Value.
    assert_eq!(opaque["commit_oid"], "abc123");
    assert_eq!(opaque["context"]["name"], "build");
    assert_eq!(opaque["state"], "success");
    assert_eq!(opaque["trust_tier"], "trusted");
    assert_eq!(opaque["run_attempt"], 1);

    // The CONSUMER decodes the opaque value into the typed view (no second struct — the carriage
    // decodes to exactly Git's CheckStatus).
    let fact = consumer_decode(opaque);
    assert_eq!(fact.commit_oid, GitOid("abc123".into()));
    assert_eq!(fact.context, CheckContext::ci("build"));
    assert_eq!(fact.state, CheckState::Success);
    assert_eq!(fact.trust_tier, TrustTier::Trusted);
}

/// **The CDC: the consumer applies monotonic supersession + evaluates the gate.** This is the END of
/// the consumer half: decode → project (last-writer-wins by `run_attempt`) → gate. A late lower
/// attempt is dropped; the required-set policy decides the gate (Git decides which contexts gate).
#[test]
fn cdc_5_9_consumer_supersedes_and_gates() {
    let mut proj = CheckStatusProjection::new();
    let build = CheckContext::ci("build");

    // CI emits build attempt 1 (failure); Git decodes + applies.
    let a1 = consumer_decode(producer_opaque_payload(
        "c1",
        "build",
        1,
        CheckState::Failure,
        TrustTier::Trusted,
    ));
    assert_eq!(
        proj.apply(&a1),
        ApplyOutcome::Superseded { current_attempt: 1 }
    );

    // CI emits the re-run, build attempt 2 (success); Git supersedes.
    let a2 = consumer_decode(producer_opaque_payload(
        "c1",
        "build",
        2,
        CheckState::Success,
        TrustTier::Trusted,
    ));
    assert_eq!(
        proj.apply(&a2),
        ApplyOutcome::Superseded { current_attempt: 2 }
    );

    // The at-least-once transport re-delivers the stale attempt 1 — the consumer DROPS it.
    assert_eq!(
        proj.apply(&a1),
        ApplyOutcome::DroppedStale {
            incoming_attempt: 1,
            current_attempt: 2
        },
        "a late lower attempt is dropped (supersession is monotonic, X-1)"
    );

    // The gate: build is required; the current (attempt 2) row is a trusted success → GREEN.
    let policy = RequiredSetPolicy::requiring(vec![build]);
    assert_eq!(
        gate_outcome(&policy, &proj, &GitOid("c1".into()), &[]),
        GateOutcome::AllRequiredGreen
    );
}

/// **The CDC: the supersession rule + the projection key are the frozen X-1 shape.** Pins the
/// `(commit_oid, context)` key, the `>=` monotonic rule, and the projection-table DDL — the contract
/// surface the GIT-P20 live consumer + the CI producer (M4) build against.
#[test]
fn cdc_5_9_frozen_shape_surfaces() {
    // The supersession rule: monotonic on run_attempt, `>=` (a re-apply of the same attempt is
    // idempotent), a lower attempt is dropped.
    assert!(supersedes(2, 1));
    assert!(supersedes(1, 1));
    assert!(!supersedes(1, 2));

    // The projection-table schema includes every authority dimension with the supersession column.
    assert!(CHECK_STATUS_PROJECTION_DDL
        .contains("tenant_id, region, repo_ref, commit_oid, context_provider, context_name"));
    assert!(CHECK_STATUS_PROJECTION_DDL.contains("run_attempt"));

    // CheckContext distinguishes ci vs external providers (the KEY half of the frozen shape).
    assert_ne!(CheckContext::ci("build"), CheckContext::external("build"));
}
