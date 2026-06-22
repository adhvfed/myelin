//! # `merge_queue` — the merge queue as a durable workflow, the Git-side composition (GIT-P23 / P-285, M3)
//!
//! **The merge queue parks on `ci.result`; exactly-once merge (GIT-D10 part (d) + the full GIT-D10
//! aggregate).** This module is the **Git-side composition** of the merge-queue durable workflow: it
//! wires Git's OWN merge gate ([`crate::merge_gate`], §6.2) + the fork-endorsement posture
//! ([`crate::fork_gate`], §6.3) into the GENERIC durable-workflow BODY
//! ([`myelin_flow::WfCtx::run_merge_attempt`], contract 9.4 — the `ci.result`-waiting,
//! exactly-once-on-`merge_attempt_id` mechanics already shipped at P-FLOW-19 / P-215, M2).
//!
//! **Owning architecture:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md` §6.4
//! (the merge queue — one durable workflow per target ref; the `SCHEDULE_AND_RUN_JOB` long-park idiom;
//! parks on the rollup `ci.result` signal; success-for-all-required → §6.2 `may_merge` + the §3-4
//! linearizable merge + emit `git.pr.merged`; failure → dequeue with a humanised reason) + §6.2 (the
//! "what is allowed to land" decision) + §6.3 (the fork / trust-tier gate). **Contract:** index row
//! **5.9** (the merge-queue `ci.result` wait — OWNED: parks on `ci.result`, exactly-once merge,
//! idempotent on `merge_attempt_id`) + **9.1/9.2/9.4** (the DurableExecutor + `SCHEDULE_AND_RUN_JOB` +
//! the durable `ci.result` signal — CONSUMED from `myelin-flow`). **Reconciliation:** X-1 (the most
//! load-bearing cross-subsystem seam) + OQ-F (per-effect `idem_key` + `SCHEDULE_AND_RUN_JOB`).
//!
//! ## EI-01 §7 — EXTEND/COMPOSE, never duplicate
//!
//! The GENERIC durable mechanics — the deterministic `merge_attempt_id` mint, the reserve-at-dispatch,
//! the `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` long-park, the doubly-delivered
//! ci.result-wakes-once dedup, the merge-or-dequeue branch, the `git.pr.merged` emit, the
//! references-not-payloads `CiResult` codec — ALREADY LIVE in [`myelin_flow::merge_queue`]
//! ([`myelin_flow::WfCtx::run_merge_attempt`]). This module does NOT re-implement any of it. It adds
//! the **genuinely-new Git half** the flow body left as a `MergePerformer` seam:
//!
//! - [`GitMergePerformer`] — the [`myelin_flow::MergePerformer`] that, **at the moment of merge**, runs
//!   Git's authoritative §6.2 required-set gate ([`crate::merge_gate::evaluate_merge_gate`]) + the §6.3
//!   fork-endorsement posture against Git's OWN [`crate::check_status::CheckStatusProjection`] for the
//!   PR head. A merge is performed ONLY when EVERY required context is a current trusted/endorsed
//!   success; an under-gated / fork-self-greened / missing-context merge is REFUSED at the merge step
//!   (the flow body dequeues it with a humanised reason). This is the "what is allowed to land"
//!   decision Git owns — it is NOT a second copy of the gate logic, it is the gate logic
//!   ([`crate::merge_gate`]) bound to the durable merge step.
//!
//! ## Acyclic-by-construction — Git NEVER synchronously calls CI (no-cross-sync-cycle, X-1 / EI-02 §3)
//!
//! The merge queue READS Git's OWN `check_status` projection (a bus-fed mirror of CI's facts) to decide
//! the gate — it makes ZERO synchronous calls to CI. CI emits `ci.check.updated` (the per-context
//! projection feed) + the rollup `ci.result` signal (the merge-queue resume); Git consumes both over
//! the bus and reads its projection. The `no-cross-sync-cycle` lint is green over this module: there is
//! no `reqwest::Client` / `.call_sync(` / `SyncServiceClient` "is it green?" call to CI anywhere. The
//! `e2e_git_p23_merge_queue.rs` drill asserts this with the lint engine over this very source.
//!
//! ## FLOORS named (per the prompt)
//!
//! - **GF-8 — single-lane serialised merge queue.** v1 is one PR tested+merged at a time per target ref
//!   (correctness first). The speculative/parallel batched queue is **GIT-P33 / M5** (OQ-5). The flow
//!   body is single-lane by construction (one `wait_for_signal("ci.result", …)` outstanding per run);
//!   this module composes that single lane. NAMED, not closed here.
//! - **The seam-floor — the REAL CI producer.** Here the `ci.result` rollup is the SYNTHETIC producer's
//!   ([`myelin_flow::MockCiResultProducer`] — the carriage fixture). CI's REAL `ci.result` producer is
//!   **EB-27 / M4**; the X-1 seam goes end-to-end (GIT-D10 / CI-D8 re-confirmed against the real
//!   producer) at the **M4 co-gate**. NAMED, not closed here.

