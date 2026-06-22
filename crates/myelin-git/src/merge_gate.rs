//! # `merge_gate` — the merge gate + the required-set policy (GIT-P21 / P-282, M3)
//!
//! **Git owns what is allowed to land.** This module is the bridge the merge gate fires across: it
//! reads Git's OWN [`check_status`](crate::check_status) projection (a mirror of CI's facts) against a
//! `base_ref`'s branch-protection [`BranchProtectionRuleset`](crate::lifecycle::BranchProtectionRuleset)
//! `required_contexts`, and decides whether a PR's `head_oid` is **green-and-current** on every
//! required context. **Git decides which contexts gate (the required-set policy); CI only reports the
//! facts.** Git **reads `trust_tier` off the fact** — it never recomputes trust, and it **never
//! synchronously calls CI** (it reads its own projection — EI-01 §7 / EI-02 §3, acyclic by
//! construction).
//!
//! **Owning architecture:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md`
//! §6 (the merge gate + the required-set policy) + §6.2 (the "what is allowed to land" decision) +
//! §6.3 (the fork / trust-tier gate — the GIT-P22 follow-on). `00-overview.md` §1.1 (Git owns what is
//! allowed to land; reads `trust_tier` off the fact, never recomputes). **Contract:** index row **5.9**
//! (the required-set policy — `ruleset.required_contexts`; CI reports facts, Git decides which contexts
//! gate this `base_ref`). **Reconciliation:** X-1 (the most load-bearing cross-subsystem seam).
//!
//! ## What GIT-P21 (this prompt) ships — and the named follow-ons
//! GIT-P20 ([`crate::check_status`]) landed the projection + the monotonic `run_attempt` supersession +
//! the PURE per-context [`gate_outcome`](crate::check_status::gate_outcome) over typed
//! [`CheckContext`](crate::check_status::CheckContext)s. GIT-P16 ([`crate::lifecycle`]) landed the
//! [`BranchProtectionRuleset`](crate::lifecycle::BranchProtectionRuleset) entity whose
//! `required_contexts: Vec<String>` NAMES which contexts gate a `base_ref` (in the `provider/name`
//! string grammar, e.g. `"ci/build"`). **This module is the MERGE GATE that wires those two together:**
//! it parses a ruleset's `required_contexts` strings into typed [`CheckContext`]s
//! ([`parse_required_context`]), resolves each against the live projection for the PR head, applies the
//! fork-endorsement trust posture ([`is_acceptable_satisfaction`]), and returns the typed
//! [`MergeGateOutcome`] — the **0-under-gated-merges** decision.
//!
//! The bridge is the genuinely-new GIT-P21 deliverable (EI-01 §7 — extend/reconcile, never duplicate):
//! the per-context gate LOGIC lives in [`crate::check_status::gate_outcome`] (reused, not re-defined);
//! this module adds the **ruleset→projection resolution + the live-store gate read**.
//!
//! ## FLOORS named (per the prompt)
//! - **The fork / trust-tier endorsement gate is GIT-P22** — this module CONSUMES an `endorsed: bool`
//!   posture per context (the [`is_acceptable_satisfaction`](crate::check_status::is_acceptable_satisfaction)
//!   input), but the LIVE `check(subject, approve_untrusted_ci, repo)` resolution that PRODUCES that
//!   bool (and the `fork:<pr_id>` trust-scoped cache confinement) is GIT-P22. Here the endorsement set
//!   is an explicit input — the gate already BLOCKS an un-endorsed `untrusted_fork` success (the
//!   neutral-for-gating rule holds), so a fork cannot self-green its required gate even before P22
//!   wires the live endorsement check.
//! - **The merge queue (durable workflow, exactly-once merge, the `ci.result` rollup wait) is
//!   GIT-P23** — this module is the SYNCHRONOUS gate the queue's per-PR step calls; the durable
//!   serialisation + the long-park-on-`ci.result` is GIT-P23.

use crate::check_status::{
    is_acceptable_satisfaction, CheckContext, CheckProvider, CheckState, CheckStatusProjection,
    CheckStatusRow, GitOid,
};

// ---------------------------------------------------------------------------
// 1. The required-set policy — Git decides which contexts gate this base_ref
// ---------------------------------------------------------------------------

