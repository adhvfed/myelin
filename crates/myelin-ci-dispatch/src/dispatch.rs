//! **The CI Trigger & Dispatch BEHAVIOUR (CI-P10 / P-353, M4): the `EventMatcher` (= `QueryAst`)
//! + exactly-once dedup + the trust-tier evaluation and the single stamp.**
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §1 (trigger → dispatch: 1. **match** via the `EventMatcher` (= the frozen `QueryAst`, contract
//! 3.4 — NOT CEL, NOT a CI trigger DSL); 2. **dedup** on the triggering `event_id` via the
//! `consumer_dedup` ledger (contract 2.5 — exactly-once *effect* under at-least-once delivery); 3.
//! **trust-tier evaluation + the single stamp** — classify the run `Trusted | UntrustedFork |
//! SelfHosted` from run provenance + the ReBAC ABAC edge `read & !is_untrusted_fork` (contract 4.9),
//! stamped ONCE onto BOTH `JobSpec.trust_tier` AND every `CheckStatus.trust_tier`, X-1);
//! `01-tech-and-data-model.md` §2 (the `trust_tier` is one value stamped once). **Reconciliation:**
//! `00-reconciliation-decisions.md` X-1 (the trust_tier stamped by CI from run provenance; an
//! `untrusted_fork` success is neutral for gating), §OQ-C (the `QueryAst` is the one expression
//! language — not CEL). **Contracts consumed:** 3.4 (`EventMatcher` = `QueryAst`), 2.5
//! (`consumer_dedup`), 4.2 (`check` + `CaveatContext`), 4.9 (the `read & !is_untrusted_fork` edge).
//!
//! ## This module REUSES the one engine — it does NOT re-define a trigger language or a trust enum
//! - The matcher is [`myelin_query::EventMatcher`] (= the frozen [`QueryAst`](myelin_query::QueryAst),
//!   3.4) — a `pull_request` trigger compiles to a `QueryAst`, NOT a CI-specific DSL, NOT CEL
//!   (OQ-C). [`compile_trigger`] lowers an [`OnTrigger`] spec to that one matcher.
//! - The dedup ledger is the `consumer_dedup` table CI-P6 (P-349) shipped in [`crate::migrations`];
//!   the exactly-once `INSERT … ON CONFLICT (consumer, event_id) DO NOTHING` semantics are modelled
//!   deterministically by [`DedupLedger`] (the in-memory shape the live `apply` rides — the SAME
//!   `(consumer, event_id)` key).
//! - The trust tiers are the ALREADY-FROZEN enums: [`myelin_ci_sandbox::TrustTier`] (the full 3-way
//!   `Trusted | UntrustedFork | SelfHosted` that gates the `JobSpec`) and
//!   [`myelin_git::check_status::TrustTier`] (the 2-way `Trusted | UntrustedFork` the `CheckStatus`
//!   carries for the merge gate, X-1). This module does NOT define a third trust enum; it
//!   EVALUATES the tier once ([`classify_trust`]) and STAMPS it consistently onto both
//!   ([`stamp_trust`] / [`git_trust_of`]).
//!
//! ## The single-stamp consistency invariant (the security-critical half, X-1)
//! Trust is evaluated EXACTLY ONCE into the full [`TrustTier`](myelin_ci_sandbox::TrustTier). The
//! `CheckStatus` half is then a TOTAL, DETERMINISTIC projection ([`git_trust_of`]): `UntrustedFork`
//! → git `UntrustedFork`; `Trusted` and `SelfHosted` → git `Trusted` (a member/self-hosted run is
//! trusted *code* for gating — the fork exclusion is the only thing the merge gate cares about,
//! X-1). Because both stamps derive from the ONE evaluated value, a fork PR is `UntrustedFork` on
//! BOTH the `JobSpec` AND the `CheckStatus` with **0 divergence** — the gate-drill invariant. There
//! is no second classification path that could stamp the two halves inconsistently.
//!
//! ## Floor named (the prompt's DoD)
//! The definition resolution → content-addressed snapshot + the reserve/start handoff is **CI-P11**
//! (P-354); the sandboxed dynamic-generation escape hatch is wired in **CI-P11**. This module stops
//! at the matched, deduped, trust-stamped trigger — it produces the stamped [`TrustTier`] + the
//! dedup verdict; it does NOT yet read `.myelin/ci.*`, build the CAS snapshot, or call
//! `DurableExecutor::start`. The live-DB `consumer_dedup` apply is proven by CI-P6's integration
//! test; this module's exactly-once *effect* is proven deterministically here (the drill: deliver
//! the trigger twice → exactly one effect).

use std::collections::BTreeSet;

use myelin_events::{EventEnvelope, EventId};
use myelin_git::check_status::TrustTier as GitTrustTier;
use myelin_identity::{Literal, ObjectType, SetExpr};
use myelin_query::{CmpOp, EventMatcher, Expr, Predicate, RelMembership};

pub use myelin_ci_sandbox::TrustTier;

/// The dedup `consumer` name Trigger & Dispatch records its exactly-once effect under (the first
/// half of the `consumer_dedup` PK `(consumer, event_id)`, contract 2.5 / arch 01 §3.8). One stable
/// consumer name for the trigger leg, so a triggering `event_id` is deduped against exactly this
/// consumer's prior effects.
pub const TRIGGER_CONSUMER: &str = "ci-dispatch.trigger";

/// The `run` ReBAC object type (the `is_untrusted_fork`-stamped object, contract 4.9 / arch
/// identity §5.2). The trust evaluation reads the `read & !is_untrusted_fork` edge over THIS type.
pub const RUN_OBJECT_TYPE: &str = "run";

// ---------------------------------------------------------------------------------------------
// 1. The EventMatcher — a project's armed trigger compiles to the frozen QueryAst (contract 3.4).
// ---------------------------------------------------------------------------------------------

/// The supported triggering-event kinds (arch 02 §1: `git.ref.updated`,
/// `git.pull_request.synchronized`, `git.pr.opened`, `issue.transitioned`, manual, schedule, agent
/// request). Each lowers to an `event.type ==` pin on the one [`QueryAst`] — CI does NOT invent a
/// trigger language; an `on: pull_request: {...}` IS a `QueryAst`.
///
/// **DEVIATION (documented):** the prompt's prose names `git.pull_request.synchronized`; the FROZEN
/// Git event taxonomy (`myelin_git::events`) names the same event `git.pr.synchronized` (and
/// `git.pr.opened`). This module compiles against the FROZEN constants so the matcher's
/// `event.type` pin is byte-identical to what Git's outbox actually emits (no drift). EI-01 §1: the
/// real frozen name wins over the prose sketch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnTrigger {
    /// `on: push` — a member push moves a ref (`git.ref.updated`).
    Push,
    /// `on: pull_request` — a PR is opened or its head is synchronized (a fork PR is the
    /// untrusted-fork case). Matches `git.pr.opened` OR `git.pr.synchronized`.
    PullRequest,
    /// `on: issue` — an issue transitions (`issue.transitioned`).
    IssueTransitioned,
    /// `on: manual` — a manual API dispatch (`ci.run.requested`).
    Manual,
    /// `on: schedule` — a schedule-timer tick (`ci.schedule.tick`).
    Schedule,
    /// `on: agent` — an agent dispatch request (`agent.ci.requested`).
    Agent,
}

impl OnTrigger {
    /// The frozen dotted event type(s) this trigger fires on. A `PullRequest` arms on TWO event
    /// types (opened OR synchronized), so the head of every PR change runs CI.
    pub fn event_types(&self) -> &'static [&'static str] {
        match self {
            OnTrigger::Push => &[myelin_git::events::GIT_REF_UPDATED],
            OnTrigger::PullRequest => &[
                myelin_git::events::GIT_PR_OPENED,
                myelin_git::events::GIT_PR_SYNCHRONIZED,
            ],
            OnTrigger::IssueTransitioned => &["issue.transitioned"],
            OnTrigger::Manual => &["ci.run.requested"],
            OnTrigger::Schedule => &["ci.schedule.tick"],
            OnTrigger::Agent => &["agent.ci.requested"],
        }
    }
}