use crate::check_status::{
    is_acceptable_satisfaction, CheckContext, CheckKey, CheckStatusProjection, GitOid,
};
use crate::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy};
use myelin_flow::{ActivityError, MergePerformer, MergeRequest};

// ---------------------------------------------------------------------------
// The Git-owned "what is allowed to land" merge performer (§6.2 + §6.3)
// ---------------------------------------------------------------------------

/// **The Git-owned merge performer — the §6.2/§6.3 "what is allowed to land" decision bound to the
/// durable merge step (GIT-P23).** This is the [`myelin_flow::MergePerformer`] seam the durable
/// merge-queue body ([`myelin_flow::WfCtx::run_merge_attempt`]) calls AFTER the `ci.result` rollup
/// reports `success` for all required contexts. It re-asserts Git's AUTHORITATIVE merge gate against
/// Git's OWN [`CheckStatusProjection`] for the PR head — the projection is the source of truth on the
/// per-context state + trust posture (the rollup is a coarse `overall: success|failure`; Git's
/// projection carries the per-context `trust_tier` the §6.3 fork gate reads).
///
/// **Why re-gate at merge.** The rollup says "CI overall succeeded"; it does NOT carry the per-context
/// trust posture. A fork's `untrusted_fork` success can roll up `overall: success` — but it must NOT
/// merge unless endorsed (§6.3, the poisoned-pipeline defence). So Git re-evaluates the FULL
/// required-set + trust gate at the merge step, reading `trust_tier` OFF the projection facts (never
/// recomputed — X-1). A non-admitted gate REFUSES the merge ([`ActivityError`]), and the flow body
/// dequeues the PR with the §6.2 humanised reason — **0 under-gated / fork-self-greened merges land**.
///
/// **0 synchronous CI calls.** The performer reads the in-memory projection (a bus-fed mirror); it
/// never calls CI. The actual git ref-CAS merge (the §3-4 linearizable merge on `base_ref`) is the
/// `merge_fn` closure the caller supplies — Git owns the merge mechanics; this type owns the GATE that
/// guards it.
pub struct GitMergePerformer<'a, F>
where
    F: Fn(&MergeRequest) -> Result<String, ActivityError>,
{
    /// Git's OWN `check_status` projection for the PR head (the bus-fed mirror of CI's facts). The
    /// authoritative per-context state + `trust_tier` source the §6.2/§6.3 gate reads.
    projection: &'a CheckStatusProjection,
    /// The PR's head commit OID — the `(head_oid, context)` key the gate resolves each required context
    /// against.
    head_oid: GitOid,
    /// The required-set policy (Git's `ruleset.required_contexts` parsed to typed contexts, §6.2). The
    /// merge is gated on EVERY one being a current trusted/endorsed success.
    policy: MergeGatePolicy,
    /// The fork-ENDORSED contexts (the §6.3 maintainer-`approve_untrusted_ci` posture — produced by
    /// [`crate::fork_gate::EndorsementResolver`] from the LIVE check). An `untrusted_fork` success in
    /// this set is acceptable; one outside it is NEUTRAL-for-gating (blocks the merge).
    endorsed_contexts: Vec<CheckContext>,
    /// **The actual git merge (the §3-4 linearizable ref-CAS on `base_ref`).** Called ONLY after the
    /// gate ADMITS. Returns the merged commit OID, or an [`ActivityError`] on a merge conflict (which
    /// the flow body dequeues). Git owns this mechanic; the closure lets the caller bind the real
    /// `GitCore` merge in prod and a fixture in drills (no second merge engine).
    merge_fn: F,
}