/// **The required-set policy for a `base_ref` (contract 5.9 — owned).** The merge gate's input: the set
/// of contexts a ruleset requires to be green-and-current (with an acceptable trust posture) for a
/// merge into the protected ref. **This is Git's policy — CI reports facts, Git decides which contexts
/// gate** (X-1 / Δ1). Derived from a [`BranchProtectionRuleset`](crate::lifecycle::BranchProtectionRuleset)'s
/// `required_contexts` strings via [`MergeGatePolicy::from_required_contexts`].
///
/// Distinct from [`crate::check_status::RequiredSetPolicy`] (which GIT-P20 declared over typed
/// [`CheckContext`]s for the pure per-context gate): THIS type is the GATE-level policy that also
/// carries the **per-context endorsement posture** (the GIT-P22 fork-endorsement input) and resolves
/// from the ruleset's STRING context grammar. It REUSES the per-context gate logic — it does not
/// re-define it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MergeGatePolicy {
    /// The contexts that gate the target ref (typed — resolved from the ruleset's `provider/name`
    /// strings). A merge is BLOCKED unless every one has a CURRENT projection row with
    /// `state = success` AND an acceptable trust posture (trusted, or an endorsed `untrusted_fork`).
    pub required: Vec<CheckContext>,
}

impl MergeGatePolicy {
    /// Build the gate policy from a ruleset's `required_contexts` strings (the `provider/name` grammar,
    /// e.g. `"ci/build"`, `"external/sonarcloud"`). A malformed context string is a LOUD
    /// [`RequiredContextParseError`] — the merge gate must never silently treat an unparseable required
    /// context as "not required" (that would be an under-gated merge). Returns the parsed policy or the
    /// first parse error.
    pub fn from_required_contexts<S: AsRef<str>>(
        contexts: &[S],
    ) -> Result<MergeGatePolicy, RequiredContextParseError> {
        let mut required = Vec::with_capacity(contexts.len());
        for c in contexts {
            required.push(parse_required_context(c.as_ref())?);
        }
        Ok(MergeGatePolicy { required })
    }

    /// Does this policy gate on `context`?
    pub fn requires(&self, context: &CheckContext) -> bool {
        self.required.contains(context)
    }

    /// Is the required set empty (an unprotected-by-checks ref)? An empty required set is a VALID
    /// policy (a ref may gate on approvals only) — the gate admits on checks alone, but the lifecycle
    /// ruleset gate ([`crate::lifecycle::evaluate_ruleset`]) still enforces approvals/CODEOWNERS/etc.
    pub fn is_empty(&self) -> bool {
        self.required.is_empty()
    }
}

/// Parse a ruleset `required_contexts` string in the `provider/name` grammar into a typed
/// [`CheckContext`]. The grammar (matching the e2e + entity-layer convention, e.g. `"ci/build"`):
/// - `"ci/<name>"` → `CheckContext{ Ci, <name> }` (a Myelin-CI context);
/// - `"external/<name>"` → `CheckContext{ External, <name> }` (a third-party status);
/// - a BARE `"<name>"` (no `/`) → `CheckContext{ Ci, <name> }` (defaults to the CI provider — the
///   common case; a ruleset author writes `"build"` and means the CI build context).
///
/// A name may itself contain `/` (e.g. `"ci/test/unit"` → `Ci, "test/unit"`) — only the FIRST segment
/// is consumed as the provider, and only when it is a KNOWN provider keyword (`ci`/`external`). An
/// empty string (or an empty name) is a LOUD error — never silently dropped (an unparseable required
/// context that silently vanished would be an under-gated merge).
pub fn parse_required_context(s: &str) -> Result<CheckContext, RequiredContextParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(RequiredContextParseError::Empty);
    }
    match s.split_once('/') {
        Some(("ci", name)) if !name.is_empty() => Ok(CheckContext {
            provider: CheckProvider::Ci,
            name: name.to_string(),
        }),
        Some(("external", name)) if !name.is_empty() => Ok(CheckContext {
            provider: CheckProvider::External,
            name: name.to_string(),
        }),
        // A `/`-bearing string whose first segment is NOT a known provider keyword (e.g.
        // `"team/foo"`), OR a known provider with an EMPTY name (`"ci/"`): the whole string is the
        // name under the default CI provider — EXCEPT `"ci/"`/`"external/"` with an empty name, which
        // is malformed.
        Some(("ci", "")) | Some(("external", "")) => {
            Err(RequiredContextParseError::EmptyName { raw: s.to_string() })
        }
        Some(_) => Ok(CheckContext {
            provider: CheckProvider::Ci,
            name: s.to_string(),
        }),
        // No `/` — a bare name under the default CI provider.
        None => Ok(CheckContext {
            provider: CheckProvider::Ci,
            name: s.to_string(),
        }),
    }
}