/// A predicate `event.type == <ty>` over the projected envelope (the one [`Expr`]/[`CmpOp`] core —
/// no CI DSL, no CEL).
fn type_eq(ty: &str) -> Predicate {
    Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("event.type".into()),
        rhs: Expr::Lit(Literal::Str(ty.into())),
    }
}

/// **Compile a project's armed trigger to the shared bounded [`EventMatcher`] (= the frozen
/// `QueryAst`, contract 3.4).** An `on: pull_request: {...}` becomes a `QueryAst` over the projected
/// envelope (`event.type == "git.pr.opened" OR event.type == "git.pr.synchronized"`), NOT a
/// CI-specific trigger language and NOT CEL (OQ-C). The matcher selects the [`RUN_OBJECT_TYPE`]
/// (`run`) object space — its permission compose is leak-free by construction (the matcher tests
/// visibility FIRST, §4.5). The over-budget AST is rejected at compile time (the DoS-hardening
/// property the one engine already enforces).
///
/// Returns the matcher, or [`myelin_query::PredicateError`] if the lowered predicate were
/// over-budget (it never is for a fixed trigger — the surface is bounded by construction).
pub fn compile_trigger(on: &OnTrigger) -> Result<EventMatcher, myelin_query::PredicateError> {
    let types = on.event_types();
    let predicate = if types.len() == 1 {
        type_eq(types[0])
    } else {
        Predicate::Or(types.iter().map(|t| type_eq(t)).collect())
    };
    EventMatcher::compile(ObjectType(RUN_OBJECT_TYPE.into()), predicate)
}