impl<'a, F> GitMergePerformer<'a, F>
where
    F: Fn(&MergeRequest) -> Result<String, ActivityError>,
{
    /// Compose the Git merge performer over Git's projection, the PR head, the required-set policy, the
    /// endorsed-context posture, and the actual-merge closure. The `endorsed_contexts` come from the
    /// LIVE [`crate::fork_gate::EndorsementResolver`] (§6.3) — never caller-invented trust.
    pub fn new(
        projection: &'a CheckStatusProjection,
        head_oid: GitOid,
        policy: MergeGatePolicy,
        endorsed_contexts: Vec<CheckContext>,
        merge_fn: F,
    ) -> GitMergePerformer<'a, F> {
        GitMergePerformer {
            projection,
            head_oid,
            policy,
            endorsed_contexts,
            merge_fn,
        }
    }

    /// **Git's authoritative merge gate over its OWN projection (§6.2 + §6.3).** Returns
    /// [`MergeGateOutcome::Admitted`] iff EVERY required context is a current trusted/endorsed success
    /// for the PR head, else [`MergeGateOutcome::Blocked`] with the specific unmet contexts. Reuses
    /// [`evaluate_merge_gate`] (no second gate). Exposed so a drill can assert the gate decision
    /// independently of the merge step.
    pub fn gate_outcome(&self) -> MergeGateOutcome {
        evaluate_merge_gate(
            &self.policy,
            self.projection,
            &self.head_oid,
            &self.endorsed_contexts,
        )
    }

    /// Is a single required `context` satisfied (current trusted/endorsed success) for the PR head?
    /// Reuses [`is_acceptable_satisfaction`] (§6.3) over the projection row — never recomputes trust.
    /// The per-context primitive the gate folds over; exposed for a targeted drill assertion.
    pub fn context_satisfied(&self, context: &CheckContext) -> bool {
        let key = CheckKey {
            commit_oid: self.head_oid.clone(),
            context: context.clone(),
        };
        match self.projection.current(&key) {
            None => false, // a missing required context never satisfies (fail-closed).
            Some(row) => is_acceptable_satisfaction(row, self.endorsed_contexts.contains(context)),
        }
    }
}

impl<F> MergePerformer for GitMergePerformer<'_, F>
where
    F: Fn(&MergeRequest) -> Result<String, ActivityError>,
{
    /// **Perform the merge ONLY when Git's authoritative gate admits (§6.2/§6.3).** The durable
    /// merge-queue body calls this after a `success` `ci.result` rollup; this is Git's last,
    /// authoritative check that EVERY required context is a current trusted/endorsed success — reading
    /// `trust_tier` OFF the projection (never recomputed). On `Admitted` → run the actual git merge
    /// (`merge_fn`). On `Blocked` → REFUSE the merge with a humanised [`ActivityError`] (the flow body
    /// dequeues the PR with the §6.2 reason; **0 under-gated / fork-self-greened merges land**). The
    /// performer makes 0 synchronous CI calls — it reads Git's own projection.
    fn merge(&self, request: &MergeRequest) -> Result<String, ActivityError> {
        match self.gate_outcome() {
            MergeGateOutcome::Admitted => (self.merge_fn)(request),
            MergeGateOutcome::Blocked { unmet } => {
                // Surface a humanised, operator-readable refusal (contract 7.3) — NEVER a raw gate
                // struct. The flow body wraps this into a dequeue with the merge-conflict-class reason
                // (the merge "could not be completed"); the specific unmet contexts are named so the
                // checks panel can humanise WHY the merge was held.
                let names: Vec<String> = unmet
                    .iter()
                    .map(|u| format!("{}/{}", provider_label(&u.context), u.context.name))
                    .collect();
                Err(ActivityError(format!(
                    "the merge gate did not admit: the required check(s) {} are not green-and-current \
                     with an acceptable trust posture (an un-endorsed fork success is neutral for \
                     gating). The pull request was not merged.",
                    names.join(", ")
                )))
            }
        }
    }
}