/// A malformed `required_contexts` string — surfaced LOUDLY so the merge gate never silently treats an
/// unparseable required context as absent (that would be an under-gated merge). Humanisable; never a
/// raw rendered string into the checks panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequiredContextParseError {
    /// The required-context string was empty (or whitespace-only).
    Empty,
    /// A provider keyword (`ci`/`external`) with an empty name (e.g. `"ci/"`). Carries the raw string.
    EmptyName {
        /// The malformed raw string.
        raw: String,
    },
}

impl std::fmt::Display for RequiredContextParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequiredContextParseError::Empty => {
                write!(f, "a required_contexts entry was empty")
            }
            RequiredContextParseError::EmptyName { raw } => {
                write!(
                    f,
                    "a required_contexts entry has a provider but no name: {raw:?}"
                )
            }
        }
    }
}

impl std::error::Error for RequiredContextParseError {}

// ---------------------------------------------------------------------------
// 2. The merge gate — the "what is allowed to land" decision (§6.2)
// ---------------------------------------------------------------------------

/// **The merge-gate outcome for a PR head (§6.2 — the "what is allowed to land" decision).** The
/// required-set half of the gate: either every required context is green-and-current with an acceptable
/// trust posture, or the SPECIFIC unmet contexts (each with WHY — missing, not-green, or un-endorsed
/// fork) are surfaced. **0 merges are admitted with a missing/stale/un-endorsed required context** (the
/// gate is the only path to `Admitted`).
///
/// This is the required-SET half only. The full `may_merge` (§6.2) also folds in `Id.check(merge)`,
/// approvals/CODEOWNERS ([`crate::lifecycle::evaluate_ruleset`]), and the agent-needs-human HITL gate —
/// those are the lifecycle/ReBAC halves the merge tool (GIT-P30) and the durable queue (GIT-P23)
/// compose ON TOP of this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeGateOutcome {
    /// Every required context has a CURRENT `success` row with an acceptable trust posture — the
    /// required-set gate is GREEN (the merge may proceed once the other `may_merge` legs pass).
    Admitted,
    /// At least one required context is unmet — the merge is BLOCKED. Carries the specific unmet
    /// contexts + WHY (humanised into the PR checks panel by Notif — never a raw string).
    Blocked {
        /// The specific contexts that are not satisfied, each with its block reason (≥ 1).
        unmet: Vec<UnmetContext>,
    },
}

impl MergeGateOutcome {
    /// `true` exactly when the required-set gate admits the merge (the `gate_satisfied` the merge tool
    /// reads). A `Blocked` outcome is the 0-under-gated-merges guard.
    pub fn is_admitted(&self) -> bool {
        matches!(self, MergeGateOutcome::Admitted)
    }
}

/// A required context that is NOT satisfied + the reason — the loud, typed "why this merge is blocked".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnmetContext {
    /// The required context that is unmet.
    pub context: CheckContext,
    /// Why it is unmet.
    pub reason: UnmetReason,
}

/// Why a required context did not satisfy the merge gate (humanisable; never a raw string).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnmetReason {
    /// No CURRENT projection row for `(head_oid, context)` — CI has not (yet) reported this context for
    /// the head commit. The gate treats a missing required context as BLOCKING (fail-closed).
    Missing,
    /// A current row exists but its state is not a success (`failure`/`error`/`cancelled`/pending/
    /// `neutral`). Carries the actual state.
    NotGreen {
        /// The current (non-success) state.
        state: CheckState,
    },
    /// A current `success` row exists but its trust posture is unacceptable: an `untrusted_fork`
    /// success that has NOT been endorsed (or re-run trusted). **Neutral for gating** — a fork must
    /// never turn its own required gate green by running attacker-controlled CI config (Δ3, the
    /// poisoned-pipeline defence). The LIVE endorsement resolution is GIT-P22.
    UntrustedForkNeutral,
}