/// **The match decision for an armed trigger** — does this triggering event arm a run? Delegates to
/// the one [`EventMatcher::matches`] (permission-aware BY CONSTRUCTION: the viewer's `visible` set
/// is tested before the predicate, the 0-leak invariant). `member_oracle` answers the relational
/// `SetExpr` arms for the candidate run object (the consumer's authz reverse-index lookup).
pub fn trigger_matches(
    matcher: &EventMatcher,
    envelope: &EventEnvelope,
    visible: &SetExpr,
    member_oracle: &dyn Fn(&RelMembership) -> bool,
) -> Result<bool, myelin_query::EvalError> {
    matcher.matches(envelope, visible, member_oracle)
}

// ---------------------------------------------------------------------------------------------
// 2. Exactly-once dedup — the consumer_dedup ledger (contract 2.5): one push = one run.
// ---------------------------------------------------------------------------------------------

/// **The exactly-once-effect ledger (contract 2.5 / arch 01 §3.8).** Models the `consumer_dedup`
/// table's `INSERT … ON CONFLICT (consumer, event_id) DO NOTHING` deterministically: [`record`] is
/// the first-write-wins guard. The FIRST delivery of a triggering `event_id` records the row and
/// returns `true` (fire the effect — start one run); EVERY redelivery of the SAME `event_id` hits
/// the conflict and returns `false` (the at-least-once transport's duplicate is absorbed — NO second
/// run). This is the exactly-once *effect* under at-least-once *delivery* (Helland 2012).
///
/// The live table (CI-P6, [`crate::migrations`]) is the durable backing; this in-memory ledger is
/// the deterministic shape the live `apply` rides and the unit/drill harness exercises (the SAME
/// `(consumer, event_id)` key). It is NOT a second dedup mechanism — it is the same guard, modelled.
///
/// [`record`]: DedupLedger::record
#[derive(Clone, Debug, Default)]
pub struct DedupLedger {
    /// The recorded `(consumer, event_id)` pairs — the PK set of the live `consumer_dedup` table.
    recorded: BTreeSet<(String, String)>,
}