/// The human-facing provider label for a [`CheckContext`] in a refusal message (`ci` / `external`).
/// PII-free; deterministic (replay-stable).
fn provider_label(context: &CheckContext) -> &'static str {
    use crate::check_status::CheckProvider;
    match context.provider {
        CheckProvider::Ci => "ci",
        CheckProvider::External => "external",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_status::{
        CheckState, CheckStatus, CheckStatusProjection, HumanisedRef, Timestamp, TrustTier,
    };
    use myelin_tenancy::{ArtifactRef, TenantId};
    use std::cell::Cell;
    use std::collections::BTreeMap;

    const HEAD: &str = "deadbeefcafe";
    const REPO: &str = "myelin://acme/git/repo/core";

    fn fact(context: &str, attempt: u32, state: CheckState, trust: TrustTier) -> CheckStatus {
        CheckStatus {
            tenant: TenantId("acme".into()),
            repo: ArtifactRef(REPO.into()),
            commit_oid: GitOid(HEAD.into()),
            context: CheckContext::ci(context),
            state,
            required: true,
            run: ArtifactRef(format!("myelin://acme/ci/run/{attempt}")),
            run_attempt: attempt,
            trust_tier: trust,
            details_ref: ArtifactRef(format!("myelin://acme/ci/run/{attempt}#step-2")),
            summary: HumanisedRef {
                template_key: "ci.check.updated".into(),
                args: BTreeMap::new(),
            },
            started_at: Timestamp("2026-06-22T00:00:00Z".into()),
            completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
            cost_settled: true,
        }
    }

    fn request() -> MergeRequest {
        MergeRequest {
            pr_ref: format!("{REPO}#pr-7"),
            target_ref: "refs/heads/main".into(),
            speculative_commit_oid: HEAD.into(),
            required_contexts: vec!["ci/build".into(), "ci/test".into()],
        }
    }

    fn policy() -> MergeGatePolicy {
        MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test"]).unwrap()
    }

    /// **The gate ADMITS when all required contexts are trusted successes → the merge runs ONCE.** The
    /// performer evaluates Git's gate over its projection and, on admit, calls the actual-merge closure
    /// exactly once.
    #[test]
    fn admits_and_merges_when_all_required_trusted_green() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        proj.apply(&fact("test", 1, CheckState::Success, TrustTier::Trusted));

        let merges = Cell::new(0u32);
        let perf = GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), vec![], |r| {
            merges.set(merges.get() + 1);
            Ok(format!("merged-{}", r.speculative_commit_oid))
        });
        assert!(matches!(perf.gate_outcome(), MergeGateOutcome::Admitted));
        let oid = perf.merge(&request()).expect("admitted → merge");
        assert_eq!(oid, "merged-deadbeefcafe");
        assert_eq!(merges.get(), 1, "the actual merge ran EXACTLY once");
    }

    /// **An un-endorsed fork success is NEUTRAL for gating → the merge is REFUSED (§6.3, GIT-D10 b).**
    /// Even though CI rolled up green, the per-context projection carries `untrusted_fork`; Git's gate
    /// blocks; the performer refuses the merge (0 merge calls) with a humanised reason — 0 forks
    /// self-green their gate at the merge step.
    #[test]
    fn refuses_un_endorsed_fork_success() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        // `test` rolled up green but the run was an untrusted fork — NEUTRAL until endorsed.
        proj.apply(&fact(
            "test",
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let merges = Cell::new(0u32);
        let perf = GitMergePerformer::new(
            &proj,
            GitOid(HEAD.into()),
            policy(),
            vec![], // NOT endorsed
            |_r| {
                merges.set(merges.get() + 1);
                Ok("should-not-run".into())
            },
        );
        assert!(matches!(
            perf.gate_outcome(),
            MergeGateOutcome::Blocked { .. }
        ));
        let err = perf
            .merge(&request())
            .expect_err("an un-endorsed fork success must refuse the merge");
        assert!(
            err.0.contains("the merge gate did not admit"),
            "humanised: {}",
            err.0
        );
        assert!(
            err.0.contains("ci/test"),
            "names the unmet context: {}",
            err.0
        );
        assert!(
            !err.0.contains("Blocked"),
            "no raw gate struct in the reason: {}",
            err.0
        );
        assert_eq!(merges.get(), 0, "0 forks self-green their gate at merge");
    }

    /// **A maintainer ENDORSEMENT of the fork context flips the merge gate green (§6.3, GIT-D10 c).**
    /// With the fork context in the endorsed set, the performer admits and merges once.
    #[test]
    fn endorsed_fork_success_admits_and_merges() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        proj.apply(&fact(
            "test",
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let merges = Cell::new(0u32);
        let perf = GitMergePerformer::new(
            &proj,
            GitOid(HEAD.into()),
            policy(),
            vec![CheckContext::ci("test")], // the maintainer endorsed the fork's `test` run
            |_r| {
                merges.set(merges.get() + 1);
                Ok("merged".into())
            },
        );
        assert!(matches!(perf.gate_outcome(), MergeGateOutcome::Admitted));
        perf.merge(&request())
            .expect("an endorsed fork success admits");
        assert_eq!(merges.get(), 1, "the endorsed fork context merges once");
    }

    /// **A missing required context REFUSES the merge (fail-closed).** A required context with no
    /// projection row (CI never reported it) blocks — the performer refuses (0 merge), even on a
    /// success rollup.
    #[test]
    fn refuses_on_a_missing_required_context() {
        let mut proj = CheckStatusProjection::new();
        // only `build` reported — `test` (required) is missing.
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));

        let perf = GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), vec![], |_r| {
            panic!("must not merge with a missing required context")
        });
        assert!(
            !perf.context_satisfied(&CheckContext::ci("test")),
            "missing → unsatisfied"
        );
        let err = perf
            .merge(&request())
            .expect_err("a missing required context must refuse the merge");
        assert!(
            err.0.contains("ci/test"),
            "names the missing context: {}",
            err.0
        );
    }

    /// **The actual-merge closure surfaces a conflict as an [`ActivityError`] (the gate admitted but the
    /// git merge failed).** The performer admits the gate, calls the merge, and propagates the closure's
    /// conflict error verbatim (the flow body dequeues it).
    #[test]
    fn admitted_gate_propagates_a_merge_conflict() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        proj.apply(&fact("test", 1, CheckState::Success, TrustTier::Trusted));

        let perf = GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), vec![], |_r| {
            Err(ActivityError("merge conflict".into()))
        });
        let err = perf.merge(&request()).expect_err("the conflict propagates");
        assert_eq!(err.0, "merge conflict");
    }

    /// **`context_satisfied` reads trust OFF the fact — a trusted success satisfies, an un-endorsed fork
    /// success does not, an endorsed fork success does.** The per-context primitive the gate folds over.
    #[test]
    fn context_satisfied_reads_trust_off_the_fact() {
        let mut proj = CheckStatusProjection::new();
        proj.apply(&fact("build", 1, CheckState::Success, TrustTier::Trusted));
        proj.apply(&fact(
            "fork",
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let unendorsed =
            GitMergePerformer::new(&proj, GitOid(HEAD.into()), policy(), vec![], |_r| {
                Ok(String::new())
            });
        assert!(
            unendorsed.context_satisfied(&CheckContext::ci("build")),
            "trusted success satisfies"
        );
        assert!(
            !unendorsed.context_satisfied(&CheckContext::ci("fork")),
            "an un-endorsed fork success does not satisfy"
        );

        let endorsed = GitMergePerformer::new(
            &proj,
            GitOid(HEAD.into()),
            policy(),
            vec![CheckContext::ci("fork")],
            |_r| Ok(String::new()),
        );
        assert!(
            endorsed.context_satisfied(&CheckContext::ci("fork")),
            "an endorsed fork success satisfies"
        );
    }
}