/// **THE MERGE GATE (§6.2 — the required-set policy, the in-memory projection read).** Evaluate the
/// [`MergeGatePolicy`] against the CURRENT [`CheckStatusProjection`] rows for the PR's `head_oid`,
/// given the set of fork-ENDORSED contexts (the maintainer `approve_untrusted_ci` endorsements — the
/// GIT-P22 input). Returns [`MergeGateOutcome::Admitted`] iff EVERY required context has a current
/// `success` row with an acceptable trust posture, else [`MergeGateOutcome::Blocked`] with the specific
/// unmet contexts + reasons.
///
/// **Git reads its OWN projection — it never synchronously calls CI** (acyclic, EI-02 §3). It reads
/// `trust_tier` OFF the fact via [`is_acceptable_satisfaction`] — it never recomputes trust. The
/// 0-under-gated-merges invariant: a missing OR not-green OR un-endorsed-fork required context is
/// ALWAYS in `unmet` (fail-closed on every non-green posture).
pub fn evaluate_merge_gate(
    policy: &MergeGatePolicy,
    projection: &CheckStatusProjection,
    head_oid: &GitOid,
    endorsed_contexts: &[CheckContext],
) -> MergeGateOutcome {
    let mut unmet: Vec<UnmetContext> = Vec::new();
    for ctx in &policy.required {
        match resolve_context(projection, head_oid, ctx, endorsed_contexts) {
            None => {}
            Some(reason) => unmet.push(UnmetContext {
                context: ctx.clone(),
                reason,
            }),
        }
    }
    if unmet.is_empty() {
        MergeGateOutcome::Admitted
    } else {
        MergeGateOutcome::Blocked { unmet }
    }
}

/// Resolve one required context against the projection: `None` if it is satisfied (current success,
/// acceptable trust), else `Some(reason)`. The single-context primitive the in-memory gate and the
/// live-store gate ([`evaluate_merge_gate_row`]) share — so the in-memory + the DB paths apply the
/// IDENTICAL trust/state logic (no drift).
fn resolve_context(
    projection: &CheckStatusProjection,
    head_oid: &GitOid,
    ctx: &CheckContext,
    endorsed_contexts: &[CheckContext],
) -> Option<UnmetReason> {
    let key = crate::check_status::CheckKey {
        commit_oid: head_oid.clone(),
        context: ctx.clone(),
    };
    match projection.current(&key) {
        None => Some(UnmetReason::Missing),
        Some(row) => classify_row(row, endorsed_contexts.contains(ctx)),
    }
}

/// Classify a CURRENT projection row for a required context: `None` if it satisfies (success +
/// acceptable trust), else the specific [`UnmetReason`]. **Reads `trust_tier` off the row, never
/// recomputes it.** Reuses [`is_acceptable_satisfaction`] (GIT-P20) for the trust posture — this only
/// adds the loud WHY classification on top.
fn classify_row(row: &CheckStatusRow, endorsed: bool) -> Option<UnmetReason> {
    if is_acceptable_satisfaction(row, endorsed) {
        return None;
    }
    // Not acceptable — surface WHY (loud, typed). A non-success state is NotGreen; a success that
    // failed the trust posture is an un-endorsed untrusted-fork (the only way a success is unacceptable).
    if !row.state.is_success() {
        Some(UnmetReason::NotGreen { state: row.state })
    } else {
        Some(UnmetReason::UntrustedForkNeutral)
    }
}