impl DedupLedger {
    /// A fresh, empty ledger.
    pub fn new() -> DedupLedger {
        DedupLedger::default()
    }

    /// **Record the effect for `(consumer, event_id)`; return `true` iff this is the FIRST time**
    /// (the `INSERT … ON CONFLICT DO NOTHING` semantics: `true` = inserted = fire the effect;
    /// `false` = conflict = a duplicate delivery, absorb it). Idempotent: a repeated call with the
    /// same key always returns `false` after the first.
    pub fn record(&mut self, consumer: &str, event_id: &EventId) -> bool {
        self.recorded
            .insert((consumer.to_string(), event_id.0.clone()))
    }

    /// Whether `(consumer, event_id)` has already fired its effect (a read-only probe).
    pub fn seen(&self, consumer: &str, event_id: &EventId) -> bool {
        self.recorded
            .contains(&(consumer.to_string(), event_id.0.clone()))
    }

    /// The number of distinct effects recorded (= the number of distinct runs started). The
    /// exactly-once drill asserts this is `1` after delivering one event N times.
    pub fn effect_count(&self) -> usize {
        self.recorded.len()
    }
}

// ---------------------------------------------------------------------------------------------
// 3. The trust-tier evaluation + the single stamp (arch 02 §1.3; the security-critical half, X-1).
// ---------------------------------------------------------------------------------------------

/// **The run provenance the trust classification reads** (arch 02 §1.3). The two structural facts
/// that decide the tier, plus the authz fact from the ABAC edge:
/// - `is_fork` — does the run execute code from a fork (a PR from a fork, or any run running
///   untrusted contributor code)? This is the provenance fact a PR-from-fork carries.
/// - `targets_self_hosted` — does the run target a self-hosted runner pool? A self-hosted member
///   run is its own tier (the per-run token is scoped to one tenant's `SelfHosted` jobs).
/// - `read_excludes_fork` — the result of the ReBAC `read & !is_untrusted_fork` ABAC edge (contract
///   4.9) for this run subject: `true` iff the run is NOT stamped `is_untrusted_fork` (i.e. the
///   Exclusion did NOT subtract it — it is in the `read` set). The classification cross-checks the
///   structural `is_fork` against this authz fact: they MUST agree (a fork run that the edge would
///   admit, or a non-fork run the edge excludes, is a misconfiguration — fail closed to
///   `UntrustedFork`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunProvenance {
    /// The run executes fork / untrusted-contributor code.
    pub is_fork: bool,
    /// The run targets a self-hosted runner pool.
    pub targets_self_hosted: bool,
    /// The `read & !is_untrusted_fork` edge admits the run (NOT stamped `is_untrusted_fork`).
    pub read_excludes_fork: bool,
}