/// **The single-context merge-gate decision over an OPTIONAL current row** — the primitive the
/// LIVE-STORE gate (`check_status_store`) calls per required context after fetching the row from
/// Postgres. `None` row → [`UnmetReason::Missing`]; a present row is classified by [`classify_row`].
/// `None` return ⇒ the context is satisfied. Exported so the store-backed gate applies the IDENTICAL
/// state/trust logic as the in-memory gate (the DB path and the in-memory path can never drift).
pub fn evaluate_merge_gate_row(
    row: Option<&CheckStatusRow>,
    endorsed: bool,
) -> Option<UnmetReason> {
    match row {
        None => Some(UnmetReason::Missing),
        Some(r) => classify_row(r, endorsed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check_status::{CheckStatus, HumanisedRef, Timestamp, TrustTier};
    use myelin_tenancy::{ArtifactRef, TenantId};
    use std::collections::BTreeMap;

    fn fact(
        commit: &str,
        ctx: CheckContext,
        attempt: u32,
        state: CheckState,
        trust: TrustTier,
    ) -> CheckStatus {
        CheckStatus {
            tenant: TenantId("acme".into()),
            repo: ArtifactRef("myelin://acme/git/repo/core".into()),
            commit_oid: GitOid(commit.into()),
            context: ctx,
            state,
            required: true,
            run: ArtifactRef("myelin://acme/ci/run/1".into()),
            run_attempt: attempt,
            trust_tier: trust,
            details_ref: ArtifactRef("myelin://acme/ci/run/1#step-3".into()),
            summary: HumanisedRef {
                template_key: "ci.check.updated".into(),
                args: BTreeMap::new(),
            },
            started_at: Timestamp("2026-06-22T00:00:00Z".into()),
            completed_at: Some(Timestamp("2026-06-22T00:01:00Z".into())),
            cost_settled: true,
        }
    }

    // ---- the required-context parse grammar ----

    #[test]
    fn parse_ci_provider_prefixed_context() {
        assert_eq!(
            parse_required_context("ci/build").unwrap(),
            CheckContext::ci("build")
        );
    }

    #[test]
    fn parse_external_provider_prefixed_context() {
        assert_eq!(
            parse_required_context("external/sonarcloud").unwrap(),
            CheckContext::external("sonarcloud")
        );
    }

    #[test]
    fn parse_bare_name_defaults_to_ci() {
        assert_eq!(
            parse_required_context("build").unwrap(),
            CheckContext::ci("build")
        );
    }

    #[test]
    fn parse_name_with_slash_keeps_provider_prefix() {
        // "ci/test/unit" → Ci, "test/unit" (only the first KNOWN provider segment is consumed).
        assert_eq!(
            parse_required_context("ci/test/unit").unwrap(),
            CheckContext::ci("test/unit")
        );
    }

    #[test]
    fn parse_unknown_first_segment_is_a_ci_name() {
        // "team/foo" — `team` is not a provider keyword → the whole string is a CI context name.
        assert_eq!(
            parse_required_context("team/foo").unwrap(),
            CheckContext::ci("team/foo")
        );
    }

    #[test]
    fn parse_empty_is_a_loud_error() {
        assert_eq!(
            parse_required_context("").unwrap_err(),
            RequiredContextParseError::Empty
        );
        assert_eq!(
            parse_required_context("   ").unwrap_err(),
            RequiredContextParseError::Empty
        );
    }

    #[test]
    fn parse_provider_without_name_is_a_loud_error() {
        // Both provider keywords with an empty name are EmptyName errors (the `!name.is_empty()` guard
        // on the success arms is load-bearing — without it `"ci/"` would parse to an empty-named ctx).
        assert_eq!(
            parse_required_context("ci/").unwrap_err(),
            RequiredContextParseError::EmptyName { raw: "ci/".into() }
        );
        assert_eq!(
            parse_required_context("external/").unwrap_err(),
            RequiredContextParseError::EmptyName {
                raw: "external/".into()
            }
        );
        // The Display surfaces the loud, humanisable message (never a silent drop).
        assert_eq!(
            parse_required_context("ci/").unwrap_err().to_string(),
            "a required_contexts entry has a provider but no name: \"ci/\""
        );
        assert_eq!(
            RequiredContextParseError::Empty.to_string(),
            "a required_contexts entry was empty"
        );
    }

    #[test]
    fn outcome_predicates_distinguish_admit_from_block() {
        // is_admitted is load-bearing — the merge tool reads it as the gate_satisfied guard.
        assert!(MergeGateOutcome::Admitted.is_admitted());
        assert!(!MergeGateOutcome::Blocked {
            unmet: vec![UnmetContext {
                context: CheckContext::ci("build"),
                reason: UnmetReason::Missing,
            }]
        }
        .is_admitted());
    }

    #[test]
    fn policy_is_empty_distinguishes_empty_from_non_empty() {
        assert!(MergeGatePolicy::default().is_empty());
        assert!(!MergeGatePolicy::from_required_contexts(&["ci/build"])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn policy_from_ruleset_strings() {
        let policy =
            MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test", "external/scan"])
                .unwrap();
        assert_eq!(policy.required.len(), 3);
        assert!(policy.requires(&CheckContext::ci("build")));
        assert!(policy.requires(&CheckContext::external("scan")));
        assert!(!policy.requires(&CheckContext::ci("lint")));
    }

    #[test]
    fn policy_from_ruleset_propagates_parse_error() {
        assert!(MergeGatePolicy::from_required_contexts(&["ci/build", ""]).is_err());
    }

    // ---- the merge gate (the 0-under-gated-merges core) ----

    #[test]
    fn gate_admits_when_all_required_green_trusted() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));
        proj.apply(&fact(
            "h1",
            CheckContext::ci("test"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test"]).unwrap();
        assert_eq!(
            evaluate_merge_gate(&policy, &proj, &head, &[]),
            MergeGateOutcome::Admitted
        );
    }

    #[test]
    fn gate_blocks_on_missing_required_context() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        // build is green; test is REQUIRED but never reported.
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build", "ci/test"]).unwrap();
        match evaluate_merge_gate(&policy, &proj, &head, &[]) {
            MergeGateOutcome::Blocked { unmet } => {
                assert_eq!(unmet.len(), 1);
                assert_eq!(unmet[0].context, CheckContext::ci("test"));
                assert_eq!(unmet[0].reason, UnmetReason::Missing);
            }
            MergeGateOutcome::Admitted => panic!("a missing required context must BLOCK"),
        }
    }

    #[test]
    fn gate_blocks_on_failing_required_context() {
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Failure,
            TrustTier::Trusted,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
        match evaluate_merge_gate(&policy, &proj, &head, &[]) {
            MergeGateOutcome::Blocked { unmet } => {
                assert_eq!(
                    unmet[0].reason,
                    UnmetReason::NotGreen {
                        state: CheckState::Failure
                    }
                );
            }
            MergeGateOutcome::Admitted => panic!("a failing required context must BLOCK"),
        }
    }

    #[test]
    fn gate_blocks_on_stale_pending_required_context() {
        // A required context whose CURRENT row is still in_progress (not terminal) BLOCKS — the merge
        // is not admitted until it goes green (fail-closed on a not-yet-green context).
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::InProgress,
            TrustTier::Trusted,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
        match evaluate_merge_gate(&policy, &proj, &head, &[]) {
            MergeGateOutcome::Blocked { unmet } => {
                assert_eq!(
                    unmet[0].reason,
                    UnmetReason::NotGreen {
                        state: CheckState::InProgress
                    }
                );
            }
            MergeGateOutcome::Admitted => panic!("a pending required context must BLOCK"),
        }
    }

    #[test]
    fn gate_blocks_un_endorsed_untrusted_fork_success() {
        // Δ3: an un-endorsed untrusted_fork success is NEUTRAL for gating — the gate BLOCKS with the
        // distinct UntrustedForkNeutral reason (a fork cannot self-green its required gate).
        let mut proj = CheckStatusProjection::new();
        let head = GitOid("h1".into());
        proj.apply(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));

        let policy = MergeGatePolicy::from_required_contexts(&["ci/build"]).unwrap();
        // Un-endorsed → blocked (neutral-for-gating).
        match evaluate_merge_gate(&policy, &proj, &head, &[]) {
            MergeGateOutcome::Blocked { unmet } => {
                assert_eq!(unmet[0].reason, UnmetReason::UntrustedForkNeutral);
            }
            MergeGateOutcome::Admitted => panic!("an un-endorsed fork success must BLOCK"),
        }
        // Endorsed (the GIT-P22 input) → admitted.
        assert_eq!(
            evaluate_merge_gate(&policy, &proj, &head, &[CheckContext::ci("build")]),
            MergeGateOutcome::Admitted,
            "a maintainer-endorsed fork success admits"
        );
    }

    #[test]
    fn empty_required_set_admits_on_checks_alone() {
        // A ref that gates on approvals only (empty required_contexts) admits the CHECKS half — the
        // approvals/CODEOWNERS half is the lifecycle ruleset gate, composed on top.
        let proj = CheckStatusProjection::new();
        let policy = MergeGatePolicy::default();
        assert!(policy.is_empty());
        assert_eq!(
            evaluate_merge_gate(&policy, &proj, &GitOid("h1".into()), &[]),
            MergeGateOutcome::Admitted
        );
    }

    #[test]
    fn the_in_memory_and_row_paths_agree() {
        // The store-backed gate primitive (evaluate_merge_gate_row) applies the IDENTICAL classify
        // logic as the in-memory gate — a row that the in-memory gate admits, the row primitive admits.
        let f = fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::Trusted,
        );
        let row = CheckStatusRow::from_fact(&f);
        assert_eq!(
            evaluate_merge_gate_row(Some(&row), false),
            None,
            "trusted success satisfies"
        );
        assert_eq!(
            evaluate_merge_gate_row(None, false),
            Some(UnmetReason::Missing),
            "absent → missing"
        );

        let fork = CheckStatusRow::from_fact(&fact(
            "h1",
            CheckContext::ci("build"),
            1,
            CheckState::Success,
            TrustTier::UntrustedFork,
        ));
        assert_eq!(
            evaluate_merge_gate_row(Some(&fork), false),
            Some(UnmetReason::UntrustedForkNeutral),
            "un-endorsed fork success → neutral"
        );
        assert_eq!(
            evaluate_merge_gate_row(Some(&fork), true),
            None,
            "endorsed fork success satisfies"
        );
    }
}