/// **Evaluate the run's trust tier ONCE from provenance + the `read & !is_untrusted_fork` ABAC edge
/// (contract 4.9; arch 02 §1.3).** This is the security-critical classification — fail CLOSED.
///
/// The rule (in priority order):
/// 1. If the run executes fork/untrusted code (`is_fork`) **OR** the ABAC edge does NOT exclude the
///    fork (`!read_excludes_fork` — the run IS stamped `is_untrusted_fork`), classify
///    [`TrustTier::UntrustedFork`]. The two facts are OR'd, not AND'd: EITHER the structural
///    provenance OR the authz stamp marking it untrusted is sufficient — a fork must never be able
///    to launder itself trusted by one fact disagreeing (the poisoned-pipeline-execution defence,
///    EI-02 §1).
/// 2. Else if the run targets a self-hosted pool, [`TrustTier::SelfHosted`].
/// 3. Else (a member push, edge-admitted, not self-hosted), [`TrustTier::Trusted`].
///
/// This is evaluated ONCE; [`stamp_trust`] / [`git_trust_of`] derive the two consistent stamps from
/// this one value.
///
/// **MUTATION-SCORE FLOOR (mandatory-core — the security classification, EI-01 §5).** The
/// trust-tier classifier ([`classify_trust`] + [`git_trust_of`] + [`stamp_trust`]) is the
/// security-critical poisoned-pipeline defence; its `cargo-mutants` mutation-score floor is
/// **≥ 90% viable mutants caught**. The exhaustive provenance-combination tests
/// (`stamp_is_consistent_for_all_provenance`, the per-tier classifier tests, the consistency drill)
/// are written to KILL every boundary/operator/branch mutant: flipping the `||` to `&&` (the
/// fail-closed OR), dropping a tier branch, or perturbing the `git_trust_of` projection all flip a
/// pinned assertion. A `< 90%` survivor count is a regression — the floor is never weakened to pass.
pub fn classify_trust(provenance: &RunProvenance) -> TrustTier {
    // Fail-closed OR: untrusted if the provenance says fork OR the edge did not admit it.
    if provenance.is_fork || !provenance.read_excludes_fork {
        return TrustTier::UntrustedFork;
    }
    if provenance.targets_self_hosted {
        return TrustTier::SelfHosted;
    }
    TrustTier::Trusted
}

/// **Project the evaluated [`TrustTier`] onto the `CheckStatus` (Git/merge-gate) trust tier (X-1).**
/// A TOTAL, deterministic map: `UntrustedFork` → git `UntrustedFork`; `Trusted` and `SelfHosted` →
/// git `Trusted`. The merge gate only cares about the fork exclusion (an `untrusted_fork` success
/// cannot self-satisfy a `required` context); a self-hosted member run is trusted *code* for
/// gating. Because this derives from the ONE evaluated tier, the `CheckStatus.trust_tier` can NEVER
/// diverge from the `JobSpec.trust_tier`'s fork verdict — the X-1 consistency invariant.
pub fn git_trust_of(tier: TrustTier) -> GitTrustTier {
    match tier {
        TrustTier::UntrustedFork => GitTrustTier::UntrustedFork,
        TrustTier::Trusted | TrustTier::SelfHosted => GitTrustTier::Trusted,
    }
}

/// **The single trust-tier stamp (arch 01 §2; X-1).** Evaluate the tier ONCE and return BOTH
/// stamps: the full [`TrustTier`] for the `JobSpec.trust_tier` (gating secrets/cache-scope/egress)
/// and the projected [`GitTrustTier`] for every `CheckStatus.trust_tier` (the merge gate). The two
/// are derived from the SAME evaluated value — one value, stamped once, 0 divergence. This is the
/// function the dispatch leg calls; the `JobSpec` builder (CI-P11) reads `.job_tier`, the
/// `CheckStatus` emit reads `.check_tier`.
pub fn stamp_trust(provenance: &RunProvenance) -> TrustStamp {
    let tier = classify_trust(provenance);
    TrustStamp {
        job_tier: tier,
        check_tier: git_trust_of(tier),
    }
}

/// The single stamp's two consistent faces (X-1): the `JobSpec` tier and the `CheckStatus` tier,
/// both derived from ONE evaluation. A fork PR is `UntrustedFork` on both; there is no path that
/// stamps them inconsistently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustStamp {
    /// The full tier stamped onto `JobSpec.trust_tier` (3-way: gates secrets/cache/egress).
    pub job_tier: TrustTier,
    /// The projected tier stamped onto every `CheckStatus.trust_tier` (2-way: the merge gate, X-1).
    pub check_tier: GitTrustTier,
}

impl TrustStamp {
    /// **The consistency invariant (X-1 drill): the two faces agree on the fork verdict.** True iff
    /// `job_tier == UntrustedFork` ⇔ `check_tier == UntrustedFork`. By construction
    /// ([`git_trust_of`]) this ALWAYS holds — the method makes the invariant assertable in the gate
    /// drill (0 divergence).
    pub fn is_consistent(&self) -> bool {
        let job_untrusted = self.job_tier == TrustTier::UntrustedFork;
        let check_untrusted = self.check_tier == GitTrustTier::UntrustedFork;
        job_untrusted == check_untrusted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{ObjectId, Principal, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn principal() -> Principal {
        Principal::stub(
            myelin_identity::PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )
    }

    /// Build an envelope of `type_` whose subject is a `run` object `<id>`.
    fn envelope(type_: &str, run_id: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("ev-{run_id}")),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("fr-par".into()),
            actor: Actor(principal()),
            subject: ArtifactRef(format!("myelin://t1/ci/run/{run_id}")),
            aggregate: AggregateKey("agg".into()),
            causation_id: None,
            correlation_id: CorrelationId("corr".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-23T00:00:00Z".into()),
            payload: serde_json::json!({}),
        }
    }

    /// All `run` objects visible (the trigger leg's permission compose is exercised in the matcher's
    /// own CDC; here the trigger fires for a visible run). The `_` member oracle is never consulted
    /// for `SetExpr::All`.
    fn visible_all() -> SetExpr {
        SetExpr::All
    }

    fn no_member(_: &RelMembership) -> bool {
        false
    }

    // ---- 1. The QueryAst trigger compile ----

    /// **An `on: pull_request` matcher matches `git.pr.opened` AND `git.pr.synchronized`, and does
    /// NOT match a push.** The trigger compiled to the ONE `QueryAst` over the projected envelope —
    /// not a CI DSL, not CEL (3.4 / OQ-C).
    #[test]
    fn pull_request_trigger_matches_the_right_events() {
        let m = compile_trigger(&OnTrigger::PullRequest).expect("compiles to QueryAst");
        for ty in [
            myelin_git::events::GIT_PR_OPENED,
            myelin_git::events::GIT_PR_SYNCHRONIZED,
        ] {
            assert!(
                trigger_matches(&m, &envelope(ty, "r1"), &visible_all(), &no_member).unwrap(),
                "the pull_request trigger fires on {ty}"
            );
        }
        assert!(
            !trigger_matches(
                &m,
                &envelope(myelin_git::events::GIT_REF_UPDATED, "r1"),
                &visible_all(),
                &no_member,
            )
            .unwrap(),
            "a push does NOT arm a pull_request trigger"
        );
    }

    /// **An `on: push` matcher matches `git.ref.updated` only.**
    #[test]
    fn push_trigger_matches_only_ref_updated() {
        let m = compile_trigger(&OnTrigger::Push).expect("compiles");
        assert!(trigger_matches(
            &m,
            &envelope(myelin_git::events::GIT_REF_UPDATED, "r1"),
            &visible_all(),
            &no_member,
        )
        .unwrap());
        assert!(!trigger_matches(
            &m,
            &envelope(myelin_git::events::GIT_PR_OPENED, "r1"),
            &visible_all(),
            &no_member,
        )
        .unwrap());
    }

    /// **The matcher selects the `run` object type (contract 4.9), and the predicate IS a
    /// `QueryAst`** (the byte-identical no-drift property — the matcher's predicate serialises
    /// exactly as a bare `QueryAst`, X-3). The subject-filter lowers the type pin to the NATS
    /// subject (the cheap server-side prefilter).
    #[test]
    fn the_matcher_is_the_one_queryast_over_run() {
        let m = compile_trigger(&OnTrigger::Push).expect("compiles");
        assert_eq!(m.object_type().0, RUN_OBJECT_TYPE);
        // The single-type push trigger lowers to an exact subject filter (the type pin).
        assert_eq!(
            m.compile_subject_filter().as_deref(),
            Some(myelin_git::events::GIT_REF_UPDATED),
            "the type pin lowers to the NATS subject (the cheap prefilter, §4.5)"
        );
    }

    /// **The 0-leak permission compose: a run NOT in the viewer's visible set never matches, even
    /// when the predicate would hold.** The matcher tests visibility FIRST (the load-bearing
    /// invariant) — a trigger can never arm a run the subject cannot read.
    #[test]
    fn invisible_run_never_arms_even_on_a_type_hit() {
        let m = compile_trigger(&OnTrigger::Push).expect("compiles");
        let visible = SetExpr::Ids(vec![ObjectId("other".into())]);
        assert!(
            !trigger_matches(
                &m,
                &envelope(myelin_git::events::GIT_REF_UPDATED, "r1"),
                &visible,
                &no_member,
            )
            .unwrap(),
            "the predicate holds, but the run is invisible → 0 match (0-leak)"
        );
    }

    // ---- 2. The dedup ledger — one effect per event_id ----

    /// **One effect per `event_id`: the FIRST `record` returns true, every redelivery returns
    /// false** (the `INSERT … ON CONFLICT DO NOTHING` semantics, contract 2.5).
    #[test]
    fn dedup_yields_one_effect_per_event_id() {
        let mut ledger = DedupLedger::new();
        let ev = EventId("ev-push-1".into());
        assert!(
            ledger.record(TRIGGER_CONSUMER, &ev),
            "first delivery fires the effect"
        );
        assert!(
            !ledger.record(TRIGGER_CONSUMER, &ev),
            "redelivery is absorbed (no second effect)"
        );
        assert!(ledger.seen(TRIGGER_CONSUMER, &ev));
        assert_eq!(ledger.effect_count(), 1, "exactly one effect recorded");
    }

    /// **THE exactly-once-effect DRILL (the prompt GATE): a triggering event delivered TWICE (the
    /// at-least-once transport) → exactly ONE run (1 effect per `event_id`; 0 duplicate runs).**
    /// This is the failure-injection scenario (deliver the trigger twice) proven deterministically.
    #[test]
    fn drill_deliver_twice_yields_exactly_one_run() {
        let m = compile_trigger(&OnTrigger::Push).expect("compiles");
        let mut ledger = DedupLedger::new();
        let env = envelope(myelin_git::events::GIT_REF_UPDATED, "r1");

        let mut runs_started = 0u32;
        // The bus delivers the SAME event twice (at-least-once redelivery).
        for _ in 0..2 {
            let matched = trigger_matches(&m, &env, &visible_all(), &no_member).expect("eval ok");
            if matched && ledger.record(TRIGGER_CONSUMER, &env.event_id) {
                // The effect: start exactly one run (CI-P11 does the actual start; here we count).
                runs_started += 1;
            }
        }
        assert_eq!(
            runs_started, 1,
            "one push (one event_id) = exactly ONE run, even under double delivery"
        );
        assert_eq!(ledger.effect_count(), 1, "dedup-count = 0 duplicate runs");
    }

    // ---- 3. The trust-tier classifier + the single consistent stamp ----

    /// **member push → Trusted.** Not a fork, edge admits it, not self-hosted.
    #[test]
    fn member_push_is_trusted() {
        let prov = RunProvenance {
            is_fork: false,
            targets_self_hosted: false,
            read_excludes_fork: true,
        };
        assert_eq!(classify_trust(&prov), TrustTier::Trusted);
    }

    /// **fork PR → UntrustedFork** (the security-critical classification).
    #[test]
    fn fork_pr_is_untrusted_fork() {
        let prov = RunProvenance {
            is_fork: true,
            targets_self_hosted: false,
            // Even if the structural fork flag were the only signal, the edge agrees.
            read_excludes_fork: false,
        };
        assert_eq!(classify_trust(&prov), TrustTier::UntrustedFork);
    }

    /// **self-hosted member run → SelfHosted.**
    #[test]
    fn self_hosted_member_run_is_self_hosted() {
        let prov = RunProvenance {
            is_fork: false,
            targets_self_hosted: true,
            read_excludes_fork: true,
        };
        assert_eq!(classify_trust(&prov), TrustTier::SelfHosted);
    }

    /// **Fail-closed: the structural fork flag OR the authz stamp marks untrusted (either suffices).**
    /// A run the edge marks `is_untrusted_fork` (`read_excludes_fork = false`) is UntrustedFork even
    /// if the structural `is_fork` flag were (mistakenly) false — a fork cannot launder itself
    /// trusted by one fact disagreeing.
    #[test]
    fn edge_stamp_alone_forces_untrusted_fork() {
        let prov = RunProvenance {
            is_fork: false,
            targets_self_hosted: false,
            read_excludes_fork: false,
        };
        assert_eq!(classify_trust(&prov), TrustTier::UntrustedFork);
    }

    /// **THE trust-stamp-consistency GATE (X-1): a fork PR is `UntrustedFork` on BOTH
    /// `JobSpec.trust_tier` AND `CheckStatus.trust_tier` — the SAME value, 0 divergence.**
    #[test]
    fn drill_fork_pr_stamps_both_halves_untrusted_zero_divergence() {
        let prov = RunProvenance {
            is_fork: true,
            targets_self_hosted: false,
            read_excludes_fork: false,
        };
        let stamp = stamp_trust(&prov);
        assert_eq!(stamp.job_tier, TrustTier::UntrustedFork, "JobSpec tier");
        assert_eq!(
            stamp.check_tier,
            GitTrustTier::UntrustedFork,
            "CheckStatus tier — the SAME fork verdict"
        );
        assert!(stamp.is_consistent(), "0 divergence between the two stamps");
    }

    /// **A trusted member run stamps both halves trusted (consistent), and a self-hosted member run
    /// projects to git `Trusted` (the merge gate only excludes forks).** The stamp is consistent for
    /// every tier — there is no path that diverges.
    #[test]
    fn every_tier_stamps_consistently() {
        for prov in [
            RunProvenance {
                is_fork: false,
                targets_self_hosted: false,
                read_excludes_fork: true,
            },
            RunProvenance {
                is_fork: false,
                targets_self_hosted: true,
                read_excludes_fork: true,
            },
        ] {
            let stamp = stamp_trust(&prov);
            assert_eq!(
                stamp.check_tier,
                GitTrustTier::Trusted,
                "a non-fork run is trusted CODE for the gate ({:?})",
                stamp.job_tier
            );
            assert!(stamp.is_consistent(), "0 divergence ({:?})", stamp.job_tier);
        }
    }

    /// **The single-stamp invariant holds across ALL provenance combinations (exhaustive): the two
    /// faces NEVER disagree on the fork verdict.** This is the mutation-resistant core property —
    /// every one of the 8 provenance combos yields a consistent stamp.
    #[test]
    fn stamp_is_consistent_for_all_provenance() {
        for is_fork in [false, true] {
            for targets_self_hosted in [false, true] {
                for read_excludes_fork in [false, true] {
                    let prov = RunProvenance {
                        is_fork,
                        targets_self_hosted,
                        read_excludes_fork,
                    };
                    let stamp = stamp_trust(&prov);
                    assert!(
                        stamp.is_consistent(),
                        "stamp diverged for {prov:?} (job={:?} check={:?})",
                        stamp.job_tier,
                        stamp.check_tier
                    );
                    // And the fork verdict is exactly: fork OR not-edge-admitted.
                    let expect_untrusted = is_fork || !read_excludes_fork;
                    assert_eq!(
                        stamp.job_tier == TrustTier::UntrustedFork,
                        expect_untrusted,
                        "the fork verdict for {prov:?}"
                    );
                }
            }
        }
    }
}
